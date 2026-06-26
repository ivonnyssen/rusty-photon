//! `TouptekCamera` — the ASCOM `Device` + `Camera` implementation over the
//! [`CameraHandle`](crate::backend::CameraHandle) seam.
//!
//! **Phase C (this file) is a bare scaffold.** It implements the connection
//! lifecycle, stable identity, sensor pixel size, and the cached sub-frame origin
//! — i.e. the methods the `ascom-alpaca` `Camera` trait *requires* — so the
//! service compiles, registers a device, and serves on `:11123`. The exposure
//! state machine (`StartExposure`/`ImageReady`/`ImageArray`), ROI/binning,
//! gain/offset, cooling, RAW readout, sensor type, and ST4 `PulseGuide` are all
//! Phase E: they fall back to the trait's `NotImplemented` defaults until then.
//! See `docs/plans/touptek-driver.md` (Delivery phasing) and the forthcoming
//! `docs/services/touptek-camera.md`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::{Camera, Device};
use ascom_alpaca::{ASCOMError, ASCOMResult};
use touptek_rs::CameraInfo;

use crate::backend::CameraHandle;
use crate::config::DeviceOverride;
use crate::config_actions::TouptekCameraDriver;
use rusty_photon_driver::ConfigActionCtx;

/// Placeholder exposure bounds. ToupTek's exposure control is in microseconds and
/// the real range comes from `Toupcam_get_ExpTimeRange`; Phase E reads it at the
/// connect handshake. Until then these are conservative, ConformU-plausible
/// constants so the required `ExposureMin`/`Max`/`Resolution` members answer.
const EXPOSURE_MIN: Duration = Duration::from_micros(100);
const EXPOSURE_MAX: Duration = Duration::from_secs(3600);
const EXPOSURE_RESOLUTION: Duration = Duration::from_micros(1);

/// `MaxADU` for the 16-bit RAW readout the astro path uses (`2^16 − 1`). Phase E
/// derives this from the configured bit depth.
const MAX_ADU: u32 = 65_535;

/// Per-device runtime state. Phase C carries only the cached sub-frame origin
/// (`StartX`/`StartY`) so the required accessors round-trip; the full exposure
/// state machine lands in Phase E.
#[derive(Debug, Default)]
struct DeviceState {
    start_x: AtomicU32,
    start_y: AtomicU32,
}

/// One ASCOM Camera device per discovered ToupTek camera.
#[derive(derive_more::Debug)]
pub struct TouptekCamera {
    #[debug(skip)]
    handle: Arc<dyn CameraHandle>,
    info: CameraInfo,
    unique_id: String,
    name: String,
    description: String,
    state: Arc<DeviceState>,
    #[debug(skip)]
    config_ctx: Option<ConfigActionCtx<TouptekCameraDriver>>,
}

impl TouptekCamera {
    /// Build a device from an SDK handle and an optional per-id config override.
    /// The ASCOM `UniqueID` is the handle's id-derived id; `name`/`description`
    /// fall back to SDK-derived defaults.
    pub fn new(handle: Arc<dyn CameraHandle>, overrides: Option<&DeviceOverride>) -> Self {
        let info = handle.info();
        let unique_id = handle.unique_id();
        let default_name = if info.display_name.is_empty() {
            info.model_name.clone()
        } else {
            info.display_name.clone()
        };
        let name = overrides
            .and_then(|o| o.name.clone())
            .unwrap_or(default_name);
        let description = overrides
            .and_then(|o| o.description.clone())
            .unwrap_or_else(|| format!("ToupTek camera ({})", info.model_name));
        Self {
            handle,
            info,
            unique_id,
            name,
            description,
            state: Arc::new(DeviceState::default()),
            config_ctx: None,
        }
    }

    /// Attach config-action wiring (enables `config.get`/`apply`/`schema`).
    #[must_use]
    pub fn with_config_actions(mut self, ctx: ConfigActionCtx<TouptekCameraDriver>) -> Self {
        self.config_ctx = Some(ctx);
        self
    }

    fn ensure_connected(&self) -> ASCOMResult<()> {
        if self.handle.is_open() {
            Ok(())
        } else {
            Err(ASCOMError::NOT_CONNECTED)
        }
    }
}

#[async_trait::async_trait]
impl Device for TouptekCamera {
    fn static_name(&self) -> &str {
        &self.name
    }

    fn unique_id(&self) -> &str {
        &self.unique_id
    }

    async fn connected(&self) -> ASCOMResult<bool> {
        Ok(self.handle.is_open())
    }

    async fn set_connected(&self, connected: bool) -> ASCOMResult<()> {
        if self.handle.is_open() == connected {
            return Ok(());
        }
        // `open`/`close` do blocking SDK I/O (`Toupcam_OpenByIndex` enumerates over
        // USB), so offload them off the executor. The handle is `Arc`-backed.
        let handle = Arc::clone(&self.handle);
        tokio::task::spawn_blocking(move || {
            if connected {
                handle.open()
            } else {
                handle.close()
            }
        })
        .await
        .map_err(|e| ASCOMError::invalid_operation(format!("connect task failed: {e}")))?
        .map_err(|_| ASCOMError::NOT_CONNECTED)
    }

