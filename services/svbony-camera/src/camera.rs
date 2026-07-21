//! `SvbonyCamera` — the ASCOM `Device` + `Camera` implementation over the
//! [`CameraHandle`](crate::backend::CameraHandle) seam.
//!
//! **Phase C/D scope (this file).** `Device` is implemented for real:
//! `Name`/`Description`/`DriverInfo`/`DriverVersion`/`Connected`/`UniqueID`
//! and the `config.get`/`apply`/`schema` action dispatch all genuinely work,
//! and connect/disconnect drive the real `CameraHandle::open`/`close`. Every
//! `ascom_alpaca::api::Camera` method is present and compiles, but returns
//! `ASCOMError::NOT_IMPLEMENTED` uniformly — an honest "not yet built"
//! response, not a partial/best-effort one. Phase E
//! (`docs/plans/svbony-camera.md`) replaces these stubs one behavioural area
//! at a time (exposure, ROI/binning, gain/offset, cooling, sensor
//! properties), following the state machine documented in
//! `docs/services/svbony-camera.md` "Behavioral contracts → Exposure".

use std::sync::Arc;

use ascom_alpaca::api::camera::{CameraState, GuideDirection, ImageArray, SensorType};
use ascom_alpaca::api::{Camera, Device};
use ascom_alpaca::{ASCOMError, ASCOMResult};
use std::time::{Duration, SystemTime};
use svbony_rs::CameraInfo;

use crate::backend::CameraHandle;
use crate::config::DeviceOverride;
use crate::config_actions::SvbonyCameraDriver;
use rusty_photon_driver::ConfigActionCtx;

/// One ASCOM Camera device per discovered SVBony camera.
#[derive(Clone, derive_more::Debug)]
pub struct SvbonyCamera {
    #[debug(skip)]
    handle: Arc<dyn CameraHandle>,
    #[allow(dead_code)] // read by Phase E device-property implementations
    info: CameraInfo,
    unique_id: String,
    name: String,
    description: String,
    #[debug(skip)]
    config_ctx: Option<ConfigActionCtx<SvbonyCameraDriver>>,
}

impl SvbonyCamera {
    /// Build a device from an SDK handle and an optional per-serial config
    /// override. The ASCOM `UniqueID` is the handle's serial-derived id;
    /// `name`/`description` fall back to SDK-derived defaults.
    pub fn new(handle: Arc<dyn CameraHandle>, overrides: Option<&DeviceOverride>) -> Self {
        let info = handle.info();
        let unique_id = handle.unique_id();
        let name = overrides
            .and_then(|o| o.name.clone())
            .unwrap_or_else(|| info.friendly_name.clone());
        let description = overrides
            .and_then(|o| o.description.clone())
            .unwrap_or_else(|| format!("SVBony camera ({})", info.friendly_name));
        Self {
            handle,
            info,
            unique_id,
            name,
            description,
            config_ctx: None,
        }
    }

    /// Attach config-action wiring (enables `config.get`/`apply`/`schema`).
    #[must_use]
    pub fn with_config_actions(mut self, ctx: ConfigActionCtx<SvbonyCameraDriver>) -> Self {
        self.config_ctx = Some(ctx);
        self
    }

    fn connect(&self) -> ASCOMResult<()> {
        self.handle.open().map_err(|_| ASCOMError::NOT_CONNECTED)?;
        tracing::debug!(camera = %self.unique_id, "camera connected");
        Ok(())
    }

    fn disconnect(&self) -> ASCOMResult<()> {
        self.handle.close().map_err(|_| ASCOMError::NOT_CONNECTED)?;
        tracing::debug!(camera = %self.unique_id, "camera disconnected");
        Ok(())
    }
}

