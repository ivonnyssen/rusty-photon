use ascom_alpaca::api::{Camera, Device};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Shared state held by the [`SkySurveyCamera`] device and the custom
/// `/sky-survey/*` HTTP routes. Cloning a [`SkySurveyCamera`] only
/// clones the `Arc` — both views observe the same connection and
/// pointing state.
#[derive(Debug)]
pub struct DeviceState {
    pub config: Config,
    pub connected: AtomicBool,
    pub pointing: SharedPointing,
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
        let state = DeviceState {
            config,
            connected: AtomicBool::new(false),
            pointing,
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
        Ok(Duration::from_micros(1))
    }

    async fn exposure_max(&self) -> ASCOMResult<Duration> {
        Ok(Duration::from_secs(3600))
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

    async fn start_x(&self) -> ASCOMResult<u32> {
        Ok(0)
    }

    async fn set_start_x(&self, _start_x: u32) -> ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn start_y(&self) -> ASCOMResult<u32> {
        Ok(0)
    }

    async fn set_start_y(&self, _start_y: u32) -> ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn start_exposure(&self, _duration: Duration, _light: bool) -> ASCOMResult<()> {
        // Slice 1: not yet implemented. Slices 3 / 4 wire in parameter
        // validation, the SurveyClient fetch, the FITS cache, and the
        // ImageArray production path per contracts E1-E6 and S1-S6.
        Err(ASCOMError::NOT_IMPLEMENTED)
    }
}