    async fn description(&self) -> ASCOMResult<String> {
        Ok(self.description.clone())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("rusty-photon touptek-camera".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }

    async fn supported_actions(&self) -> ASCOMResult<Vec<String>> {
        Ok(rusty_photon_driver::supported_actions(&self.config_ctx))
    }

    async fn action(&self, action: String, parameters: String) -> ASCOMResult<String> {
        rusty_photon_driver::dispatch::<TouptekCameraDriver>(&self.config_ctx, action, parameters)
            .await
    }
}

#[async_trait::async_trait]
impl Camera for TouptekCamera {
    // --- exposure bounds (placeholders until the Phase E connect handshake) -----

    async fn exposure_min(&self) -> ASCOMResult<Duration> {
        self.ensure_connected()?;
        Ok(EXPOSURE_MIN)
    }

    async fn exposure_max(&self) -> ASCOMResult<Duration> {
        self.ensure_connected()?;
        Ok(EXPOSURE_MAX)
    }

    async fn exposure_resolution(&self) -> ASCOMResult<Duration> {
        self.ensure_connected()?;
        Ok(EXPOSURE_RESOLUTION)
    }

    // --- sensor geometry --------------------------------------------------------

    async fn pixel_size_x(&self) -> ASCOMResult<f64> {
        self.ensure_connected()?;
        Ok(f64::from(self.info.pixel_size_x))
    }

    async fn pixel_size_y(&self) -> ASCOMResult<f64> {
        self.ensure_connected()?;
        Ok(f64::from(self.info.pixel_size_y))
    }

    async fn max_adu(&self) -> ASCOMResult<u32> {
        self.ensure_connected()?;
        Ok(MAX_ADU)
    }

    /// ToupTek sensors are shutterless, so `Light = false` captures identically
    /// (no mechanical dark).
    async fn has_shutter(&self) -> ASCOMResult<bool> {
        self.ensure_connected()?;
        Ok(false)
    }

    // --- sub-frame origin (cached round-trip; full ROI is Phase E) --------------

    async fn start_x(&self) -> ASCOMResult<u32> {
        self.ensure_connected()?;
        Ok(self.state.start_x.load(Ordering::Acquire))
    }

    async fn set_start_x(&self, start_x: u32) -> ASCOMResult<()> {
        self.ensure_connected()?;
        self.state.start_x.store(start_x, Ordering::Release);
        Ok(())
    }

    async fn start_y(&self) -> ASCOMResult<u32> {
        self.ensure_connected()?;
        Ok(self.state.start_y.load(Ordering::Acquire))
    }

    async fn set_start_y(&self, start_y: u32) -> ASCOMResult<()> {
        self.ensure_connected()?;
        self.state.start_y.store(start_y, Ordering::Release);
        Ok(())
    }

    // --- exposure state machine (Phase E) ---------------------------------------

    /// Phase E drives this through trigger mode + the `touptek-rs` callback→pull
    /// bridge. The Phase C scaffold has no exposure path yet.
    async fn start_exposure(&self, _duration: Duration, _light: bool) -> ASCOMResult<()> {
        self.ensure_connected()?;
        Err(ASCOMError::NOT_IMPLEMENTED)
    }
}

#[cfg(all(test, feature = "simulation"))]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::backend::TouptekCameraHandle;

    fn sim_camera() -> TouptekCamera {
        let sdk = touptek_rs::Sdk::new().expect("simulation SDK");
        let info = sdk.enumerate().expect("enumerate")[0].clone();
        let handle: Arc<dyn CameraHandle> = Arc::new(TouptekCameraHandle::new(
            sdk,
            0,
            info,
            "TOUPTEK:Simulated-ToupTek-Camera:sim-0".to_string(),
        ));
        TouptekCamera::new(handle, None)
    }

    #[tokio::test]
    async fn identity_is_id_derived() {
        let cam = sim_camera();
        assert_eq!(cam.unique_id(), "TOUPTEK:Simulated-ToupTek-Camera:sim-0");
        assert_eq!(cam.static_name(), "Simulated ToupTek Camera");
    }

    #[tokio::test]
    async fn properties_require_connection() {
        let cam = sim_camera();
        // Before connect, a property read reports NotConnected.
        assert_eq!(
            cam.pixel_size_x().await.unwrap_err().code,
            ASCOMError::NOT_CONNECTED.code
        );
        assert!(!cam.connected().await.unwrap());
        cam.set_connected(true).await.unwrap();
        assert!(cam.connected().await.unwrap());
        // After connect the geometry answers (the sim camera is 3.76 µm).
        assert!((cam.pixel_size_x().await.unwrap() - 3.76).abs() < 1e-6);
        assert_eq!(cam.max_adu().await.unwrap(), 65_535);
        assert!(!cam.has_shutter().await.unwrap());
    }

    #[tokio::test]
    async fn sub_frame_origin_round_trips() {
        let cam = sim_camera();
        cam.set_connected(true).await.unwrap();
        cam.set_start_x(64).await.unwrap();
        cam.set_start_y(48).await.unwrap();
        assert_eq!(cam.start_x().await.unwrap(), 64);
        assert_eq!(cam.start_y().await.unwrap(), 48);
    }

    #[tokio::test]
    async fn start_exposure_is_phase_e() {
        let cam = sim_camera();
        cam.set_connected(true).await.unwrap();
        assert_eq!(
            cam.start_exposure(Duration::from_millis(1), true)
                .await
                .unwrap_err()
                .code,
            ASCOMError::NOT_IMPLEMENTED.code
        );
    }
}
