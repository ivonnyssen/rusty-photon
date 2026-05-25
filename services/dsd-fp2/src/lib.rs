#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! Deep Sky Dad FP2 driver — ASCOM Alpaca CoverCalibrator.
//!
//! Wraps the FP2's bracketed ASCII serial protocol behind the workspace's
//! `rusty-photon-shared-transport` lifecycle scaffolding.

pub mod codec;
pub mod config;
pub mod device;
pub mod error;
pub mod manager;
#[cfg(feature = "mock")]
pub mod mock;
pub mod protocol;
pub mod transport;

pub use codec::Fp2Codec;
pub use config::{load_config, Config, CoverCalibratorConfig, SerialConfig, ServerConfig};
pub use device::DsdFp2Device;
pub use error::{DsdFp2Error, Result};
pub use manager::{CachedState, FlatPanelManager};
pub use transport::Fp2SerialTransportFactory;

#[cfg(feature = "mock")]
pub use mock::{MockState, MockTransportFactory};

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use ascom_alpaca::api::CargoServerInfo;
use ascom_alpaca::Server;
use rp_tls::config::TlsConfig;
use rusty_photon_shared_transport::TransportFactory;
use tracing::{debug, info};

/// Builder for the FP2 ASCOM Alpaca server.
pub struct ServerBuilder {
    config: Config,
    factory: Arc<dyn TransportFactory>,
}

fn default_factory(config: &Config) -> Arc<dyn TransportFactory> {
    let concrete = Fp2SerialTransportFactory::new(
        config.serial.port.clone(),
        config.serial.baud_rate,
        config.serial.timeout,
    );
    let factory: Arc<dyn TransportFactory> = Arc::new(concrete);
    factory
}

impl Default for ServerBuilder {
    fn default() -> Self {
        let config = Config::default();
        let factory = default_factory(&config);
        Self { config, factory }
    }
}

impl ServerBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(mut self, config: Config) -> Self {
        self.factory = default_factory(&config);
        self.config = config;
        self
    }

    /// Override the transport factory (used by mock-mode startup).
    pub fn with_factory(mut self, factory: Arc<dyn TransportFactory>) -> Self {
        self.factory = factory;
        self
    }

    pub async fn build(self) -> std::result::Result<BoundServer, Box<dyn std::error::Error>> {
        let manager = FlatPanelManager::new(self.config.clone(), self.factory);

        // Eager hardware validation at startup: opens the port,
        // runs the handshake (identity probe), and spawns the
        // reconnect supervisor before binding the HTTP listener. On
        // handshake failure the error bubbles up to `main` for a
        // non-zero exit, so systemd / orchestration treats startup
        // as failed rather than the service advertising a broken
        // device on the network.
        info!("validating hardware via eager startup handshake");
        manager.transport().start().await?;

        // All post-start work is fallible (bind / local_addr in
        // particular). Wrap it so a failure runs `transport.shutdown()`
        // before propagating; otherwise the reconnect supervisor task
        // would outlive the dropped manager and keep the port open
        // until process exit.
        let build_result: std::result::Result<BoundServer, Box<dyn std::error::Error>> = async {
            let mut server = Server::new(CargoServerInfo!());
            server.listen_addr = SocketAddr::from(([0, 0, 0, 0], self.config.server.port));
            server.discovery_port = self.config.server.discovery_port;

            if self.config.cover_calibrator.enabled {
                let device =
                    DsdFp2Device::new(self.config.cover_calibrator.clone(), Arc::clone(&manager));
                server.devices.register(device);
                info!(
                    "Registered CoverCalibrator device: {}",
                    self.config.cover_calibrator.name
                );
            }

            info!("Serial port: {}", self.config.serial.port);

            let tls = self.config.server.tls.clone();
            let router = axum::Router::new().fallback_service(server.into_service());

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
                manager: Arc::clone(&manager),
            })
        }
        .await;

        match build_result {
            Ok(bound) => Ok(bound),
            Err(e) => {
                if let Err(shutdown_err) = manager.transport().shutdown().await {
                    tracing::warn!(
                        error = %shutdown_err,
                        "transport shutdown failed during build() error rollback"
                    );
                }
                Err(e)
            }
        }
    }
}

/// Fully bound FP2 server ready to accept connections.
pub struct BoundServer {
    listener: tokio::net::TcpListener,
    router: axum::Router,
    local_addr: SocketAddr,
    tls: Option<TlsConfig>,
    /// Held so `start()` can call `manager.transport().shutdown()` after
    /// the HTTP server stops. No-op in LazyAcquire mode.
    manager: Arc<FlatPanelManager>,
}

impl BoundServer {
    pub fn listen_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn start(
        self,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        // Capture the serve result so transport.shutdown() runs even
        // when the HTTP server errors out — otherwise the supervisor
        // and port would leak past a serve failure.
        let serve_result = match self.tls {
            Some(ref tls_config) => {
                info!("dsd-fp2 started on {} (TLS)", self.local_addr);
                rp_tls::server::serve_tls(self.listener, self.router, tls_config, shutdown).await
            }
            None => {
                info!("dsd-fp2 started on {}", self.local_addr);
                rp_tls::server::serve_plain(self.listener, self.router, shutdown).await
            }
        };
        if let Err(e) = self.manager.transport().shutdown().await {
            tracing::warn!(error = %e, "transport shutdown returned an error during teardown");
        }
        debug!("dsd-fp2 shut down");
        serve_result.map_err(Into::into)
    }
}
