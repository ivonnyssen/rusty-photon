use ascom_alpaca::api::{Camera, Device};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::debug;

use crate::config::Config;
use crate::pointing::{PointingState, SharedPointing};

/// 0x500 — ASCOM "unspecified" / driver-specific catch-all. The
/// Behavioral Contracts in `docs/services/sky-survey-camera.md` use
/// "UNSPECIFIED_ERROR" for everything that isn't covered by a more
/// precise standard code.
const UNSPECIFIED_ERROR: ASCOMErrorCode = ASCOMErrorCode::new_for_driver(0);

const MAX_BIN: u8 = 4;
const EXPOSURE_MIN: Duration = Duration::from_micros(1);
const EXPOSURE_MAX: Duration = Duration::from_secs(3600);

/// Shared state held by the [`SkySurveyCamera`] device and the custom
/// `/sky-survey/*` HTTP routes. Cloning a [`SkySurveyCamera`] only
/// clones the `Arc` — both views observe the same connection and
/// pointing state.
#[derive(Debug)]
pub struct DeviceState {
    pub config: Config,
    pub connected: AtomicBool,
    pub pointing: SharedPointing,
    pub bin_x: AtomicU8,
    pub bin_y: AtomicU8,
    pub num_x: AtomicU32,
    pub num_y: AtomicU32,
    pub start_x: AtomicU32,
    pub start_y: AtomicU32,
    pub exposure_in_flight: AtomicBool,
}

#[derive(Debug, Clone)]
pub struct SkySurveyCamera {
    state: Arc<DeviceState>,
}

impl SkySurveyCamera {
    pub fn new(config: Config) -> Self {
        let pointing = SharedPointing::new(PointingState::new(
            config.pointing.initial_ra_deg,
            config.pointing.initial_dec_deg,
            config.pointing.initial_rotation_deg,
        ));
        let sensor_w = config.optics.sensor_width_px;
        let sensor_h = config.optics.sensor_height_px;
        let state = DeviceState {
            config,
            connected: AtomicBool::new(false),
            pointing,
            bin_x: AtomicU8::new(1),
            bin_y: AtomicU8::new(1),
            num_x: AtomicU32::new(sensor_w),
            num_y: AtomicU32::new(sensor_h),
            start_x: AtomicU32::new(0),
            start_y: AtomicU32::new(0),
            exposure_in_flight: AtomicBool::new(false),
        };
        Self {
            state: Arc::new(state),
        }
    }

    pub fn shared_state(&self) -> Arc<DeviceState> {
        Arc::clone(&self.state)
    }

    pub fn is_connected(&self) -> bool {
        self.state.connected.load(Ordering::Acquire)
    }
}

#[async_trait::async_trait]
impl Device for SkySurveyCamera {
    fn static_name(&self) -> &str {
        &self.state.config.device.name
    }

    fn unique_id(&self) -> &str {
        &self.state.config.device.unique_id
    }

    async fn connected(&self) -> ASCOMResult<bool> {
        Ok(self.state.connected.load(Ordering::Acquire))
    }

    async fn set_connected(&self, connected: bool) -> ASCOMResult<()> {
        if connected {
            // C2: cache_dir must be creatable + writable.
            let cache_dir = &self.state.config.survey.cache_dir;
            if let Err(e) = std::fs::create_dir_all(cache_dir) {
                debug!(?cache_dir, error = %e, "cache_dir not writable");
                return Err(ASCOMError::new(
                    UNSPECIFIED_ERROR,
                    format!("cache_dir is not writable: {e}"),
                ));
            }
            // C3: survey endpoint must respond to a HEAD request.
            let endpoint = &self.state.config.survey.endpoint;
            let timeout = self.state.config.survey.request_timeout;
            let client = match reqwest::Client::builder().timeout(timeout).build() {
                Ok(c) => c,
                Err(e) => {
                    return Err(ASCOMError::new(
                        UNSPECIFIED_ERROR,
                        format!("HTTP client build failed: {e}"),
                    ));
                }
            };
            match client.head(endpoint).send().await {
                Ok(resp) if resp.status().is_success() => {}
                Ok(resp) => {
                    debug!(status = %resp.status(), "survey endpoint HEAD non-success");
                    return Err(ASCOMError::new(
                        UNSPECIFIED_ERROR,
                        format!(
                            "survey endpoint {endpoint} returned status {}",
                            resp.status()
                        ),
                    ));
                }
                Err(e) => {
                    debug!(error = %e, "survey endpoint unreachable");
                    return Err(ASCOMError::new(
                        UNSPECIFIED_ERROR,
                        format!("survey endpoint {endpoint} unreachable: {e}"),
                    ));
                }
            }
        }
        self.state.connected.store(connected, Ordering::Release);
        Ok(())
    }