#[async_trait::async_trait]
impl Device for SvbonyCamera {
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
        // `open`/`close` do blocking SDK I/O, so offload off the executor
        // (SvbonyCamera is cheap to clone: it is `Arc`-backed).
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            if connected {
                this.connect()
            } else {
                this.disconnect()
            }
        })
        .await
        .map_err(|e| ASCOMError::invalid_operation(format!("connect task failed: {e}")))?
    }

    async fn description(&self) -> ASCOMResult<String> {
        Ok(self.description.clone())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("rusty-photon svbony-camera".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }

    async fn supported_actions(&self) -> ASCOMResult<Vec<String>> {
        Ok(rusty_photon_driver::supported_actions(&self.config_ctx))
    }

    async fn action(&self, action: String, parameters: String) -> ASCOMResult<String> {
        rusty_photon_driver::dispatch::<SvbonyCameraDriver>(&self.config_ctx, action, parameters)
            .await
    }
}

/// Every method below is an intentional Phase-C/D stub: present, compiling,
/// and honestly `NOT_IMPLEMENTED`. See the module docs and
/// `docs/services/svbony-camera.md` for the Phase E replacement plan.
#[async_trait::async_trait]
impl Camera for SvbonyCamera {
    async fn camera_x_size(&self) -> ASCOMResult<u32> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn camera_y_size(&self) -> ASCOMResult<u32> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn pixel_size_x(&self) -> ASCOMResult<f64> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn pixel_size_y(&self) -> ASCOMResult<f64> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn max_adu(&self) -> ASCOMResult<u32> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn electrons_per_adu(&self) -> ASCOMResult<f64> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn sensor_name(&self) -> ASCOMResult<String> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn bin_x(&self) -> ASCOMResult<u8> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn bin_y(&self) -> ASCOMResult<u8> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn set_bin_x(&self, _bin_x: u8) -> ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn set_bin_y(&self, _bin_y: u8) -> ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn max_bin_x(&self) -> ASCOMResult<u8> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn max_bin_y(&self) -> ASCOMResult<u8> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn can_asymmetric_bin(&self) -> ASCOMResult<bool> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn num_x(&self) -> ASCOMResult<u32> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn num_y(&self) -> ASCOMResult<u32> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn start_x(&self) -> ASCOMResult<u32> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn start_y(&self) -> ASCOMResult<u32> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn set_num_x(&self, _num_x: u32) -> ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn set_num_y(&self, _num_y: u32) -> ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn set_start_x(&self, _start_x: u32) -> ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn set_start_y(&self, _start_y: u32) -> ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn exposure_min(&self) -> ASCOMResult<Duration> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn exposure_max(&self) -> ASCOMResult<Duration> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn exposure_resolution(&self) -> ASCOMResult<Duration> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn gain(&self) -> ASCOMResult<i32> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn gain_min(&self) -> ASCOMResult<i32> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn gain_max(&self) -> ASCOMResult<i32> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn set_gain(&self, _gain: i32) -> ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn offset(&self) -> ASCOMResult<i32> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn offset_min(&self) -> ASCOMResult<i32> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn offset_max(&self) -> ASCOMResult<i32> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn set_offset(&self, _offset: i32) -> ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn readout_mode(&self) -> ASCOMResult<usize> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn readout_modes(&self) -> ASCOMResult<Vec<String>> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn set_readout_mode(&self, _readout_mode: usize) -> ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn sensor_type(&self) -> ASCOMResult<SensorType> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn bayer_offset_x(&self) -> ASCOMResult<u8> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn bayer_offset_y(&self) -> ASCOMResult<u8> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn can_set_ccd_temperature(&self) -> ASCOMResult<bool> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn can_get_cooler_power(&self) -> ASCOMResult<bool> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn ccd_temperature(&self) -> ASCOMResult<f64> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn set_ccd_temperature(&self) -> ASCOMResult<f64> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn set_set_ccd_temperature(&self, _set_ccd_temperature: f64) -> ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn cooler_on(&self) -> ASCOMResult<bool> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn set_cooler_on(&self, _cooler_on: bool) -> ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn cooler_power(&self) -> ASCOMResult<f64> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn has_shutter(&self) -> ASCOMResult<bool> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn can_abort_exposure(&self) -> ASCOMResult<bool> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn can_stop_exposure(&self) -> ASCOMResult<bool> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn can_pulse_guide(&self) -> ASCOMResult<bool> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn is_pulse_guiding(&self) -> ASCOMResult<bool> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn camera_state(&self) -> ASCOMResult<CameraState> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn image_ready(&self) -> ASCOMResult<bool> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn percent_completed(&self) -> ASCOMResult<u8> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn last_exposure_start_time(&self) -> ASCOMResult<SystemTime> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn last_exposure_duration(&self) -> ASCOMResult<Duration> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn image_array(&self) -> ASCOMResult<ImageArray> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn start_exposure(&self, _duration: Duration, _light: bool) -> ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn abort_exposure(&self) -> ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn stop_exposure(&self) -> ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn pulse_guide(
        &self,
        _direction: GuideDirection,
        _duration: Duration,
    ) -> ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::backend::mock::MockCameraHandle;
    use std::sync::atomic::Ordering;

