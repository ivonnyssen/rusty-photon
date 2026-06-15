//! # zwo-camera — ASCOM Alpaca driver for ZWO ASI cameras (and EFW filter wheels)
//!
//! **Phase C (Track A) scaffold.** This crate currently stands up a *bare*
//! Alpaca server on a fixed port that enumerates every connected ASI camera via
//! [`zwo_rs`] and registers each as a minimal [`ZwoCamera`] device (identity +
//! cached geometry). Its purpose is to prove the build/link chain
//! `zwo-camera → zwo-rs → libzwo-sys → ` the native ZWO SDK, and the CI / Bazel
//! gating around that native dependency, *before* the full device-trait work
//! (Phase E: exposure state machine, ROI/bin, gain/offset, cooling, pulse
//! guiding) and the EFW `FilterWheel` (Phase F).
//!
//! See [`docs/services/zwo-camera.md`](https://github.com/ivonnyssen/rusty_photon)
//! for the full design and [`docs/plans/zwo-driver.md`] for the decision record.
//!
//! ## Native dependency
//!
//! `zwo-rs`'s `libzwo-sys` links the ZWO ASI/EFW SDK **unconditionally**, so
//! this package must be compiled on a machine with the SDK installed — even with
//! the `simulation` feature, which removes the *camera*, not the *link*.

mod camera;
mod config;
mod error;

pub use camera::ZwoCamera;
pub use config::{load_effective_config, CliOverrides, Config, DEFAULT_PORT};
pub use error::ZwoCameraError;

use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};

use ascom_alpaca::api::{CargoServerInfo, Device};
use ascom_alpaca::Server;
use tokio::net::TcpListener;
use tracing::{debug, info, warn};
use zwo_rs::CameraInfo;

/// Builds a bound zwo-camera server from an effective [`Config`].
#[derive(Debug, Default)]
pub struct ServerBuilder {
    config: Config,
}

impl ServerBuilder {
    /// Create a builder with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the effective configuration to serve.
    #[must_use]
    pub fn with_config(mut self, config: Config) -> Self {
        self.config = config;
        self
    }

    /// Enumerate the connected ASI cameras, register each as an ASCOM device,
    /// and bind the Alpaca listener.
    ///
    /// Zero discovered cameras is **not** a hard failure: the server starts with
    /// no Camera devices and logs a warning (a later reload re-enumerates).
    ///
    /// # Errors
    /// Returns [`ZwoCameraError`] when SDK enumeration fails or the listener
    /// cannot bind the configured port.
    pub async fn build(self) -> Result<BoundServer, ZwoCameraError> {
        let port = self.config.server.port;
        let cameras = enumerate_cameras().await?;
        if cameras.is_empty() {
            warn!("no ASI cameras discovered; starting with no Camera devices");
        }

        let mut server = Server::new(CargoServerInfo!());
        // The Alpaca discovery responder is bound separately by the crate; we
        // serve the HTTP service directly (below), so it is never started.
        for (index, info) in cameras.into_iter().enumerate() {
            let camera = ZwoCamera::new(index, info);
            debug!(
                device = index,
                name = %camera.static_name(),
                "registering ASI camera as ASCOM device"
            );
            server.devices.register(camera);
        }

        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))
            .await
            .map_err(|source| ZwoCameraError::Bind {
                addr: format!("0.0.0.0:{port}"),
                source,
            })?;
        let local_addr = listener
            .local_addr()
            .map_err(|source| ZwoCameraError::Bind {
                addr: format!("0.0.0.0:{port}"),
                source,
            })?;

        let app = axum::Router::new().fallback_service(server.into_service());
        info!(address = %local_addr, "Service started successfully");
        Ok(BoundServer {
            listener,
            app,
            local_addr,
        })
    }
}

/// A zwo-camera server bound to a local port, ready to [`start`](Self::start).
pub struct BoundServer {
    listener: TcpListener,
    app: axum::Router,
    local_addr: SocketAddr,
}

impl BoundServer {
    /// The address the listener is bound to (useful when the port was `0`).
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Serve until `shutdown` resolves, then drain gracefully.
    ///
    /// # Errors
    /// Returns [`ZwoCameraError::Server`] if the HTTP server stops with an error.
    pub async fn start(
        self,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), ZwoCameraError> {
        axum::serve(self.listener, self.app)
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(|e| ZwoCameraError::Server(e.to_string()))?;
        Ok(())
    }
}

/// Enumerate connected ASI cameras on the blocking thread pool.
///
/// The ASI SDK is blocking C FFI, so every SDK call funnels through
/// [`tokio::task::spawn_blocking`] (see the design doc "Concurrency").
async fn enumerate_cameras() -> Result<Vec<CameraInfo>, ZwoCameraError> {
    let cameras = tokio::task::spawn_blocking(|| {
        let sdk = zwo_rs::Sdk::new()?;
        sdk.cameras()
    })
    .await??;
    debug!(count = cameras.len(), "enumerated ASI cameras");
    Ok(cameras)
}

#[cfg(all(test, feature = "simulation"))]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod simulation_tests {
    use super::*;

    /// End-to-end Track-A proof: against the `zwo-rs` simulation backend the
    /// builder enumerates the one simulated camera, registers it, and binds an
    /// ephemeral port.
    #[tokio::test]
    async fn builds_and_binds_against_the_simulation_backend() {
        // Port 0 → ephemeral, so the test never clashes with a real :11122.
        let config: Config = serde_json::from_str(r#"{"server":{"port":0}}"#).unwrap();
        let bound = ServerBuilder::new()
            .with_config(config)
            .build()
            .await
            .unwrap();
        assert_ne!(bound.local_addr().port(), 0);
    }
}
