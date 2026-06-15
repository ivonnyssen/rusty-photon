//! The [`ZwoCamera`] ASCOM device — a minimal Phase C (Track A) Camera.
//!
//! This is intentionally a *bare* device: it reports identity and cached sensor
//! geometry from the enumerated [`CameraInfo`], and tracks connection plus ROI
//! offset state. The full imaging path (exposure state machine, ROI/bin
//! validation, gain/offset, cooling, ST4 pulse-guiding) is Phase E device-trait
//! work and is left to the trait's `NOT_IMPLEMENTED` defaults here.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use ascom_alpaca::api::camera::SensorType;
use ascom_alpaca::api::{Camera, Device};
use ascom_alpaca::{ASCOMError, ASCOMResult};
use async_trait::async_trait;
use zwo_rs::CameraInfo;

/// Lowest exposure the driver advertises (ASI sensors bottom out near 32 µs —
/// required for bias frames).
const EXPOSURE_MIN: Duration = Duration::from_micros(32);
/// Highest exposure the driver advertises.
const EXPOSURE_MAX: Duration = Duration::from_secs(3600);
/// Exposure granularity (ASI exposure control is in microseconds).
const EXPOSURE_RESOLUTION: Duration = Duration::from_micros(1);

/// One enumerated ASI camera, exposed as an ASCOM `Camera` device.
#[derive(Debug)]
pub struct ZwoCamera {
    info: CameraInfo,
    name: String,
    unique_id: String,
    connected: AtomicBool,
    start_x: AtomicU32,
    start_y: AtomicU32,
}

impl ZwoCamera {
    /// Build a device for an enumerated camera at `index` (registration order).
    #[must_use]
    pub fn new(index: usize, info: CameraInfo) -> Self {
        let name = if index == 0 {
            info.name.clone()
        } else {
            format!("{} #{index}", info.name)
        };
        // Phase C placeholder identity. Phase E replaces this with the
        // `ASIGetSerialNumber`-derived UniqueID (see docs/services/zwo-camera.md
        // "Device identity"): the SDK index alone is not a stable identity for
        // two identical-model cameras.
        let unique_id = format!("ZWO:{}:{}", info.name.replace(' ', "-"), info.id);
        Self {
            info,
            name,
            unique_id,
            connected: AtomicBool::new(false),
            start_x: AtomicU32::new(0),
            start_y: AtomicU32::new(0),
        }
    }
}

/// `(2^bit_depth) - 1` — the maximum ADU for a given ADC depth. ASI ADCs are
/// ≤ 16-bit, so the shift never overflows; the saturating fallback is defensive.
fn max_adu_from_bit_depth(bit_depth: u32) -> u32 {
    1u32.checked_shl(bit_depth).map_or(u32::MAX, |v| v - 1)
}

#[async_trait]
impl Device for ZwoCamera {
    fn static_name(&self) -> &str {
        &self.name
    }

    fn unique_id(&self) -> &str {
        &self.unique_id
    }

    async fn connected(&self) -> ASCOMResult<bool> {
        Ok(self.connected.load(Ordering::SeqCst))
    }

    async fn set_connected(&self, connected: bool) -> ASCOMResult<()> {
        // Phase C tracks the flag only; Phase E opens the camera, runs
        // ASIInitCamera, and caches ASI_CAMERA_INFO / control caps here.
        self.connected.store(connected, Ordering::SeqCst);
        Ok(())
    }

    async fn description(&self) -> ASCOMResult<String> {
        Ok(format!("ZWO ASI camera ({})", self.info.name))
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("zwo-camera (rusty-photon) — ASCOM Alpaca driver for ZWO ASI cameras".to_owned())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_owned())
    }
}

#[async_trait]
impl Camera for ZwoCamera {
    async fn exposure_min(&self) -> ASCOMResult<Duration> {
        Ok(EXPOSURE_MIN)
    }

    async fn exposure_max(&self) -> ASCOMResult<Duration> {
        Ok(EXPOSURE_MAX)
    }

    async fn exposure_resolution(&self) -> ASCOMResult<Duration> {
        Ok(EXPOSURE_RESOLUTION)
    }

    async fn has_shutter(&self) -> ASCOMResult<bool> {
        // ASI sensors are shutterless; darks/bias differ only in client metadata.
        Ok(false)
    }

    async fn max_adu(&self) -> ASCOMResult<u32> {
        Ok(max_adu_from_bit_depth(self.info.bit_depth))
    }

    async fn pixel_size_x(&self) -> ASCOMResult<f64> {
        Ok(self.info.pixel_size_um)
    }

    async fn pixel_size_y(&self) -> ASCOMResult<f64> {
        // ASI exposes a single pixel size, so X == Y trivially.
        Ok(self.info.pixel_size_um)
    }

    async fn start_x(&self) -> ASCOMResult<u32> {
        Ok(self.start_x.load(Ordering::SeqCst))
    }

