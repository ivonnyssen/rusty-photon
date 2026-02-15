//! Sentinel - Observatory monitoring and notification service
//!
//! Polls ASCOM Alpaca devices, detects state transitions, and sends notifications.

pub mod alpaca_client;
pub mod config;
pub mod dashboard;
pub mod engine;
pub mod error;
pub mod io;
pub mod monitor;
pub mod notifier;
pub mod pushover;
pub mod state;

pub use config::{load_config, Config};
pub use error::{Result, SentinelError};

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::alpaca_client::AlpacaSafetyMonitor;
use crate::config::MonitorConfig;
use crate::engine::Engine;
use crate::io::ReqwestHttpClient;
use crate::monitor::Monitor;
use crate::notifier::Notifier;
use crate::pushover::PushoverNotifier;

/// Run the sentinel service with the given configuration
pub async fn run(config: Config) -> Result<()> {
    let http: Arc<dyn io::HttpClient> = Arc::new(ReqwestHttpClient::new());
    let cancel = CancellationToken::new();

    // Build monitors
    let mut monitors: Vec<Arc<dyn Monitor>> = Vec::new();
    let mut polling_intervals = Vec::new();
    for monitor_config in &config.monitors {
        let monitor: Arc<dyn Monitor> = match monitor_config {
            MonitorConfig::AlpacaSafetyMonitor { .. } => {
                Arc::new(AlpacaSafetyMonitor::new(monitor_config, Arc::clone(&http)))
            }
        };
        let interval = Duration::from_secs(monitor_config.polling_interval_seconds());
        polling_intervals.push((monitor.name().to_string(), interval));
        monitors.push(monitor);
    }

    // Build notifiers
    let mut notifiers: Vec<Arc<dyn Notifier>> = Vec::new();
    for notifier_config in &config.notifiers {
        let notifier: Arc<dyn Notifier> = match notifier_config {
            config::NotifierConfig::Pushover { .. } => {
                Arc::new(PushoverNotifier::new(notifier_config, Arc::clone(&http)))
            }
        };
        notifiers.push(notifier);
    }

    // Build shared state
    let monitor_names: Vec<String> = monitors.iter().map(|m| m.name().to_string()).collect();
    let state = state::new_state_handle(monitor_names, config.dashboard.history_size);

    // Build engine
    let engine = Engine::new(
        monitors,
        notifiers,
        &config,
        Arc::clone(&state),
        cancel.clone(),
    );

    // Connect monitors
    engine.connect_all().await;

    // Setup shutdown handler
    let cancel_for_signal = cancel.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to listen for ctrl-c");
        tracing::info!("Shutdown signal received");
        cancel_for_signal.cancel();
    });

    // Start dashboard if enabled
    if config.dashboard.enabled {
        let dashboard_port = config.dashboard.port;
        let dashboard_state = Arc::clone(&state);
        let cancel_for_dashboard = cancel.clone();

        tokio::spawn(async move {
            let router = dashboard::build_router(dashboard_state);
            let addr = SocketAddr::from(([0, 0, 0, 0], dashboard_port));
            tracing::info!("Dashboard listening on http://{}", addr);

            let listener = match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!(
                        "Failed to bind dashboard to port {}: {}. Continuing without dashboard.",
                        dashboard_port,
                        e
                    );
                    return;
                }
            };

            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    cancel_for_dashboard.cancelled().await;
                })
                .await
                .ok();

            tracing::debug!("Dashboard stopped");
        });
    }

    tracing::info!("Sentinel engine started");

    // Run the engine (blocks until cancelled)
    engine.run(&polling_intervals).await;

    // Disconnect monitors
    engine.disconnect_all().await;
    tracing::info!("Sentinel engine stopped");

    Ok(())
}
