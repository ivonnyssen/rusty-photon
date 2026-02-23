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

/// Builder for running the sentinel service with injectable dependencies
pub struct SentinelRunner {
    config: Config,
    http: Arc<dyn io::HttpClient>,
    cancel: CancellationToken,
}

impl SentinelRunner {
    /// Create a new runner with production defaults
    pub fn new(config: Config) -> Self {
        Self {
            config,
            http: Arc::new(ReqwestHttpClient::default()),
            cancel: CancellationToken::new(),
        }
    }

    /// Override the HTTP client (useful for testing)
    pub fn with_http_client(mut self, http: Arc<dyn io::HttpClient>) -> Self {
        self.http = http;
        self
    }

    /// Override the cancellation token (useful for testing)
    pub fn with_cancellation_token(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    /// Run the sentinel service
    pub async fn run(self) -> Result<()> {
        let http = self.http;
        let cancel = self.cancel;
        let config = self.config;

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
        let monitors_with_intervals: Vec<(String, u64)> = polling_intervals
            .iter()
            .map(|(name, dur)| (name.clone(), dur.as_millis() as u64))
            .collect();
        let state = state::new_state_handle(monitors_with_intervals, config.dashboard.history_size);

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DashboardConfig, MonitorConfig};
    use crate::io::{HttpResponse, MockHttpClient};

    fn pre_cancelled_token() -> CancellationToken {
        let token = CancellationToken::new();
        token.cancel();
        token
    }

    fn disabled_dashboard() -> DashboardConfig {
        DashboardConfig {
            enabled: false,
            ..DashboardConfig::default()
        }
    }

    fn ok_response() -> HttpResponse {
        HttpResponse {
            status: 200,
            body: r#"{"Value": true, "ErrorNumber": 0, "ErrorMessage": ""}"#.to_string(),
        }
    }

    fn single_monitor_config(name: &str, host: &str, port: u16, device_number: u32) -> Config {
        Config {
            monitors: vec![MonitorConfig::AlpacaSafetyMonitor {
                name: name.to_string(),
                host: host.to_string(),
                port,
                device_number,
                polling_interval_seconds: 30,
            }],
            dashboard: disabled_dashboard(),
            ..Config::default()
        }
    }

    #[tokio::test]
    async fn run_with_empty_config_completes() {
        let config = Config {
            dashboard: disabled_dashboard(),
            ..Config::default()
        };
        let mock = MockHttpClient::new();

        SentinelRunner::new(config)
            .with_http_client(Arc::new(mock))
            .with_cancellation_token(pre_cancelled_token())
            .run()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn run_connects_and_disconnects_monitors() {
        let config = single_monitor_config("Test", "localhost", 11111, 0);
        let mut mock = MockHttpClient::new();

        mock.expect_put_form()
            .withf(|_url, params| params.contains(&("Connected", "true")))
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(ok_response()) }));

        mock.expect_put_form()
            .withf(|_url, params| params.contains(&("Connected", "false")))
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(ok_response()) }));

        mock.expect_get()
            .returning(|_| Box::pin(async { Ok(ok_response()) }));

        SentinelRunner::new(config)
            .with_http_client(Arc::new(mock))
            .with_cancellation_token(pre_cancelled_token())
            .run()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn run_creates_monitor_with_correct_url() {
        let config = single_monitor_config("Test", "myhost", 9999, 2);
        let mut mock = MockHttpClient::new();

        mock.expect_put_form()
            .withf(|url, _params| url == "http://myhost:9999/api/v1/safetymonitor/2/connected")
            .times(2)
            .returning(|_, _| Box::pin(async { Ok(ok_response()) }));

        mock.expect_get()
            .withf(|url| url == "http://myhost:9999/api/v1/safetymonitor/2/issafe")
            .returning(|_| Box::pin(async { Ok(ok_response()) }));

        SentinelRunner::new(config)
            .with_http_client(Arc::new(mock))
            .with_cancellation_token(pre_cancelled_token())
            .run()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn run_with_multiple_monitors_connects_all() {
        let config = Config {
            monitors: vec![
                MonitorConfig::AlpacaSafetyMonitor {
                    name: "Monitor1".to_string(),
                    host: "localhost".to_string(),
                    port: 11111,
                    device_number: 0,
                    polling_interval_seconds: 30,
                },
                MonitorConfig::AlpacaSafetyMonitor {
                    name: "Monitor2".to_string(),
                    host: "localhost".to_string(),
                    port: 11111,
                    device_number: 1,
                    polling_interval_seconds: 30,
                },
            ],
            dashboard: disabled_dashboard(),
            ..Config::default()
        };
        let mut mock = MockHttpClient::new();

        mock.expect_put_form()
            .times(4)
            .returning(|_, _| Box::pin(async { Ok(ok_response()) }));

        mock.expect_get()
            .times(2)
            .returning(|_| Box::pin(async { Ok(ok_response()) }));

        SentinelRunner::new(config)
            .with_http_client(Arc::new(mock))
            .with_cancellation_token(pre_cancelled_token())
            .run()
            .await
            .unwrap();
    }
}
