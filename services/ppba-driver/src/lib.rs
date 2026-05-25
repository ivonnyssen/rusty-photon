#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! PPBA Driver
//!
//! ASCOM Alpaca driver for the Pegasus Astro Pocket Powerbox Advance Gen2 (PPBA).
//!
//! Exposes two ASCOM devices over one shared serial transport managed by
//! `rusty_photon_shared_transport::SharedTransport`:
//! - Switch device (power control + sensor monitoring)
//! - ObservingConditions device (environmental sensors)

pub mod codec;
pub mod config;
pub mod error;
pub mod manager;
pub mod mean;
// Compiled into the binary under `--features mock` (BDD + ConformU) and
// also into the lib's `cargo test` build so each module's `#[cfg(test)]`
// suite can drive the same canonical PPBA simulator. Production builds
// don't compile it.
#[cfg(any(feature = "mock", test))]
pub mod mock;
pub mod observingconditions_device;
pub mod protocol;
pub mod serial;
pub mod switch_device;
pub mod switches;

pub use codec::{PpbaCodec, PpbaCodecError, PpbaResponse};
pub use config::{
    load_config, Config, DeviceConfig, ObservingConditionsConfig, SerialConfig, ServerConfig,
    SwitchConfig,
};
pub use error::{PpbaError, Result};
pub use manager::{CachedState, PpbaManager};
pub use observingconditions_device::PpbaObservingConditionsDevice;
pub use serial::PpbaTransportFactory;
pub use switch_device::PpbaSwitchDevice;
pub use switches::{SwitchId, SwitchInfo, MAX_SWITCH};

#[cfg(feature = "mock")]
pub use mock::MockPpbaTransportFactory;

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use ascom_alpaca::api::CargoServerInfo;
use ascom_alpaca::Server;
use rp_tls::config::TlsConfig;
use rusty_photon_shared_transport::TransportFactory;
use tracing::{debug, info};

/// Builder for the ASCOM Alpaca server.
pub struct ServerBuilder {
    config: Config,
    factory: Arc<dyn TransportFactory>,
}

impl ServerBuilder {
    pub fn new(config: Config) -> Self {
        let factory: Arc<dyn TransportFactory> = Arc::new(PpbaTransportFactory::new(
            config.serial.port.clone(),
            config.serial.baud_rate,
            config.serial.timeout,
        ));
        Self { config, factory }
    }

    pub fn with_factory(mut self, factory: Arc<dyn TransportFactory>) -> Self {
        self.factory = factory;
        self
    }

    pub async fn build(self) -> std::result::Result<BoundServer, Box<dyn std::error::Error>> {
        let manager = PpbaManager::new(self.config.clone(), self.factory);

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

            if self.config.switch.enabled {
                let switch_device =
                    PpbaSwitchDevice::new(self.config.switch.clone(), Arc::clone(&manager));
                server.devices.register(switch_device);
                info!("Registered Switch device: {}", self.config.switch.name);
            }

            if self.config.observingconditions.enabled {
                let oc_device = PpbaObservingConditionsDevice::new(
                    self.config.observingconditions.clone(),
                    Arc::clone(&manager),
                );
                server.devices.register(oc_device);
                info!(
                    "Registered ObservingConditions device: {}",
                    self.config.observingconditions.name
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

            // This println is parsed by conformu_integration tests to discover the bound port.
            // It must go to stdout (not tracing/stderr) so the subprocess output can be read.
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

pub struct BoundServer {
    listener: tokio::net::TcpListener,
    router: axum::Router,
    local_addr: SocketAddr,
    tls: Option<TlsConfig>,
    /// Held so `start()` can call `manager.transport().shutdown()` after
    /// the HTTP server stops. No-op in LazyAcquire mode.
    manager: Arc<PpbaManager>,
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
                info!("ppba-driver started on {} (TLS)", self.local_addr);
                rp_tls::server::serve_tls(self.listener, self.router, tls_config, shutdown).await
            }
            None => {
                info!("ppba-driver started on {}", self.local_addr);
                rp_tls::server::serve_plain(self.listener, self.router, shutdown).await
            }
        };
        if let Err(e) = self.manager.transport().shutdown().await {
            tracing::warn!(error = %e, "transport shutdown returned an error during teardown");
        }
        debug!("ppba-driver shut down");
        serve_result.map_err(Into::into)
    }
}
