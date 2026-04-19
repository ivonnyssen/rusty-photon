#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
pub mod config;
pub mod equipment;
pub mod error;
pub mod events;
pub mod hash_password_cmd;
pub mod imaging;
pub mod mcp;
pub mod routes;
pub mod session;
pub mod tls_cmd;

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::signal;
use tracing::{debug, info};

use rp_tls::config::TlsConfig;

use crate::config::Config;
use crate::equipment::EquipmentRegistry;
use crate::error::Result;
use crate::events::EventBus;
use crate::mcp::McpHandler;
use crate::routes::{build_router, AppState};
use crate::session::{SessionConfig, SessionManager};

/// Builder for the rp server.
///
/// Configures equipment, event bus, session manager, and MCP handler,
/// then binds the server. The returned [`BoundServer`] can be inspected
/// (e.g. `listen_addr()`) before calling `start()`.
pub struct ServerBuilder {
    config: Option<Config>,
}

impl ServerBuilder {
    pub fn new() -> Self {
        Self { config: None }
    }

    pub fn with_config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    pub async fn build(self) -> Result<BoundServer> {
        let config = self.config.expect("config is required");
        let bind_addr = format!("{}:{}", config.server.bind_address, config.server.port);

        debug!("initializing equipment registry");
        let equipment = Arc::new(EquipmentRegistry::new(&config.equipment).await);

        debug!("initializing event bus");
        let event_bus = Arc::new(EventBus::from_config(&config.plugins));

        debug!("initializing session manager");
        let session = Arc::new(SessionManager::new(event_bus.clone(), &config.plugins));

        let session_config = SessionConfig {
            data_directory: config.session.data_directory.clone(),
        };

        let mcp = McpHandler::new(equipment.clone(), event_bus.clone(), session_config);

        let state = AppState {
            equipment,
            mcp,
            session: session.clone(),
        };

        let router = build_router(state);

        // Layer authentication if configured
        let router = match &config.server.auth {
            Some(auth) => {
                if config.server.tls.is_none() {
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

        let tls = config.server.tls.clone();

        let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
        let local_addr = listener.local_addr()?;

        // Set the MCP base URL on the session manager
        let scheme = if tls.is_some() { "https" } else { "http" };
        let base_url = format!("{scheme}://{local_addr}");
        session.set_mcp_base_url(base_url).await;

        // This println is parsed by BDD tests to discover the bound port.
        // It must go to stdout (not tracing/stderr) so the subprocess output can be read.
        println!("Bound rp server bound_addr={}", local_addr);
        info!("rp service bound on {}", local_addr);

        Ok(BoundServer {
            listener,
            router,
            local_addr,
            tls,
        })
    }
}

impl Default for ServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// A fully bound rp server ready to accept connections.
pub struct BoundServer {
    listener: tokio::net::TcpListener,
    router: axum::Router,
    local_addr: SocketAddr,
    tls: Option<TlsConfig>,
}

impl BoundServer {
    pub fn listen_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn start(self) -> Result<()> {
        self.start_with_shutdown(shutdown_signal()).await
    }

    /// Run the server until the `shutdown` future resolves, at which point axum's
    /// graceful-shutdown path drains in-flight connections before returning.
    ///
    /// `run_server_loop` uses this to drive shutdown externally on reload/stop,
    /// so the outer loop can `await` the in-flight future instead of dropping it
    /// and rebinding the same port while requests are still resolving.
    pub async fn start_with_shutdown<F>(self, shutdown: F) -> Result<()>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        match self.tls {
            Some(ref tls_config) => {
                info!("rp service started on {} (TLS)", self.local_addr);
                rp_tls::server::serve_tls(self.listener, self.router, tls_config, shutdown)
                    .await
                    .map_err(|e| crate::error::RpError::Server(e.to_string()))?;
            }
            None => {
                info!("rp service started on {}", self.local_addr);
                axum::serve(self.listener, self.router)
                    .with_graceful_shutdown(shutdown)
                    .await?;
            }
        }

        debug!("rp service shut down");
        Ok(())
    }
}

/// Run the server in a loop, restarting on reload signal and exiting on stop.
///
/// On each iteration the config file is re-read, the server rebuilt, and a fresh
/// listener bound. The `stop` and `reload` closures return futures that complete
/// when the respective signal is received (e.g., SIGTERM for stop, SIGHUP for reload).
///
/// When `stop` or `reload` fires, a oneshot triggers axum's graceful shutdown on
/// the in-flight server and the loop awaits that future to completion before
/// rebinding the same port. Dropping the running future instead would leave
/// in-flight connections unresolved and race the new iteration's listener bind.
pub async fn run_server_loop(
    config_path: &std::path::Path,
    mut stop: impl FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>,
    mut reload: impl FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    loop {
        // See #83 for the longer-term refactor to let `load_config` accept `&Path`.
        let config_str = config_path.to_str().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("configuration path is not valid UTF-8: {:?}", config_path),
            )
        })?;
        let config = config::load_config(config_str)?;
        tracing::info!("Starting rp server on port {}", config.server.port);
        let server = ServerBuilder::new().with_config(config).build().await?;
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let shutdown_fut = async move {
            let _ = shutdown_rx.await;
        };
        let mut start_fut = Box::pin(server.start_with_shutdown(shutdown_fut));
        tokio::select! {
            result = &mut start_fut => return result.map_err(Into::into),
            _ = stop() => {
                tracing::info!("Received stop signal");
                let _ = shutdown_tx.send(());
                if let Err(err) = start_fut.await {
                    tracing::warn!("rp shutdown returned error: {err}");
                }
                break;
            }
            _ = reload() => {
                tracing::info!("Reloading configuration");
                let _ = shutdown_tx.send(());
                if let Err(err) = start_fut.await {
                    tracing::warn!("rp shutdown returned error: {err}");
                }
                continue;
            }
        }
    }
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => debug!("received Ctrl+C"),
        () = terminate => debug!("received SIGTERM"),
    }
}
