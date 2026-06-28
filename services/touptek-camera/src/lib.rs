#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! # touptek-camera — ASCOM Alpaca driver for ToupTek (and OEM-rebrand) cameras
//!
//! The service enumerates every connected ToupTek camera via [`touptek_rs`] and
//! registers each as an ASCOM [`TouptekCamera`] device on one port, with an
//! id-derived `UniqueID`. The same flat ToupCam ABI is shared by the OEM rebrands
//! (Altair, Omegon, Meade, Bresser, Mallincam, RisingCam/Ogma, SVBony,
//! StarShootG, Nncam, Tscam), so one driver covers the whole family.
//!
//! See `docs/plans/touptek-driver.md` for the decision record and (from Phase D)
//! `docs/services/touptek-camera.md` for the full design.
//!
//! ## Phase C scaffold
//!
//! This is the bare service: it builds, enumerates (real or simulated), registers
//! a minimal Camera, and binds `:11123`. The full `Camera` surface (exposure,
//! ROI/binning, gain/offset, cooling, RAW readout, ST4 pulse-guiding) is Phase E —
//! see [`camera`].
//!
//! ## Native dependency
//!
//! `touptek-rs`'s `libtoupcam-sys` links the ToupTek SDK on the *real* FFI path.
//! The `simulation` feature removes the *camera*, and the Bazel `_sim` chain
//! additionally skip-links the SDK (`TOUPCAM_SKIP_NATIVE_LINK`), so the simulated
//! build/test path needs no SDK provisioned. See the README and (Phase D) the
//! design doc's "Native dependency & build gating".

mod backend;
mod camera;
mod config;
mod config_actions;
mod error;

pub use camera::TouptekCamera;
pub use config::{load_effective_config, CliOverrides, Config, DeviceOverride, DEFAULT_PORT};
pub use config_actions::TouptekCameraDriver;
pub use error::TouptekCameraError;

use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use ascom_alpaca::api::{CargoServerInfo, Device};
use ascom_alpaca::Server;
use rusty_photon_service_lifecycle::ReloadSignal;
use tokio::net::TcpListener;
use touptek_rs::CameraInfo;
use tracing::{debug, info, warn};

use crate::backend::{CameraHandle, TouptekCameraHandle};

/// One camera discovered at enumeration: its index, [`CameraInfo`], the bare SDK
/// device id (`key` for `devices` config overrides), and the id-derived ASCOM
/// `UniqueID`.
struct EnumeratedCamera {
    index: usize,
    info: CameraInfo,
    key: String,
    unique_id: String,
}

/// Builds a bound touptek-camera server from an effective [`Config`].
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

    /// Enumerate the connected ToupTek cameras, register each as an ASCOM device,
    /// and bind the Alpaca listener.
    ///
    /// Zero discovered cameras is **not** a hard failure: the server starts with
    /// no Camera devices and logs a warning (a later reload re-enumerates).
    ///
    /// # Errors
    /// Returns [`TouptekCameraError`] when SDK enumeration fails or the listener
    /// cannot bind the configured port.
    pub async fn build(self) -> Result<BoundServer, TouptekCameraError> {
        let port = self.config.server.port;

        let cameras = if self.force_empty {
            Vec::new()
        } else {
            enumerate_cameras().await?
        };
        if cameras.is_empty() {
            warn!("no ToupTek cameras discovered; starting with no Camera devices");
        }

        let mut server = Server::new(CargoServerInfo!());
        for cam in cameras.iter() {
            let handle: Arc<dyn CameraHandle> = Arc::new(TouptekCameraHandle::new(
                touptek_rs::Sdk::new()?,
                cam.index,
                cam.info.clone(),
                cam.unique_id.clone(),
            ));
            // `devices` overrides are keyed by the bare SDK device id (matching the
            // config-actions `devices.{id}` paths), NOT the prefixed
            // `TOUPTEK:{name}:{id}` UniqueID.
            let mut device = TouptekCamera::new(handle, self.config.devices.get(&cam.key));
            if let (Some(path), Some(reload)) = (self.config_path.clone(), self.reload.clone()) {
                device = device.with_config_actions(rusty_photon_driver::ConfigActionCtx {
                    effective: self.config.clone(),
                    path,
                    overrides: self.overrides.clone(),
                    reload,
                });
            }
            debug!(device = cam.index, name = %device.static_name(), "registering ToupTek camera");
            server.devices.register(device);
        }

        // Shared dual-stack helper (IPv6 + IPv4) with SO_REUSEADDR, like every
        // other Alpaca service. SO_REUSEADDR matters because the in-process
        // `with_reload` loop rebinds the same port; a raw bind could fail to
        // rebind while a prior listener's TIME_WAIT lingers.
        let bind_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
        let listener = rp_tls::server::bind_dual_stack_tokio(bind_addr)
            .await
            .map_err(|source| TouptekCameraError::Bind {
                addr: bind_addr.to_string(),
                source,
            })?;
        let local_addr = listener
            .local_addr()
            .map_err(|source| TouptekCameraError::Bind {
                addr: bind_addr.to_string(),
                source: rp_tls::error::TlsError::Io(source),
            })?;

        let app = axum::Router::new().fallback_service(server.into_service());
        // Stdout is reserved for the machine-readable `bound_addr=<host>:<port>`
        // handshake that `bdd-infra::parse_bound_port` waits on for port discovery;
        // every peer service emits it. Logs go to stderr, so this stays parseable.
        println!("Bound Alpaca server bound_addr={local_addr}");
        info!(cameras = cameras.len(), address = %local_addr, "Service started successfully");
        Ok(BoundServer {
            listener,
            app,
            local_addr,
        })
    }
}

