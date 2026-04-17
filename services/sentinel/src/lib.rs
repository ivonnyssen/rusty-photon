#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
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
    pub fn build_monitors(
        &self,
        http: &Arc<dyn io::HttpClient>,
        ca_path: Option<&std::path::Path>,
    ) -> Vec<Arc<dyn Monitor>> {
        self.monitors
            .iter()
            .map(|monitor_config| -> Arc<dyn Monitor> {
                match monitor_config {
                    config::MonitorConfig::AlpacaSafetyMonitor { auth, .. } => {
                        let client: Arc<dyn io::HttpClient> = match auth {
                            Some(a) => {
                                match ReqwestHttpClient::with_auth(
                                    ca_path,
                                    a.username.clone(),
                                    a.password.clone(),
                                ) {
                                    Ok(c) => Arc::new(c),
                                    Err(e) => {
                                        tracing::error!(
                                            "Failed to build auth HTTP client: {e}. \
                                             Falling back to shared client."
                                        );
                                        Arc::clone(http)
                                    }
                                }
                            }
                            None => Arc::clone(http),
                        };
                        Arc::new(AlpacaSafetyMonitor::new(monitor_config, client))
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
    /// Create a new builder with production defaults.
    ///
    /// When `config.ca_cert` is set, the HTTP client trusts that CA for
    /// connecting to TLS-enabled Alpaca services.
    pub fn new(config: Config) -> Self {
        let ca_path = config.ca_cert.as_deref().map(rp_tls::config::expand_tilde);
        let http: Arc<dyn io::HttpClient> = match ReqwestHttpClient::new(ca_path.as_deref()) {
            Ok(client) => Arc::new(client),
            Err(e) => {
                tracing::error!("Failed to build HTTP client with CA cert: {e}. Falling back to default client.");
                Arc::new(ReqwestHttpClient::default())
            }
        };
        Self {
            config,
            http,
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
        let ca_path = config.ca_cert.as_deref().map(rp_tls::config::expand_tilde);
        let monitors = self
            .monitors
            .unwrap_or_else(|| config.build_monitors(&http, ca_path.as_deref()));
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

        let dashboard_tls = config.dashboard.tls.clone();
        let dashboard_auth = config.dashboard.auth.clone();

        Ok(Sentinel {
            engine,
            state,
            cancel,
            dashboard_listener,
            dashboard_tls,
            dashboard_auth,
        })
    }
}

/// A fully constructed sentinel service ready to run
pub struct Sentinel {
    engine: Engine,
    state: state::StateHandle,
    cancel: CancellationToken,
    dashboard_listener: Option<tokio::net::TcpListener>,
    dashboard_tls: Option<rp_tls::config::TlsConfig>,
    dashboard_auth: Option<rp_auth::config::AuthConfig>,
}

impl Sentinel {
    /// Returns whether the dashboard listener was successfully bound during build.
    pub fn has_dashboard(&self) -> bool {
        self.dashboard_listener.is_some()
    }

    /// Returns a clone of the cancellation token used by this sentinel instance.
    ///
    /// Cancelling this token stops the engine polling loop and dashboard server,
    /// which is needed when the outer `run_server_loop` handles a reload signal.
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Start the sentinel service: runs the polling loop until cancelled, then disconnects.
    pub async fn start(self) -> Result<()> {
        let cancel = self.cancel;

        // Setup shutdown handler (Ctrl+C and SIGTERM)
        let cancel_for_signal = cancel.clone();
        tokio::spawn(async move {
            let ctrl_c = async {
                tokio::signal::ctrl_c()
                    .await
                    .expect("Failed to listen for ctrl-c");
            };

            #[cfg(unix)]
            let terminate = async {
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("Failed to install SIGTERM handler")
                    .recv()
                    .await;
            };

            #[cfg(not(unix))]
            let terminate = std::future::pending::<()>();

            tokio::select! {
                () = ctrl_c => tracing::info!("Received Ctrl+C"),
                () = terminate => tracing::info!("Received SIGTERM"),
            }
            cancel_for_signal.cancel();
        });

        // Start dashboard if we have a bound listener
        if let Some(listener) = self.dashboard_listener {
            let dashboard_state = Arc::clone(&self.state);
            let cancel_for_dashboard = cancel.clone();
            let dashboard_tls = self.dashboard_tls;
            let dashboard_auth = self.dashboard_auth;

            let addr = listener.local_addr().unwrap();
            let scheme = if dashboard_tls.is_some() {
                "https"
            } else {
                "http"
            };
            tracing::info!("Dashboard listening on {scheme}://{}", addr);
            println!("Sentinel dashboard bound_addr={}", addr);

            tokio::spawn(async move {
                let router = dashboard::build_router(dashboard_state);

                // Layer authentication if configured
                let router = match &dashboard_auth {
                    Some(auth) => {
                        if dashboard_tls.is_none() {
                            tracing::warn!(
                                "Authentication is enabled but TLS is not. \
                                 Credentials will be transmitted in cleartext. \
                                 Consider enabling TLS (see `rp init-tls`)."
                            );
                        }
                        rp_auth::layer(router, auth)
                    }
                    None => router,
                };

                match dashboard_tls {
                    Some(ref tls_config) => {
                        rp_tls::server::serve_tls(listener, router, tls_config, async move {
                            cancel_for_dashboard.cancelled().await;
                        })
                        .await
                        .ok();
                    }
                    None => {
                        axum::serve(listener, router)
                            .with_graceful_shutdown(async move {
                                cancel_for_dashboard.cancelled().await;
                            })
                            .await
                            .ok();
                    }
                }

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

/// Run the sentinel in a loop, restarting on reload signal and exiting on stop.
///
/// On each iteration the config file is re-read, `apply_overrides` is invoked so
/// CLI flags like `--dashboard-port` continue to shadow the config file across
/// reloads, secrets are resolved, the sentinel is rebuilt, and a fresh dashboard
/// listener is bound. The `stop` and `reload` closures return futures that
/// complete when the respective signal is received.
///
/// Because sentinel spawns background tasks (dashboard, engine polling) that use a
/// [`CancellationToken`], both stop and reload cancel the token to ensure clean
/// shutdown of all spawned work before the next iteration or exit.
pub async fn run_server_loop(
    config_path: &std::path::Path,
    mut apply_overrides: impl FnMut(&mut Config),
    mut stop: impl FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>,
    mut reload: impl FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    loop {
        let mut config = load_config(config_path)?;
        apply_overrides(&mut config);
        config.resolve_secrets()?;
        tracing::info!("Starting sentinel service");
        let sentinel = SentinelBuilder::new(config).build().await?;
        let cancel = sentinel.cancel_token();
        tokio::select! {
            result = sentinel.start() => return result.map_err(Into::into),
            _ = stop() => { cancel.cancel(); tracing::info!("Received stop signal"); break; }
            _ = reload() => { cancel.cancel(); tracing::info!("Reloading configuration"); continue; }
        }
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::config::MonitorConfig;
    use crate::io::MockHttpClient;

    #[tokio::test]
    async fn build_notifiers_creates_pushover_from_config() {
        let config = Config {
            notifiers: vec![config::NotifierConfig::Pushover {
                api_token: "tok".to_string(),
                user_key: "usr".to_string(),
                default_title: "Alert".to_string(),
                default_priority: 0,
                default_sound: "pushover".to_string(),
            }],
            ..Config::default()
        };
        let mock = MockHttpClient::new();
        let http: Arc<dyn io::HttpClient> = Arc::new(mock);

        let notifiers = config.build_notifiers(&http);

        assert_eq!(notifiers.len(), 1);
        assert_eq!(notifiers[0].type_name(), "pushover");
    }

    #[test]
    fn build_monitors_creates_alpaca_from_config() {
        let config = Config {
            monitors: vec![MonitorConfig::AlpacaSafetyMonitor {
                name: "Test Monitor".to_string(),
                host: "localhost".to_string(),
                port: 11111,
                device_number: 0,
                polling_interval_seconds: 30,
                scheme: "http".to_string(),
                auth: None,
            }],
            ..Config::default()
        };
        let mock = MockHttpClient::new();
        let http: Arc<dyn io::HttpClient> = Arc::new(mock);

        let monitors = config.build_monitors(&http, None);

        assert_eq!(monitors.len(), 1);
        assert_eq!(monitors[0].name(), "Test Monitor");
    }

    #[tokio::test]
    async fn builder_with_http_client_uses_injected_client() {
        let mock = MockHttpClient::new();
        let http: Arc<dyn io::HttpClient> = Arc::new(mock);
        let cancel = CancellationToken::new();
        cancel.cancel();

        let sentinel = SentinelBuilder::new(Config::default())
            .with_http_client(http)
            .with_cancellation_token(cancel)
            .build()
            .await
            .unwrap();

        sentinel.start().await.unwrap();
    }

    #[tokio::test]
    async fn builder_with_monitors_skips_config_factory() {
        let cancel = CancellationToken::new();
        cancel.cancel();

        let config = Config {
            monitors: vec![MonitorConfig::AlpacaSafetyMonitor {
                name: "Should Be Ignored".to_string(),
                host: "localhost".to_string(),
                port: 11111,
                device_number: 0,
                polling_interval_seconds: 30,
                scheme: "http".to_string(),
                auth: None,
            }],
            ..Config::default()
        };

        let mock = MockHttpClient::new();
        let http: Arc<dyn io::HttpClient> = Arc::new(mock);

        // Injecting empty monitors should override the config factory
        let sentinel = SentinelBuilder::new(config)
            .with_http_client(http)
            .with_monitors(vec![])
            .with_cancellation_token(cancel)
            .build()
            .await
            .unwrap();

        sentinel.start().await.unwrap();
    }

    #[tokio::test]
    async fn builder_with_notifiers_skips_config_factory() {
        let cancel = CancellationToken::new();
        cancel.cancel();

        let config = Config {
            notifiers: vec![config::NotifierConfig::Pushover {
                api_token: "tok".to_string(),
                user_key: "usr".to_string(),
                default_title: "Alert".to_string(),
                default_priority: 0,
                default_sound: "pushover".to_string(),
            }],
            ..Config::default()
        };

        let mock = MockHttpClient::new();
        let http: Arc<dyn io::HttpClient> = Arc::new(mock);

        // Injecting empty notifiers should override the config factory
        let sentinel = SentinelBuilder::new(config)
            .with_http_client(http)
            .with_notifiers(vec![])
            .with_cancellation_token(cancel)
            .build()
            .await
            .unwrap();

        sentinel.start().await.unwrap();
    }

    #[tokio::test]
    async fn has_dashboard_true_when_enabled() {
        let config = Config {
            dashboard: config::DashboardConfig {
                enabled: true,
                port: 0,
                ..config::DashboardConfig::default()
            },
            ..Config::default()
        };
        let mock = MockHttpClient::new();
        let http: Arc<dyn io::HttpClient> = Arc::new(mock);

        let sentinel = SentinelBuilder::new(config)
            .with_http_client(http)
            .build()
            .await
            .unwrap();

        assert!(sentinel.has_dashboard());
    }

    #[tokio::test]
    async fn has_dashboard_false_when_disabled() {
        let config = Config {
            dashboard: config::DashboardConfig {
                enabled: false,
                ..config::DashboardConfig::default()
            },
            ..Config::default()
        };
        let mock = MockHttpClient::new();
        let http: Arc<dyn io::HttpClient> = Arc::new(mock);

        let sentinel = SentinelBuilder::new(config)
            .with_http_client(http)
            .build()
            .await
            .unwrap();

        assert!(!sentinel.has_dashboard());
    }
}
