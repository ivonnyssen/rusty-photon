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

use tokio_util::sync::CancellationToken;

use crate::alpaca_client::AlpacaSafetyMonitor;
use crate::engine::Engine;
use crate::io::ReqwestHttpClient;
use crate::monitor::Monitor;
use crate::notifier::Notifier;
use crate::pushover::PushoverNotifier;

/// Factory methods for building monitors and notifiers from config.
///
/// These live in `lib.rs` (rather than `config.rs`) because they depend on
/// concrete types (`AlpacaSafetyMonitor`, `PushoverNotifier`) that are defined
/// in sibling modules.
impl Config {
    pub fn build_monitors(&self, http: &Arc<dyn io::HttpClient>) -> Vec<Arc<dyn Monitor>> {
        self.monitors
            .iter()
            .map(|monitor_config| -> Arc<dyn Monitor> {
                match monitor_config {
                    config::MonitorConfig::AlpacaSafetyMonitor { .. } => {
                        Arc::new(AlpacaSafetyMonitor::new(monitor_config, Arc::clone(http)))
                    }
                }
            })
            .collect()
    }

    pub fn build_notifiers(&self, http: &Arc<dyn io::HttpClient>) -> Vec<Arc<dyn Notifier>> {
        self.notifiers
            .iter()
            .map(|notifier_config| -> Arc<dyn Notifier> {
                match notifier_config {
                    config::NotifierConfig::Pushover { .. } => {
                        Arc::new(PushoverNotifier::new(notifier_config, Arc::clone(http)))
                    }
                }
            })
            .collect()
    }
}

/// Builder for the sentinel service with injectable dependencies
pub struct SentinelBuilder {
    config: Config,
    http: Arc<dyn io::HttpClient>,
    cancel: CancellationToken,
    monitors: Option<Vec<Arc<dyn Monitor>>>,
    notifiers: Option<Vec<Arc<dyn Notifier>>>,
}