/// A touptek-camera server bound to a local port, ready to [`start`](Self::start).
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
    /// Returns [`TouptekCameraError::Server`] if the HTTP server stops with an
    /// error.
    pub async fn start(
        self,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), TouptekCameraError> {
        axum::serve(self.listener, self.app)
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(|e| TouptekCameraError::Server(e.to_string()))?;
        Ok(())
    }
}

/// Enumerate connected ToupTek cameras on the blocking thread pool, minting each
/// device's id-derived `UniqueID`.
///
/// Unlike ZWO/QHY, `Toupcam_EnumV2` returns the stable device id directly, so no
/// per-camera open is needed to read identity. The SDK is blocking C FFI, so the
/// enumeration funnels through [`tokio::task::spawn_blocking`] (design doc
/// "Concurrency").
async fn enumerate_cameras() -> Result<Vec<EnumeratedCamera>, TouptekCameraError> {
    let cameras =
        tokio::task::spawn_blocking(|| -> Result<Vec<EnumeratedCamera>, touptek_rs::Error> {
            let sdk = touptek_rs::Sdk::new()?;
            let infos = sdk.enumerate()?;
            let mut out = Vec::with_capacity(infos.len());
            for (index, info) in infos.into_iter().enumerate() {
                let (key, unique_id) = mint_identity(&info, index);
                out.push(EnumeratedCamera {
                    index,
                    info,
                    key,
                    unique_id,
                });
            }
            Ok(out)
        })
        .await??;
    debug!(count = cameras.len(), "enumerated ToupTek cameras");
    Ok(cameras)
}

/// Mint the `(key, UniqueID)` pair for an enumerated camera.
///
/// The `key` is the SDK device id (`ToupcamDeviceV2.id`) — the bus-stable handle
/// and the key for `devices` config overrides. A camera that reports no id (the
/// blank-id edge case) falls back to a position-based `noserial-{index}`: unique
/// per enumeration slot and stable across reconnects for the common single-camera
/// case. The `UniqueID` is the display name plus that key, prefixed `TOUPTEK:`.
fn mint_identity(info: &CameraInfo, index: usize) -> (String, String) {
    let key = if info.id.is_empty() {
        format!("noserial-{index}")
    } else {
        info.id.clone()
    };
    let name = if info.display_name.is_empty() {
        info.model_name.clone()
    } else {
        info.display_name.clone()
    };
    let unique_id = format!("TOUPTEK:{}:{}", name.replace(' ', "-"), key);
    (key, unique_id)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod identity_tests {
    use super::mint_identity;
    use touptek_rs::CameraInfo;

    fn info(id: &str, display: &str) -> CameraInfo {
        CameraInfo {
            id: id.to_owned(),
            display_name: display.to_owned(),
            model_name: "ToupTek Model".to_owned(),
            flag: 0,
            pixel_size_x: 3.76,
            pixel_size_y: 3.76,
            max_width: 6248,
            max_height: 4176,
            bit_depth: 16,
            is_color: false,
            supported_bins: vec![1, 2, 3, 4],
        }
    }

    #[test]
    fn mint_identity_uses_the_device_id() {
        let (key, unique_id) = mint_identity(&info("abc123", "ToupTek ATR3CMOS"), 0);
        assert_eq!(key, "abc123");
        assert_eq!(unique_id, "TOUPTEK:ToupTek-ATR3CMOS:abc123");
    }

    #[test]
    fn mint_identity_falls_back_to_position_when_id_is_blank() {
        let (key, unique_id) = mint_identity(&info("", "ToupTek ATR3CMOS"), 2);
        assert_eq!(key, "noserial-2");
        assert_eq!(unique_id, "TOUPTEK:ToupTek-ATR3CMOS:noserial-2");
    }
}

#[cfg(all(test, feature = "simulation"))]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod simulation_tests {
    use super::*;

    /// End-to-end proof against the `touptek-rs` simulation backend: the builder
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

    /// `devices` overrides are keyed by the bare SDK id, not the prefixed
    /// `TOUPTEK:{name}:{id}` UniqueID.
    #[tokio::test]
    async fn device_overrides_are_keyed_by_id_not_unique_id() {
        let cams = enumerate_cameras().await.unwrap();
        let cam = &cams[0];
        assert!(
            cam.key != cam.unique_id && cam.unique_id.ends_with(&cam.key),
            "UniqueID should be the prefixed id, key the bare id"
        );
    }
}
