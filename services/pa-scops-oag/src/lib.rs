#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! Pegasus Astro Scops OAG Driver
//!
//! ASCOM Alpaca Focuser driver for the Pegasus Astro Scops OAG (motorized
//! off-axis guider focuser).
//!
//! Exposes one ASCOM Focuser device over a shared serial transport managed by
//! `rusty_photon_shared_transport::SharedTransport`. The per-service lifecycle
//! scaffolding (refcount, slot, polling task, command-lock arbitration) lives in
//! the shared crate; this service contributes the [`ScopsCodec`], the
//! [`FocuserManager`] hooks, and the ASCOM trait implementation.

pub mod codec;
pub mod config;
pub mod config_actions;
pub mod error;
pub mod focuser_device;
pub mod manager;
// Compiled into the binary under `--features mock` (BDD + ConformU) and also
// into the lib's `cargo test` build so each module's `#[cfg(test)]` suite can
// drive the same canonical mock simulator. Production builds don't compile it.
#[cfg(any(feature = "mock", test))]
pub mod mock;
pub mod protocol;
pub mod serial;

pub use codec::{ScopsCodec, ScopsCodecError, ScopsResponse};
pub use config::{load_config, Config, FocuserConfig, SerialConfig, ServerConfig};
pub use error::{Result, ScopsOagError};
pub use focuser_device::ScopsFocuserDevice;
pub use manager::{CachedState, FocuserManager};
pub use serial::ScopsTransportFactory;

#[cfg(feature = "mock")]
pub use mock::MockScopsTransportFactory;

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use ascom_alpaca::api::CargoServerInfo;
use ascom_alpaca::Server;
use rp_tls::config::TlsConfig;
use rusty_photon_service_lifecycle::ReloadSignal;
use rusty_photon_shared_transport::TransportFactory;
use tracing::{debug, info};

use crate::config::CliOverrides;
use crate::config_actions::ScopsFocuserDriver;

/// Builder for the ASCOM Alpaca server.
#[derive(Default)]
pub struct ServerBuilder {
    config: Config,
    factory: Option<Arc<dyn TransportFactory>>,
    /// Where `config.apply` persists + which fields are CLI-pinned. `Some`
    /// enables the config actions on the registered device.
    config_source: Option<(PathBuf, CliOverrides)>,
    /// In-process reload trigger handed to the device's `config.apply` handler.
    reload: Option<ReloadSignal>,
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

    /// Wire the config-action source (persist path + CLI overrides) so the
    /// registered focuser device advertises `config.get` / `config.apply` /
    /// `config.schema`.
    pub fn with_config_source(mut self, path: PathBuf, overrides: CliOverrides) -> Self {
        self.config_source = Some((path, overrides));
        self
    }

    /// Hand the device the in-process reload trigger fired after a `config.apply`
    /// that needs a reload.
    pub fn with_reload_signal(mut self, reload: ReloadSignal) -> Self {
        self.reload = Some(reload);
        self
    }

    pub async fn build(self) -> std::result::Result<BoundServer, Box<dyn std::error::Error>> {
        // Default to the real-hardware factory when none was supplied.
        let factory: Arc<dyn TransportFactory> = self.factory.unwrap_or_else(|| {
            Arc::new(ScopsTransportFactory::new(
                self.config.serial.port.clone(),
                self.config.serial.baud_rate,
                self.config.serial.timeout,
            ))
        });

        let manager = FocuserManager::new(self.config.clone(), factory);

        // Eager hardware validation at startup: open the port, run the
        // handshake, and spawn the reconnect supervisor before binding the HTTP
        // listener. Handshake failures bubble up to `main` for a non-zero exit.
        info!("validating hardware via eager startup handshake");
        manager.transport().start().await?;

        // All post-start work is fallible (bind / local_addr in particular).
        // Wrap it so a failure runs `transport.shutdown()` before propagating;
        // otherwise the reconnect supervisor task would outlive the dropped
        // manager and keep the port open until process exit.
        let build_result: std::result::Result<BoundServer, Box<dyn std::error::Error>> = async {
            let mut server = Server::new(CargoServerInfo!());
            server.listen_addr = SocketAddr::from(([0, 0, 0, 0], self.config.server.port));
            server.discovery_port = self.config.server.discovery_port;

            if self.config.focuser.enabled {
                let mut focuser_device =
                    ScopsFocuserDevice::new(self.config.focuser.clone(), Arc::clone(&manager));
                let config_ctx: Option<rusty_photon_driver::ConfigActionCtx<ScopsFocuserDriver>> =
                    match (self.config_source.clone(), self.reload.clone()) {
                        (Some((path, overrides)), Some(reload)) => {
                            Some(rusty_photon_driver::ConfigActionCtx {
                                effective: self.config.clone(),
                                path,
                                overrides,
                                reload,
                            })
                        }
                        _ => None,
                    };
                if let Some(ctx) = config_ctx {
                    focuser_device = focuser_device.with_config_actions(ctx);
                }
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

/// A fully bound pa-scops-oag server ready to accept connections.
pub struct BoundServer {
    listener: tokio::net::TcpListener,
    router: axum::Router,
    local_addr: SocketAddr,
    tls: Option<TlsConfig>,
    /// Held so `start()` can call `manager.transport().shutdown()` after the
    /// HTTP server stops.
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
        // Capture the serve result so transport.shutdown() runs even when the
        // HTTP server errors out — otherwise the supervisor and port would leak.
        let serve_result = match self.tls {
            Some(ref tls_config) => {
                info!("pa-scops-oag started on {} (TLS)", self.local_addr);
                rp_tls::server::serve_tls(self.listener, self.router, tls_config, shutdown).await
            }
            None => {
                info!("pa-scops-oag started on {}", self.local_addr);
                rp_tls::server::serve_plain(self.listener, self.router, shutdown).await
            }
        };
        if let Err(e) = self.manager.transport().shutdown().await {
            tracing::warn!(error = %e, "transport shutdown returned an error during teardown");
        }
        debug!("pa-scops-oag shut down");
        serve_result.map_err(Into::into)
    }
}
