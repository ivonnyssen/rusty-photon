#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! PPBA Driver
//!
//! ASCOM Alpaca driver for the Pegasus Astro Pocket Powerbox Advance Gen2 (PPBA).
//!
//! This driver exposes two ASCOM devices:
//! - Switch device for power control and sensor monitoring
//! - ObservingConditions device for environmental sensors

pub mod config;
pub mod error;
pub mod io;
pub mod mean;
#[cfg(feature = "mock")]
pub mod mock;
pub mod observingconditions_device;
pub mod protocol;
pub mod serial;
pub mod serial_manager;
pub mod switch_device;
pub mod switches;

pub use config::{
    load_config, Config, DeviceConfig, ObservingConditionsConfig, SerialConfig, ServerConfig,
    SwitchConfig,
};
pub use error::{PpbaError, Result};
pub use io::SerialPortFactory;
pub use observingconditions_device::PpbaObservingConditionsDevice;
pub use serial_manager::SerialManager;
pub use switch_device::PpbaSwitchDevice;
pub use switches::{SwitchId, SwitchInfo, MAX_SWITCH};

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
/// Configures devices and serial port factory, then binds the server.
/// The returned [`BoundServer`] can be inspected (e.g. `listen_addr()`)
/// before calling `start()`.
pub struct ServerBuilder {
    config: Config,
    factory: Arc<dyn SerialPortFactory>,
}

impl ServerBuilder {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            factory: Arc::new(TokioSerialPortFactory::new()),
        }
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

        if self.config.switch.enabled {
            let switch_device =
                PpbaSwitchDevice::new(self.config.switch.clone(), Arc::clone(&serial_manager));
            server.devices.register(switch_device);
            info!(
                "Registered Switch device: {} (device number {})",
                self.config.switch.name, self.config.switch.device_number
            );
        }

        if self.config.observingconditions.enabled {
            let oc_device = PpbaObservingConditionsDevice::new(
                self.config.observingconditions.clone(),
                Arc::clone(&serial_manager),
            );
            server.devices.register(oc_device);
            info!(
                "Registered ObservingConditions device: {} (device number {})",
                self.config.observingconditions.name, self.config.observingconditions.device_number
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

        // This println is parsed by conformu_integration tests to discover the bound port.
        // It must go to stdout (not tracing/stderr) so the subprocess output can be read.
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

/// A fully bound ppba-driver server ready to accept connections.
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
                info!("ppba-driver started on {} (TLS)", self.local_addr);
                rp_tls::server::serve_tls(self.listener, self.router, tls_config, shutdown).await?;
            }
            None => {
                info!("ppba-driver started on {}", self.local_addr);
                rp_tls::server::serve_plain(self.listener, self.router, shutdown).await?;
            }
        }
        debug!("ppba-driver shut down");
        Ok(())
    }
}

/// Run the server in a loop, restarting on reload signal and exiting on stop.
///
/// On each iteration the config file is re-read, `apply_overrides` is invoked
/// so CLI flags like `--port` / `--server-port` / `--enable-*` continue to
/// shadow the config file across reloads, the server is rebuilt, and a fresh
/// listener is bound. The `stop` and `reload` closures return futures that
/// complete when the respective signal is received (e.g., SIGTERM for stop,
/// SIGHUP for reload).
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
        info!("Starting ppba-driver server on port {}", config.server.port);
        let server = ServerBuilder::new(config)
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
                    tracing::warn!("ppba-driver shutdown returned error: {err}");
                }
                break;
            }
            _ = reload() => {
                info!("Reloading configuration");
                let _ = shutdown_tx.send(());
                if let Err(err) = start_fut.await {
                    tracing::warn!("ppba-driver shutdown returned error: {err}");
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