    async fn description(&self) -> ASCOMResult<String> {
        Ok(self.state.config.device.description.clone())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("rusty-photon sky-survey-camera".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }
}

#[async_trait::async_trait]
impl Camera for SkySurveyCamera {
    async fn camera_x_size(&self) -> ASCOMResult<u32> {
        Ok(self.state.config.optics.sensor_width_px)
    }

    async fn camera_y_size(&self) -> ASCOMResult<u32> {
        Ok(self.state.config.optics.sensor_height_px)
    }

    async fn pixel_size_x(&self) -> ASCOMResult<f64> {
        Ok(self.state.config.optics.pixel_size_x_um)
    }

    async fn pixel_size_y(&self) -> ASCOMResult<f64> {
        Ok(self.state.config.optics.pixel_size_y_um)
    }

    async fn exposure_min(&self) -> ASCOMResult<Duration> {
        Ok(EXPOSURE_MIN)
    }

    async fn exposure_max(&self) -> ASCOMResult<Duration> {
        Ok(EXPOSURE_MAX)
    }

    async fn exposure_resolution(&self) -> ASCOMResult<Duration> {
        Ok(Duration::from_micros(1))
    }

    async fn has_shutter(&self) -> ASCOMResult<bool> {
        Ok(false)
    }

    async fn max_adu(&self) -> ASCOMResult<u32> {
        Ok(65535)
    }

    async fn max_bin_x(&self) -> ASCOMResult<u8> {
        Ok(MAX_BIN)
    }

    async fn max_bin_y(&self) -> ASCOMResult<u8> {
        Ok(MAX_BIN)
    }

    async fn bin_x(&self) -> ASCOMResult<u8> {
        Ok(self.state.bin_x.load(Ordering::Acquire))
    }

    async fn set_bin_x(&self, bin_x: u8) -> ASCOMResult<()> {
        if !(1..=MAX_BIN).contains(&bin_x) {
            return Err(ASCOMError::invalid_value(format!(
                "BinX {bin_x} outside [1, {MAX_BIN}]"
            )));
        }
        self.state.bin_x.store(bin_x, Ordering::Release);
        Ok(())
    }

    async fn bin_y(&self) -> ASCOMResult<u8> {
        Ok(self.state.bin_y.load(Ordering::Acquire))
    }

    async fn set_bin_y(&self, bin_y: u8) -> ASCOMResult<()> {
        if !(1..=MAX_BIN).contains(&bin_y) {
            return Err(ASCOMError::invalid_value(format!(
                "BinY {bin_y} outside [1, {MAX_BIN}]"
            )));
        }
        self.state.bin_y.store(bin_y, Ordering::Release);
        Ok(())
    }

    async fn num_x(&self) -> ASCOMResult<u32> {
        Ok(self.state.num_x.load(Ordering::Acquire))
    }

    async fn set_num_x(&self, num_x: u32) -> ASCOMResult<()> {
        let bin = self.state.bin_x.load(Ordering::Acquire) as u32;
        let max = self.state.config.optics.sensor_width_px / bin.max(1);
        if num_x == 0 || num_x > max {
            return Err(ASCOMError::invalid_value(format!(
                "NumX {num_x} outside (0, {max}]"
            )));
        }
        self.state.num_x.store(num_x, Ordering::Release);
        Ok(())
    }

    async fn num_y(&self) -> ASCOMResult<u32> {
        Ok(self.state.num_y.load(Ordering::Acquire))
    }

    async fn set_num_y(&self, num_y: u32) -> ASCOMResult<()> {
        let bin = self.state.bin_y.load(Ordering::Acquire) as u32;
        let max = self.state.config.optics.sensor_height_px / bin.max(1);
        if num_y == 0 || num_y > max {
            return Err(ASCOMError::invalid_value(format!(
                "NumY {num_y} outside (0, {max}]"
            )));
        }
        self.state.num_y.store(num_y, Ordering::Release);
        Ok(())
    }