impl SentinelBuilder {
    /// Create a new builder with production defaults
    pub fn new(config: Config) -> Self {
        Self {
            config,
            http: Arc::new(ReqwestHttpClient::default()),
            cancel: CancellationToken::new(),
            monitors: None,
            notifiers: None,
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

    /// Inject pre-built monitors instead of constructing them from config
    pub fn with_monitors(mut self, monitors: Vec<Arc<dyn Monitor>>) -> Self {
        self.monitors = Some(monitors);
        self
    }

    /// Inject pre-built notifiers instead of constructing them from config
    pub fn with_notifiers(mut self, notifiers: Vec<Arc<dyn Notifier>>) -> Self {
        self.notifiers = Some(notifiers);
        self
    }

    /// Build the sentinel service: constructs monitors, notifiers, engine, connects monitors,
    /// and binds the dashboard listener if enabled.
    pub async fn build(self) -> Result<Sentinel> {
        let http = self.http;
        let cancel = self.cancel;
        let config = self.config;

        // Use injected monitors/notifiers or fall back to config factories
        let monitors = self
            .monitors
            .unwrap_or_else(|| config.build_monitors(&http));
        let notifiers = self
            .notifiers
            .unwrap_or_else(|| config.build_notifiers(&http));

        // Build shared state
        let monitors_with_intervals: Vec<(String, u64)> = monitors
            .iter()
            .map(|m| {
                (
                    m.name().to_string(),
                    m.polling_interval().as_millis() as u64,
                )
            })
            .collect();
        let state = state::new_state_handle(monitors_with_intervals, config.dashboard.history_size);

        // Build engine
        let engine = Engine::new(
            monitors,
            notifiers,
            config.transitions.clone(),
            Arc::clone(&state),
            cancel.clone(),
        );

        // Connect monitors
        engine.connect_all().await;

        // Bind dashboard listener if enabled
        let dashboard_listener = if config.dashboard.enabled {
            let addr = SocketAddr::from(([0, 0, 0, 0], config.dashboard.port));
            match tokio::net::TcpListener::bind(addr).await {
                Ok(listener) => {
                    tracing::debug!("Dashboard bound to {}", addr);
                    Some(listener)
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to bind dashboard to port {}: {}. Continuing without dashboard.",
                        config.dashboard.port,
                        e
                    );
                    None
                }
            }
        } else {
            None
        };

        Ok(Sentinel {
            engine,
            state,
            cancel,
            dashboard_listener,
        })
    }
}

/// A fully constructed sentinel service ready to run
pub struct Sentinel {
    engine: Engine,
    state: state::StateHandle,
    cancel: CancellationToken,
    dashboard_listener: Option<tokio::net::TcpListener>,
}

impl Sentinel {
    /// Start the sentinel service: runs the polling loop until cancelled, then disconnects.
    pub async fn start(self) -> Result<()> {
        let cancel = self.cancel;

        // Setup shutdown handler
        let cancel_for_signal = cancel.clone();
        tokio::spawn(async move {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to listen for ctrl-c");
            tracing::info!("Shutdown signal received");
            cancel_for_signal.cancel();
        });

        // Start dashboard if we have a bound listener
        if let Some(listener) = self.dashboard_listener {
            let dashboard_state = Arc::clone(&self.state);
            let cancel_for_dashboard = cancel.clone();

            tracing::info!(
                "Dashboard listening on http://{}",
                listener.local_addr().unwrap()
            );

            tokio::spawn(async move {
                let router = dashboard::build_router(dashboard_state);

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
        self.engine.run().await;

        // Disconnect monitors
        self.engine.disconnect_all().await;
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

    // Build-phase tests (call build() only, drop Sentinel)

    #[tokio::test]
    async fn build_with_empty_config_succeeds() {
        let config = Config {
            dashboard: disabled_dashboard(),
            ..Config::default()
        };
        let mock = MockHttpClient::new();

        SentinelBuilder::new(config)
            .with_http_client(Arc::new(mock))
            .build()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn build_connects_monitors() {
        let config = single_monitor_config("Test", "localhost", 11111, 0);
        let mut mock = MockHttpClient::new();

        mock.expect_put_form()
            .withf(|_url, params| params.contains(&("Connected", "true")))
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(ok_response()) }));

        SentinelBuilder::new(config)
            .with_http_client(Arc::new(mock))
            .build()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn build_creates_monitor_with_correct_url() {
        let config = single_monitor_config("Test", "myhost", 9999, 2);
        let mut mock = MockHttpClient::new();

        mock.expect_put_form()
            .withf(|url, _params| url == "http://myhost:9999/api/v1/safetymonitor/2/connected")
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(ok_response()) }));

        SentinelBuilder::new(config)
            .with_http_client(Arc::new(mock))
            .build()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn build_with_multiple_monitors_connects_all() {
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
            .withf(|_url, params| params.contains(&("Connected", "true")))
            .times(2)
            .returning(|_, _| Box::pin(async { Ok(ok_response()) }));

        SentinelBuilder::new(config)
            .with_http_client(Arc::new(mock))
            .build()
            .await
            .unwrap();
    }

    // Lifecycle tests (call build() + start() with pre-cancelled token)

    #[tokio::test]
    async fn start_with_empty_config_completes() {
        let config = Config {
            dashboard: disabled_dashboard(),
            ..Config::default()
        };
        let mock = MockHttpClient::new();

        SentinelBuilder::new(config)
            .with_http_client(Arc::new(mock))
            .with_cancellation_token(pre_cancelled_token())
            .build()
            .await
            .unwrap()
            .start()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn start_disconnects_monitors_on_shutdown() {
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

        SentinelBuilder::new(config)
            .with_http_client(Arc::new(mock))
            .with_cancellation_token(pre_cancelled_token())
            .build()
            .await
            .unwrap()
            .start()
            .await
            .unwrap();
    }

    // Injection tests

    #[derive(Debug)]
    struct StubMonitor {
        monitor_name: String,
    }

    #[async_trait::async_trait]
    impl Monitor for StubMonitor {
        fn name(&self) -> &str {
            &self.monitor_name
        }

        async fn poll(&self) -> monitor::MonitorState {
            monitor::MonitorState::Safe
        }

        async fn connect(&self) -> Result<()> {
            Ok(())
        }

        async fn disconnect(&self) -> Result<()> {
            Ok(())
        }

        fn polling_interval(&self) -> std::time::Duration {
            std::time::Duration::from_secs(30)
        }
    }

    #[tokio::test]
    async fn build_with_injected_monitors_uses_them() {
        let config = Config {
            dashboard: disabled_dashboard(),
            ..Config::default()
        };
        let stub: Arc<dyn Monitor> = Arc::new(StubMonitor {
            monitor_name: "injected".to_string(),
        });

        // No HTTP mock expectations needed — injected monitors bypass config factories
        let mock = MockHttpClient::new();

        SentinelBuilder::new(config)
            .with_http_client(Arc::new(mock))
            .with_monitors(vec![stub])
            .build()
            .await
            .unwrap();
    }
}
