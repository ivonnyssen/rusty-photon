#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! QHY Q-Focuser Driver
//!
//! ASCOM Alpaca driver for the QHY Q-Focuser (EAF).
//!
//! This driver exposes an ASCOM Focuser device for controlling
//! a QHY Q-Focuser stepper motor over USB serial.

pub mod config;
pub mod error;
pub mod focuser_device;
pub mod io;
#[cfg(feature = "mock")]
pub mod mock;
pub mod protocol;
pub mod serial;
pub mod serial_manager;

pub use config::{load_config, Config, FocuserConfig, SerialConfig, ServerConfig};
pub use error::{QhyFocuserError, Result};
pub use focuser_device::QhyFocuserDevice;
pub use io::SerialPortFactory;
pub use serial_manager::SerialManager;

#[cfg(feature = "mock")]
pub use mock::MockSerialPortFactory;

use std::net::SocketAddr;
use std::sync::Arc;

use ascom_alpaca::api::CargoServerInfo;
use ascom_alpaca::Server;
use rp_tls::config::TlsConfig;
use serial::TokioSerialPortFactory;
use tokio::signal;
use tracing::{debug, info};

/// Builder for the ASCOM Alpaca server.
///
/// Configures the focuser device and serial port factory, then binds the server.
/// The returned [`BoundServer`] can be inspected (e.g. `listen_addr()`)
/// before calling `start()`.
pub struct ServerBuilder {
    config: Config,
    factory: Arc<dyn SerialPortFactory>,
}

impl Default for ServerBuilder {
    fn default() -> Self {
        Self {
            config: Config::default(),
            factory: Arc::new(TokioSerialPortFactory::new()),
        }
    }
}

impl ServerBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(mut self, config: Config) -> Self {
        self.config = config;
        self
    }

    pub fn with_factory(mut self, factory: Arc<dyn SerialPortFactory>) -> Self {
        self.factory = factory;
        self
    }

    pub async fn build(self) -> std::result::Result<BoundServer, Box<dyn std::error::Error>> {
        let mut server = Server::new(CargoServerInfo!());
        server.listen_addr = SocketAddr::from(([0, 0, 0, 0], self.config.server.port));
        server.discovery_port = self.config.server.discovery_port;

        let serial_manager = Arc::new(SerialManager::new(self.config.clone(), self.factory));

        if self.config.focuser.enabled {
            let focuser_device =
                QhyFocuserDevice::new(self.config.focuser.clone(), Arc::clone(&serial_manager));
            server.devices.register(focuser_device);
            info!(
                "Registered Focuser device: {} (device number {})",
                self.config.focuser.name, self.config.focuser.device_number
            );
        }

        info!("Serial port: {}", self.config.serial.port);

        let tls = self.config.server.tls.clone();
        let router = server.into_router();

        // Layer authentication if configured
        let router = match &self.config.server.auth {
            Some(auth) => {
                if self.config.server.tls.is_none() {
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

        let listener = rp_tls::server::bind_dual_stack_tokio(SocketAddr::from((
            [0, 0, 0, 0],
            self.config.server.port,
        )))
        .await?;
        let local_addr = listener.local_addr()?;

        println!("Bound Alpaca server bound_addr={}", local_addr);
        info!("Bound Alpaca server bound_addr={}", local_addr);

        Ok(BoundServer {
            listener,
            router,
            local_addr,
            tls,
        })
    }
}

/// A fully bound qhy-focuser server ready to accept connections.
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

    pub async fn start(self) -> std::result::Result<(), Box<dyn std::error::Error>> {
        self.start_with_shutdown(shutdown_signal()).await
    }

    /// Run the server until the `shutdown` future resolves, at which point
    /// graceful shutdown drains in-flight connections before returning.
    ///
    /// `run_server_loop` uses this to drive shutdown externally on reload/stop
    /// so the outer loop can await the in-flight future instead of dropping it
    /// and rebinding the same port while requests are still resolving.
    pub async fn start_with_shutdown<F>(
        self,
        shutdown: F,
    ) -> std::result::Result<(), Box<dyn std::error::Error>>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        match self.tls {
            Some(ref tls_config) => {
                info!("qhy-focuser started on {} (TLS)", self.local_addr);
                rp_tls::server::serve_tls(self.listener, self.router, tls_config, shutdown).await?;
            }
            None => {
                info!("qhy-focuser started on {}", self.local_addr);
                rp_tls::server::serve_plain(self.listener, self.router, shutdown).await?;
            }
        }
        debug!("qhy-focuser shut down");
        Ok(())
    }
}

/// Run the server in a loop, restarting on reload signal and exiting on stop.
///
/// On each iteration the config file is re-read, `apply_overrides` is invoked
/// so CLI flags like `--port` / `--server-port` continue to shadow the config
/// file across reloads, the server is rebuilt, and a fresh listener is bound.
/// The `stop` and `reload` closures return futures that complete when the
/// respective signal is received (e.g., SIGTERM for stop, SIGHUP for reload).
///
/// When `stop` or `reload` fires, a oneshot triggers graceful shutdown on the
/// in-flight server and the loop awaits that future to completion before
/// rebinding the same port. Dropping the running future instead would leave
/// in-flight connections unresolved and race the new iteration's listener bind.
pub async fn run_server_loop(
    config_path: &std::path::Path,
    factory: Arc<dyn SerialPortFactory>,
    mut apply_overrides: impl FnMut(&mut Config),
    mut stop: impl FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>,
    mut reload: impl FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    loop {
        let mut config = load_config(&config_path.to_path_buf())?;
        apply_overrides(&mut config);
        info!("Starting qhy-focuser server on port {}", config.server.port);
        let server = ServerBuilder::new()
            .with_config(config)
            .with_factory(factory.clone())
            .build()
            .await?;
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let shutdown_fut = async move {
            let _ = shutdown_rx.await;
        };
        let mut start_fut = Box::pin(server.start_with_shutdown(shutdown_fut));
        tokio::select! {
            result = &mut start_fut => return result,
            _ = stop() => {
                info!("Received stop signal");
                let _ = shutdown_tx.send(());
                if let Err(err) = start_fut.await {
                    tracing::warn!("qhy-focuser shutdown returned error: {err}");
                }
                break;
            }
            _ = reload() => {
                info!("Reloading configuration");
                let _ = shutdown_tx.send(());
                if let Err(err) = start_fut.await {
                    tracing::warn!("qhy-focuser shutdown returned error: {err}");
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
