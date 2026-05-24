#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! QHY Q-Focuser Driver
//!
//! ASCOM Alpaca driver for the QHY Q-Focuser (EAF).
//!
//! Exposes one ASCOM Focuser device over a shared serial transport
//! managed by `rusty_photon_shared_transport::SharedTransport`. The
//! per-service lifecycle scaffolding (refcount, slot, polling task,
//! command-lock arbitration) lives in the shared crate; this service
//! contributes the [`QhyCodec`], the [`FocuserManager`] hooks, and the
//! ASCOM trait implementation.

pub mod codec;
pub mod config;
pub mod error;
pub mod focuser_device;
pub mod manager;
// Compiled into the binary under `--features mock` (BDD + ConformU) and
// also into the lib's `cargo test` build so each module's `#[cfg(test)]`
// suite can drive the same canonical mock simulator. Production builds
// don't compile it.
#[cfg(any(feature = "mock", test))]
pub mod mock;
pub mod protocol;
pub mod serial;

pub use codec::{QhyCodec, QhyCodecError, QhyResponse};
pub use config::{load_config, Config, FocuserConfig, SerialConfig, ServerConfig};
pub use error::{QhyFocuserError, Result};
pub use focuser_device::QhyFocuserDevice;
pub use manager::{CachedState, FocuserManager};
pub use serial::QhyTransportFactory;

#[cfg(feature = "mock")]
pub use mock::MockQhyTransportFactory;

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use ascom_alpaca::api::CargoServerInfo;
use ascom_alpaca::Server;
use rp_tls::config::TlsConfig;
use rusty_photon_shared_transport::TransportFactory;
use tracing::{debug, info};

/// Builder for the ASCOM Alpaca server.
///
/// Configures the focuser device and transport factory, then binds the
/// server. The returned [`BoundServer`] can be inspected (e.g.
/// `listen_addr()`) before calling `start()`.
#[derive(Default)]
pub struct ServerBuilder {
    config: Config,
    factory: Option<Arc<dyn TransportFactory>>,
}

impl ServerBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(mut self, config: Config) -> Self {
        self.config = config;
        self
    }

    pub fn with_factory(mut self, factory: Arc<dyn TransportFactory>) -> Self {
        self.factory = Some(factory);
        self
    }

    pub async fn build(self) -> std::result::Result<BoundServer, Box<dyn std::error::Error>> {
        // Default to the real-hardware factory when none was supplied.
        let factory: Arc<dyn TransportFactory> = self.factory.unwrap_or_else(|| {
            Arc::new(QhyTransportFactory::new(
                self.config.serial.port.clone(),
                self.config.serial.baud_rate,
                self.config.serial.timeout,
            ))
        });

        let manager = FocuserManager::new(self.config.clone(), factory);

        // Eager hardware validation at startup: opens the port,
        // runs the handshake, and spawns the reconnect supervisor
        // before binding the HTTP listener. Handshake failures
        // bubble up to `main` for a non-zero exit.
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

            if self.config.focuser.enabled {
                let focuser_device =
                    QhyFocuserDevice::new(self.config.focuser.clone(), Arc::clone(&manager));
                server.devices.register(focuser_device);
                info!("Registered Focuser device: {}", self.config.focuser.name);
            }

            info!("Serial port: {}", self.config.serial.port);

            let tls = self.config.server.tls.clone();
            let router = axum::Router::new().fallback_service(server.into_service());

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

            // This println is parsed by tests to discover the bound port. It
            // must go to stdout (not tracing/stderr) so the subprocess output
            // can be read.
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

/// A fully bound qhy-focuser server ready to accept connections.
pub struct BoundServer {
    listener: tokio::net::TcpListener,
    router: axum::Router,
    local_addr: SocketAddr,
    tls: Option<TlsConfig>,
    /// Held so `start()` can call `manager.transport().shutdown()` after
    /// the HTTP server stops. No-op in LazyAcquire mode.
    manager: Arc<FocuserManager>,
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
                info!("qhy-focuser started on {} (TLS)", self.local_addr);
                rp_tls::server::serve_tls(self.listener, self.router, tls_config, shutdown).await
            }
            None => {
                info!("qhy-focuser started on {}", self.local_addr);
                rp_tls::server::serve_plain(self.listener, self.router, shutdown).await
            }
        };
        if let Err(e) = self.manager.transport().shutdown().await {
            tracing::warn!(error = %e, "transport shutdown returned an error during teardown");
        }
        debug!("qhy-focuser shut down");
        serve_result.map_err(Into::into)
    }
}
