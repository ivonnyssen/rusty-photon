#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! # zwo-camera — ASCOM Alpaca driver for ZWO ASI cameras (and EFW filter wheels)
//!
//! The service enumerates every connected ASI camera via [`zwo_rs`] and registers
//! each as an ASCOM [`ZwoCamera`] device on one port, with a serial-derived
//! `UniqueID`. Each device implements the full `Device` + `Camera` surface
//! (exposure state machine, ROI/binning, gain/offset, cooling, readout, ST4
//! pulse-guiding) over the [`backend::CameraHandle`] SDK seam. The EFW
//! `FilterWheel` is Phase F.
//!
//! See `docs/services/zwo-camera.md` for the full design and
//! `docs/plans/zwo-driver.md` for the decision record.
//!
//! ## Native dependency
//!
//! `zwo-rs`'s `libzwo-sys` links the ZWO ASI/EFW SDK **unconditionally**, so this
//! package must be compiled on a machine with the SDK installed — even with the
//! `simulation` feature, which removes the *camera*, not the *link*.

pub mod backend;
mod camera;
mod config;
mod config_actions;
mod error;

pub use camera::ZwoCamera;
pub use config::{load_effective_config, CliOverrides, Config, DeviceOverride, DEFAULT_PORT};
pub use config_actions::ZwoCameraDriver;
pub use error::ZwoCameraError;

use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use ascom_alpaca::api::{CargoServerInfo, Device};
use ascom_alpaca::Server;
use rusty_photon_service_lifecycle::ReloadSignal;
use tokio::net::TcpListener;
use tracing::{debug, info, warn};
use zwo_rs::CameraInfo;

use crate::backend::{CameraHandle, ZwoCameraHandle};

/// One camera discovered at enumeration: its index, [`CameraInfo`], and the
/// serial-derived ASCOM `UniqueID`.
struct EnumeratedCamera {
    index: usize,
    info: CameraInfo,
    unique_id: String,
}

/// Builds a bound zwo-camera server from an effective [`Config`].
#[derive(Default)]
pub struct ServerBuilder {
    config: Config,
    config_path: Option<PathBuf>,
    overrides: CliOverrides,
    reload: Option<ReloadSignal>,
    /// Register no cameras (the test-only zero-camera startup path, C0).
    force_empty: bool,
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

    /// Set the config source (persist path + CLI overrides) for the config
    /// actions. Together with [`Self::with_reload_signal`], enables editing.
    #[must_use]
    pub fn with_config_source(mut self, path: PathBuf, overrides: CliOverrides) -> Self {
        self.config_path = Some(path);
        self.overrides = overrides;
        self
    }

    /// Provide the reload trigger `config.apply` fires after its response flushes.
    #[must_use]
    pub fn with_reload_signal(mut self, reload: ReloadSignal) -> Self {
        self.reload = Some(reload);
        self
    }

    /// Register no cameras regardless of what the SDK reports — the test-only
    /// empty-backend path exercising the zero-camera startup (contract C0).
    #[must_use]
    pub fn with_empty(mut self, empty: bool) -> Self {
        self.force_empty = empty;
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

        let cameras = if self.force_empty {
            Vec::new()
        } else {
            enumerate_cameras().await?
        };
        if cameras.is_empty() {
            warn!("no ASI cameras discovered; starting with no Camera devices");
        }

        let mut server = Server::new(CargoServerInfo!());
        for cam in cameras.iter() {
            let handle: Arc<dyn CameraHandle> = Arc::new(ZwoCameraHandle::new(
                zwo_rs::Sdk::new()?,
                cam.index,
                cam.info.clone(),
                cam.unique_id.clone(),
            ));
            let mut device = ZwoCamera::new(handle, self.config.devices.get(&cam.unique_id));
            if let (Some(path), Some(reload)) = (self.config_path.clone(), self.reload.clone()) {
                device = device.with_config_actions(rusty_photon_driver::ConfigActionCtx {
                    effective: self.config.clone(),
                    path,
                    overrides: self.overrides.clone(),
                    reload,
                });
            }
            debug!(device = cam.index, name = %device.static_name(), "registering ASI camera");
            server.devices.register(device);
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
        // Stdout is reserved for the machine-readable `bound_addr=<host>:<port>`
        // handshake that `bdd-infra::parse_bound_port` waits on for port
        // discovery (see rusty-photon-service-lifecycle's logging docs); every
        // peer service emits it. Logs go to stderr, so this stays parseable.
        println!("Bound Alpaca server bound_addr={local_addr}");
        info!(cameras = cameras.len(), address = %local_addr, "Service started successfully");
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

/// Enumerate connected ASI cameras on the blocking thread pool, minting each
/// device's serial-derived `UniqueID`.
///
/// `ASIGetSerialNumber` requires an *open* camera, so each is opened briefly to
/// read its serial and then closed (the per-device connect handshake happens
/// later on `set_connected(true)`). The ASI SDK is blocking C FFI, so every SDK
/// call funnels through [`tokio::task::spawn_blocking`] (design doc "Concurrency").
async fn enumerate_cameras() -> Result<Vec<EnumeratedCamera>, ZwoCameraError> {
    let cameras =
        tokio::task::spawn_blocking(|| -> Result<Vec<EnumeratedCamera>, zwo_rs::Error> {
            let sdk = zwo_rs::Sdk::new()?;
            let infos = sdk.cameras()?;
            let mut out = Vec::with_capacity(infos.len());
            for (index, info) in infos.into_iter().enumerate() {
                // Open briefly to read the stable serial, then close (the camera
                // drops at the end of the block → `ASICloseCamera`).
                let serial = {
                    let camera = sdk.open_camera(index)?;
                    camera.serial()?
                };
                let unique_id = format!("ZWO:{}:{}", info.name.replace(' ', "-"), serial);
                out.push(EnumeratedCamera {
                    index,
                    info,
                    unique_id,
                });
            }
            Ok(out)
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

    /// End-to-end proof against the `zwo-rs` simulation backend: the builder
    /// enumerates the one simulated camera, registers it, and binds an ephemeral
    /// port.
    #[tokio::test]
    async fn builds_and_binds_against_the_simulation_backend() {
        let config: Config = serde_json::from_str(r#"{"server":{"port":0}}"#).unwrap();
        let bound = ServerBuilder::new()
            .with_config(config)
            .build()
            .await
            .unwrap();
        assert_ne!(bound.local_addr().port(), 0);
    }

    /// The empty-backend path starts healthy with no Camera devices (C0).
    #[tokio::test]
    async fn empty_backend_binds_with_no_cameras() {
        let config: Config = serde_json::from_str(r#"{"server":{"port":0}}"#).unwrap();
        let bound = ServerBuilder::new()
            .with_config(config)
            .with_empty(true)
            .build()
            .await
            .unwrap();
        assert_ne!(bound.local_addr().port(), 0);
    }
}