    async fn start_x(&self) -> ASCOMResult<u32> {
        Ok(self.state.start_x.load(Ordering::Acquire))
    }

    async fn set_start_x(&self, start_x: u32) -> ASCOMResult<()> {
        let bin = self.state.bin_x.load(Ordering::Acquire) as u32;
        let limit = self.state.config.optics.sensor_width_px / bin.max(1);
        if start_x >= limit {
            return Err(ASCOMError::invalid_value(format!(
                "StartX {start_x} >= sensor/bin {limit}"
            )));
        }
        self.state.start_x.store(start_x, Ordering::Release);
        Ok(())
    }

    async fn start_y(&self) -> ASCOMResult<u32> {
        Ok(self.state.start_y.load(Ordering::Acquire))
    }

    async fn set_start_y(&self, start_y: u32) -> ASCOMResult<()> {
        let bin = self.state.bin_y.load(Ordering::Acquire) as u32;
        let limit = self.state.config.optics.sensor_height_px / bin.max(1);
        if start_y >= limit {
            return Err(ASCOMError::invalid_value(format!(
                "StartY {start_y} >= sensor/bin {limit}"
            )));
        }
        self.state.start_y.store(start_y, Ordering::Release);
        Ok(())
    }

    async fn start_exposure(&self, duration: Duration, _light: bool) -> ASCOMResult<()> {
        if !self.state.connected.load(Ordering::Acquire) {
            return Err(ASCOMError::new(
                ASCOMErrorCode::NOT_CONNECTED,
                "camera is not connected",
            ));
        }
        if !(EXPOSURE_MIN..=EXPOSURE_MAX).contains(&duration) {
            return Err(ASCOMError::invalid_value(format!(
                "Duration {duration:?} outside [{EXPOSURE_MIN:?}, {EXPOSURE_MAX:?}]"
            )));
        }
        // Sub-frame combination is validated at setter time; here we
        // just ensure the user has supplied a non-zero region.
        let bx = self.state.bin_x.load(Ordering::Acquire) as u32;
        let by = self.state.bin_y.load(Ordering::Acquire) as u32;
        let nx = self.state.num_x.load(Ordering::Acquire);
        let ny = self.state.num_y.load(Ordering::Acquire);
        let sx = self.state.start_x.load(Ordering::Acquire);
        let sy = self.state.start_y.load(Ordering::Acquire);
        let sensor_x_binned = self.state.config.optics.sensor_width_px / bx.max(1);
        let sensor_y_binned = self.state.config.optics.sensor_height_px / by.max(1);
        if sx + nx > sensor_x_binned || sy + ny > sensor_y_binned {
            return Err(ASCOMError::invalid_value(format!(
                "subframe ({sx}+{nx},{sy}+{ny}) exceeds binned sensor ({sensor_x_binned},{sensor_y_binned})"
            )));
        }
        // E2: reject if another exposure is already in flight. Slice 4
        // turns this into a real survey fetch with cancellation.
        if self
            .state
            .exposure_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ASCOMError::invalid_operation("exposure already in flight"));
        }
        debug!(
            ?duration,
            "exposure started (slice-3 stub: in_flight set, no fetch)"
        );
        Ok(())
    }

    async fn abort_exposure(&self) -> ASCOMResult<()> {
        if !self
            .state
            .exposure_in_flight
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .is_ok_and(|prev| prev)
        {
            return Err(ASCOMError::invalid_operation(
                "no exposure in progress to abort",
            ));
        }
        Ok(())
    }

    async fn stop_exposure(&self) -> ASCOMResult<()> {
        if !self
            .state
            .exposure_in_flight
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .is_ok_and(|prev| prev)
        {
            return Err(ASCOMError::invalid_operation(
                "no exposure in progress to stop",
            ));
        }
        Ok(())
    }

    async fn image_ready(&self) -> ASCOMResult<bool> {
        // Slice 4 will replace this with a real readiness flag tracked
        // by the exposure pipeline. For now: never ready (in_flight
        // exposures never complete in slice 3), which matches the
        // cancellation contract A1 ("ImageReady is false").
        Ok(false)
    }
}