    async fn set_start_x(&self, start_x: u32) -> ASCOMResult<()> {
        self.start_x.store(start_x, Ordering::SeqCst);
        Ok(())
    }

    async fn start_y(&self) -> ASCOMResult<u32> {
        Ok(self.start_y.load(Ordering::SeqCst))
    }

    async fn set_start_y(&self, start_y: u32) -> ASCOMResult<()> {
        self.start_y.store(start_y, Ordering::SeqCst);
        Ok(())
    }

    async fn start_exposure(&self, _duration: Duration, _light: bool) -> ASCOMResult<()> {
        // Phase E implements the snap-mode exposure state machine
        // (ASIStartExposure → status poll → ASIGetDataAfterExp → ImageArray).
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    // --- Cheap reads from cached info: a "sim Camera" that reports geometry ---

    async fn camera_x_size(&self) -> ASCOMResult<u32> {
        Ok(self.info.max_width)
    }

    async fn camera_y_size(&self) -> ASCOMResult<u32> {
        Ok(self.info.max_height)
    }

    async fn sensor_name(&self) -> ASCOMResult<String> {
        Ok(self.info.name.clone())
    }

    async fn sensor_type(&self) -> ASCOMResult<SensorType> {
        Ok(if self.info.is_color {
            SensorType::RGGB
        } else {
            SensorType::Monochrome
        })
    }

    async fn electrons_per_adu(&self) -> ASCOMResult<f64> {
        // A ZWO win: a real native value, not the NOT_IMPLEMENTED placeholder.
        Ok(f64::from(self.info.e_per_adu))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use zwo_rs::BayerPattern;

    fn sample_info() -> CameraInfo {
        CameraInfo {
            id: 0,
            name: "ASI2600MM Pro".to_owned(),
            max_width: 6248,
            max_height: 4176,
            is_color: false,
            bayer_pattern: BayerPattern::Rg,
            supported_bins: vec![1, 2, 3, 4],
            pixel_size_um: 3.76,
            has_mechanical_shutter: false,
            has_st4_port: true,
            is_cooler_cam: true,
            is_usb3: true,
            e_per_adu: 0.25,
            bit_depth: 16,
            is_trigger_cam: false,
        }
    }

    #[test]
    fn max_adu_matches_bit_depth() {
        assert_eq!(max_adu_from_bit_depth(16), 65_535);
        assert_eq!(max_adu_from_bit_depth(14), 16_383);
        assert_eq!(max_adu_from_bit_depth(12), 4_095);
        assert_eq!(max_adu_from_bit_depth(0), 0);
    }

    #[test]
    fn first_device_keeps_the_model_name() {
        let camera = ZwoCamera::new(0, sample_info());
        assert_eq!(camera.static_name(), "ASI2600MM Pro");
    }

    #[test]
    fn later_devices_are_suffixed_by_index() {
        let camera = ZwoCamera::new(2, sample_info());
        assert_eq!(camera.static_name(), "ASI2600MM Pro #2");
    }

    #[test]
    fn unique_id_is_non_empty_and_stable() {
        let camera = ZwoCamera::new(0, sample_info());
        assert_eq!(camera.unique_id(), "ZWO:ASI2600MM-Pro:0");
        // Same info → same id (a stable ASCOM UniqueID for the sim camera).
        assert_eq!(
            ZwoCamera::new(0, sample_info()).unique_id(),
            camera.unique_id()
        );
    }

    #[tokio::test]
    async fn reports_cached_geometry_and_monochrome_sensor() {
        let camera = ZwoCamera::new(0, sample_info());
        assert_eq!(camera.camera_x_size().await.unwrap(), 6248);
        assert_eq!(camera.camera_y_size().await.unwrap(), 4176);
        assert!((camera.pixel_size_x().await.unwrap() - 3.76).abs() < f64::EPSILON);
        assert_eq!(
            camera.pixel_size_x().await.unwrap(),
            camera.pixel_size_y().await.unwrap()
        );
        assert_eq!(camera.sensor_type().await.unwrap(), SensorType::Monochrome);
        assert_eq!(camera.max_adu().await.unwrap(), 65_535);
        assert!(!camera.has_shutter().await.unwrap());
    }

    #[tokio::test]
    async fn connection_flag_round_trips() {
        let camera = ZwoCamera::new(0, sample_info());
        assert!(!camera.connected().await.unwrap());
        camera.set_connected(true).await.unwrap();
        assert!(camera.connected().await.unwrap());
        camera.set_connected(false).await.unwrap();
        assert!(!camera.connected().await.unwrap());
    }

    #[tokio::test]
    async fn roi_offset_setters_round_trip() {
        let camera = ZwoCamera::new(0, sample_info());
        camera.set_start_x(128).await.unwrap();
        camera.set_start_y(64).await.unwrap();
        assert_eq!(camera.start_x().await.unwrap(), 128);
        assert_eq!(camera.start_y().await.unwrap(), 64);
    }
}