    fn device(handle: MockCameraHandle) -> SvbonyCamera {
        SvbonyCamera::new(Arc::new(handle), None)
    }

    #[tokio::test]
    async fn starts_disconnected() {
        let cam = device(MockCameraHandle::default());
        assert!(!cam.connected().await.unwrap());
    }

    #[tokio::test]
    async fn set_connected_true_opens_the_handle() {
        let cam = device(MockCameraHandle::default());
        cam.set_connected(true).await.unwrap();
        assert!(cam.connected().await.unwrap());
    }

    #[tokio::test]
    async fn set_connected_false_closes_the_handle() {
        let cam = device(MockCameraHandle::default());
        cam.set_connected(true).await.unwrap();
        cam.set_connected(false).await.unwrap();
        assert!(!cam.connected().await.unwrap());
    }

    #[tokio::test]
    async fn a_failed_open_leaves_the_device_disconnected() {
        let handle = MockCameraHandle::default();
        handle.fail_open.store(true, Ordering::SeqCst);
        let cam = device(handle);
        cam.set_connected(true).await.unwrap_err();
        assert!(!cam.connected().await.unwrap());
    }

    #[tokio::test]
    async fn reconnecting_is_a_no_op() {
        let cam = device(MockCameraHandle::default());
        cam.set_connected(true).await.unwrap();
        // Idempotent: connecting an already-connected device does not error.
        cam.set_connected(true).await.unwrap();
        assert!(cam.connected().await.unwrap());
    }

    #[tokio::test]
    async fn unique_id_and_name_come_from_the_handle() {
        let cam = device(MockCameraHandle::default());
        assert_eq!(cam.unique_id(), "SVBONY:SV605CC-Simulated:SVB0123456789AB");
        assert_eq!(cam.static_name(), "SV605CC-Simulated");
    }

    #[tokio::test]
    async fn a_name_override_wins_over_the_sdk_default() {
        let overrides = DeviceOverride {
            name: Some("Main Imaging".to_string()),
            description: None,
        };
        let cam = SvbonyCamera::new(Arc::new(MockCameraHandle::default()), Some(&overrides));
        assert_eq!(cam.static_name(), "Main Imaging");
    }

    #[tokio::test]
    async fn every_camera_method_is_honestly_not_implemented() {
        let cam = device(MockCameraHandle::default());
        cam.set_connected(true).await.unwrap();
        assert_eq!(
            cam.camera_x_size().await.unwrap_err().code,
            ASCOMError::NOT_IMPLEMENTED.code
        );
        assert_eq!(
            cam.start_exposure(Duration::from_secs(1), true)
                .await
                .unwrap_err()
                .code,
            ASCOMError::NOT_IMPLEMENTED.code
        );
        assert_eq!(
            cam.cooler_on().await.unwrap_err().code,
            ASCOMError::NOT_IMPLEMENTED.code
        );
    }
}
