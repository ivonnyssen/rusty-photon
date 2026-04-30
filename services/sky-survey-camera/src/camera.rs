use ascom_alpaca::api::camera::ImageArray;
use ascom_alpaca::api::{Camera, Device};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use ndarray::Array2;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tracing::{debug, warn};

use crate::config::Config;
use crate::fits::parse_primary_hdu;
use crate::pointing::{PointingState, SharedPointing};
use crate::survey::{try_cache_load, try_cache_store, SkyViewClient, SurveyError, SurveyRequest};

/// 0x500 — ASCOM "unspecified" / driver-specific catch-all. The
/// Behavioral Contracts in `docs/services/sky-survey-camera.md` use
/// "UNSPECIFIED_ERROR" for everything that isn't covered by a more
/// precise standard code.
const UNSPECIFIED_ERROR: ASCOMErrorCode = ASCOMErrorCode::new_for_driver(0);

const MAX_BIN: u8 = 4;
const EXPOSURE_MIN: Duration = Duration::from_micros(1);
const EXPOSURE_MAX: Duration = Duration::from_secs(3600);

/// Builds the v0 cutout `SurveyRequest` for a snapshot of camera
/// state: the cutout is always sized to the binned full sensor (the
/// design doc crops sub-frames out client-side after the FITS comes
/// back).
pub fn build_full_sensor_request(
    config: &Config,
    pointing: PointingState,
    bin_x: u8,
    bin_y: u8,
) -> SurveyRequest {
    let plate_scale_x_arcsec =
        206.265 * config.optics.pixel_size_x_um / config.optics.focal_length_mm;
    let plate_scale_y_arcsec =
        206.265 * config.optics.pixel_size_y_um / config.optics.focal_length_mm;
    let bx = bin_x.max(1) as u32;
    let by = bin_y.max(1) as u32;
    let pixels_x = config.optics.sensor_width_px / bx;
    let pixels_y = config.optics.sensor_height_px / by;
    let size_x_deg = plate_scale_x_arcsec * config.optics.sensor_width_px as f64 / 3600.0;
    let size_y_deg = plate_scale_y_arcsec * config.optics.sensor_height_px as f64 / 3600.0;
    SurveyRequest {
        survey: config.survey.name.clone(),
        ra_deg: pointing.ra_deg,
        dec_deg: pointing.dec_deg,
        rotation_deg: pointing.rotation_deg,
        pixels_x,
        pixels_y,
        size_x_deg,
        size_y_deg,
    }
}

/// Outcome of the spawned exposure task — stashed on the device state
/// so subsequent ASCOM calls can map it to ImageArray vs ASCOM error.
#[derive(Debug)]
pub struct ExposureOutcome {
    pub width: u32,
    pub height: u32,
    pub data: Vec<i32>,
}

/// Shared state held by the [`SkySurveyCamera`] device and the custom
/// `/sky-survey/*` HTTP routes. Cloning a [`SkySurveyCamera`] only
/// clones the `Arc` — both views observe the same connection and
/// pointing state.
///
/// `exposure_generation` is bumped on every `start_exposure`, every
/// `abort_exposure` / `stop_exposure`, and every `set_connected(false)`
/// — the spawned exposure task captures the value at start and only
/// commits its result if the captured value still matches when it
/// finishes, so a late-completing task can never resurrect an image
/// after Abort/Stop/disconnect.
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
    pub image_ready: AtomicBool,
    pub last_image: Mutex<Option<ExposureOutcome>>,
    pub last_error: Mutex<Option<String>>,
    pub last_exposure_start: Mutex<Option<SystemTime>>,
    pub last_exposure_duration: Mutex<Option<Duration>>,
    pub exposure_generation: AtomicU64,
    pub survey_client: Arc<SkyViewClient>,
}

#[derive(Debug, Clone)]
pub struct SkySurveyCamera {
    state: Arc<DeviceState>,
}

impl SkySurveyCamera {
    pub fn new(config: Config, survey_client: Arc<SkyViewClient>) -> Self {
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
            image_ready: AtomicBool::new(false),
            last_image: Mutex::new(None),
            last_error: Mutex::new(None),
            last_exposure_start: Mutex::new(None),
            last_exposure_duration: Mutex::new(None),
            exposure_generation: AtomicU64::new(0),
            survey_client,
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

/// The body of the spawned exposure task. Performs the cache hit /
/// fetch / parse / sub-frame crop; on any failure stores the message
/// in `state.last_error` and clears `in_flight` so subsequent
/// `image_array` calls can surface UNSPECIFIED_ERROR.
///
/// `gen` is the value of `exposure_generation` at the moment
/// `start_exposure` spawned this task. Abort, Stop, and disconnect
/// bump that counter, so a late-completing task whose generation
/// no longer matches must NOT publish its outcome — that would
/// resurrect a cancelled exposure.
async fn run_exposure(state: Arc<DeviceState>, light: bool, gen: u64) {
    let result = run_exposure_inner(&state, light).await;
    if state.exposure_generation.load(Ordering::Acquire) != gen {
        debug!(
            ?gen,
            "exposure cancelled before completion; discarding outcome"
        );
        return;
    }
    match result {
        Ok(outcome) => {
            *state.last_image.lock().expect("last_image poisoned") = Some(outcome);
            *state.last_error.lock().expect("last_error poisoned") = None;
            state.image_ready.store(true, Ordering::Release);
        }
        Err(err) => {
            warn!(error = %err, "exposure failed");
            *state.last_error.lock().expect("last_error poisoned") = Some(err);
            // image_ready stays false
        }
    }
    state.exposure_in_flight.store(false, Ordering::Release);
}

async fn run_exposure_inner(
    state: &Arc<DeviceState>,
    light: bool,
) -> Result<ExposureOutcome, String> {
    let bx = state.bin_x.load(Ordering::Acquire);
    let by = state.bin_y.load(Ordering::Acquire);
    let nx = state.num_x.load(Ordering::Acquire);
    let ny = state.num_y.load(Ordering::Acquire);
    let sx = state.start_x.load(Ordering::Acquire);
    let sy = state.start_y.load(Ordering::Acquire);

    if !light {
        // S2: zero-filled NumX × NumY frame, no fetch.
        return Ok(ExposureOutcome {
            width: nx,
            height: ny,
            data: vec![0i32; (nx as usize) * (ny as usize)],
        });
    }

    let pointing = state.pointing.snapshot();
    let request = build_full_sensor_request(&state.config, pointing, bx, by);
    let cache_dir = &state.config.survey.cache_dir;
    let cache_key = request.cache_key();
    let (bytes, from_cache) = if let Some(b) = try_cache_load(cache_dir, &cache_key) {
        (b, true)
    } else {
        match state.survey_client.fetch(&request).await {
            Ok(b) => (b, false),
            Err(SurveyError::Timeout) => return Err("survey request timed out".into()),
            Err(SurveyError::NonSuccess(code)) => {
                return Err(format!("survey returned status {code}"))
            }
            Err(SurveyError::Http(msg)) => return Err(format!("survey HTTP error: {msg}")),
        }
    };

    let img = parse_primary_hdu(&bytes).map_err(|e| format!("FITS parse error: {e}"))?;
    // S6: only commit a network response to the cache after a
    // successful FITS parse. Otherwise a malformed body could poison
    // the cache and re-fail forever.
    if !from_cache {
        try_cache_store(cache_dir, &cache_key, &bytes);
    }
    let cropped = crop_subframe(&img.data, img.width, img.height, sx, sy, nx, ny)?;
    Ok(ExposureOutcome {
        width: nx,
        height: ny,
        data: cropped,
    })
}

pub(crate) fn crop_subframe(
    src: &[i32],
    src_w: u32,
    src_h: u32,
    sx: u32,
    sy: u32,
    nx: u32,
    ny: u32,
) -> Result<Vec<i32>, String> {
    if sx + nx > src_w || sy + ny > src_h {
        return Err(format!(
            "subframe ({sx}+{nx},{sy}+{ny}) exceeds source ({src_w},{src_h})"
        ));
    }
    if sx == 0 && sy == 0 && nx == src_w && ny == src_h {
        return Ok(src.to_vec());
    }
    let mut out = Vec::with_capacity((nx as usize) * (ny as usize));
    for row in sy..sy + ny {
        let start = (row as usize) * (src_w as usize) + sx as usize;
        let end = start + nx as usize;
        out.extend_from_slice(&src[start..end]);
    }
    Ok(out)
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
            // C2: cache_dir must be creatable AND writable. `create_
            // dir_all` succeeds on an existing read-only directory,
            // so we follow it with a probe write/delete.
            let cache_dir = &self.state.config.survey.cache_dir;
            if let Err(e) = std::fs::create_dir_all(cache_dir) {
                debug!(?cache_dir, error = %e, "cache_dir create failed");
                return Err(ASCOMError::new(
                    UNSPECIFIED_ERROR,
                    format!("cache_dir is not writable: {e}"),
                ));
            }
            let probe = cache_dir.join(".sky-survey-camera.write-probe");
            if let Err(e) = std::fs::write(&probe, b"").and_then(|_| std::fs::remove_file(&probe)) {
                debug!(?cache_dir, error = %e, "cache_dir write probe failed");
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
        if !connected {
            // C4: disconnect cancels any in-flight exposure. Bumping
            // the generation makes the spawned task discard its
            // outcome on completion.
            self.state
                .exposure_generation
                .fetch_add(1, Ordering::AcqRel);
            self.state
                .exposure_in_flight
                .store(false, Ordering::Release);
            self.state.image_ready.store(false, Ordering::Release);
            *self.state.last_image.lock().expect("last_image poisoned") = None;
            *self.state.last_error.lock().expect("last_error poisoned") = None;
        }
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

    async fn start_exposure(&self, duration: Duration, light: bool) -> ASCOMResult<()> {
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
        // E2: reject if another exposure is already in flight.
        if self
            .state
            .exposure_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ASCOMError::invalid_operation("exposure already in flight"));
        }
        // Reset readout state for the new exposure.
        self.state.image_ready.store(false, Ordering::Release);
        *self.state.last_error.lock().expect("last_error poisoned") = None;
        *self.state.last_image.lock().expect("last_image poisoned") = None;
        *self
            .state
            .last_exposure_start
            .lock()
            .expect("last_exposure_start poisoned") = Some(SystemTime::now());
        *self
            .state
            .last_exposure_duration
            .lock()
            .expect("last_exposure_duration poisoned") = Some(duration);

        // Bump the generation so any *previous* spawned task that
        // races to completion is ignored, and capture the new
        // generation for *this* task to honour at finish time.
        let gen = self
            .state
            .exposure_generation
            .fetch_add(1, Ordering::AcqRel)
            + 1;
        debug!(?duration, light, gen, "exposure started");
        let state = Arc::clone(&self.state);
        tokio::spawn(run_exposure(state, light, gen));
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
        // Bump the generation so the in-flight task discards its
        // outcome. The actual fetch task can't always be cancelled at
        // the OS level (e.g. a Hold stub keeps the connection open
        // until process exit) but it can no longer publish results.
        // A1 ("ImageReady is false") holds.
        self.state
            .exposure_generation
            .fetch_add(1, Ordering::AcqRel);
        self.state.image_ready.store(false, Ordering::Release);
        *self.state.last_error.lock().expect("last_error poisoned") = None;
        *self.state.last_image.lock().expect("last_image poisoned") = None;
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
        self.state
            .exposure_generation
            .fetch_add(1, Ordering::AcqRel);
        self.state.image_ready.store(false, Ordering::Release);
        *self.state.last_error.lock().expect("last_error poisoned") = None;
        *self.state.last_image.lock().expect("last_image poisoned") = None;
        Ok(())
    }

    async fn image_ready(&self) -> ASCOMResult<bool> {
        Ok(self.state.image_ready.load(Ordering::Acquire))
    }

    async fn last_exposure_start_time(&self) -> ASCOMResult<SystemTime> {
        self.state
            .last_exposure_start
            .lock()
            .expect("last_exposure_start poisoned")
            .ok_or_else(|| ASCOMError::invalid_operation("no exposure has started yet"))
    }

    async fn last_exposure_duration(&self) -> ASCOMResult<Duration> {
        self.state
            .last_exposure_duration
            .lock()
            .expect("last_exposure_duration poisoned")
            .ok_or_else(|| ASCOMError::invalid_operation("no exposure has started yet"))
    }

    async fn image_array(&self) -> ASCOMResult<ImageArray> {
        // S4-S6: a stored fetch error becomes ASCOM UNSPECIFIED_ERROR.
        if let Some(msg) = self
            .state
            .last_error
            .lock()
            .expect("last_error poisoned")
            .clone()
        {
            return Err(ASCOMError::new(UNSPECIFIED_ERROR, msg));
        }
        if !self.state.image_ready.load(Ordering::Acquire) {
            return Err(ASCOMError::invalid_operation("no image is ready"));
        }
        let guard = self.state.last_image.lock().expect("last_image poisoned");
        let outcome = guard
            .as_ref()
            .expect("image_ready=true but no stored image");
        let array = Array2::from_shape_vec(
            (outcome.height as usize, outcome.width as usize),
            outcome.data.clone(),
        )
        .map_err(|e| ASCOMError::new(UNSPECIFIED_ERROR, format!("ndarray shape: {e}")))?;
        Ok(ImageArray::from(array))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DeviceConfig, OpticsConfig, PointingConfig, ServerConfig, SurveyConfig};

    fn fake_config() -> Config {
        Config {
            device: DeviceConfig {
                name: "Test".into(),
                unique_id: "uid-001".into(),
                description: "test".into(),
            },
            optics: OpticsConfig {
                focal_length_mm: 1000.0,
                pixel_size_x_um: 3.76,
                pixel_size_y_um: 3.76,
                sensor_width_px: 640,
                sensor_height_px: 480,
            },
            pointing: PointingConfig {
                initial_ra_deg: 0.0,
                initial_dec_deg: 0.0,
                initial_rotation_deg: 0.0,
            },
            survey: SurveyConfig {
                name: "DSS2 Red".into(),
                request_timeout: Duration::from_secs(5),
                cache_dir: std::env::temp_dir().join("sky-survey-camera-tests"),
                endpoint: "http://placeholder/".into(),
            },
            server: ServerConfig {
                port: 0,
                device_number: 0,
            },
        }
    }

    #[test]
    fn build_full_sensor_request_uses_full_sensor_fov() {
        let cfg = fake_config();
        let pointing = PointingState::new(10.0, 20.0, 0.0);
        let req = build_full_sensor_request(&cfg, pointing, 1, 1);
        assert_eq!(req.pixels_x, 640);
        assert_eq!(req.pixels_y, 480);
        assert!(req.size_x_deg > 0.1 && req.size_x_deg < 1.0);
    }

    #[test]
    fn build_full_sensor_request_halves_pixels_when_binned() {
        let cfg = fake_config();
        let pointing = PointingState::new(0.0, 0.0, 0.0);
        let req = build_full_sensor_request(&cfg, pointing, 2, 2);
        assert_eq!(req.pixels_x, 320);
        assert_eq!(req.pixels_y, 240);
    }

    #[test]
    fn crop_subframe_full_frame_is_passthrough() {
        let src: Vec<i32> = (0..12).collect();
        let out = crop_subframe(&src, 4, 3, 0, 0, 4, 3).unwrap();
        assert_eq!(out, src);
    }

    #[test]
    fn crop_subframe_central_window() {
        // 4x3 source = [[0,1,2,3],[4,5,6,7],[8,9,10,11]]
        // Crop StartX=1, StartY=1, NumX=2, NumY=2 → [[5,6],[9,10]]
        let src: Vec<i32> = (0..12).collect();
        let out = crop_subframe(&src, 4, 3, 1, 1, 2, 2).unwrap();
        assert_eq!(out, vec![5, 6, 9, 10]);
    }

    #[test]
    fn crop_subframe_rejects_out_of_bounds() {
        let src: Vec<i32> = vec![0; 12];
        crop_subframe(&src, 4, 3, 3, 0, 2, 1).unwrap_err();
        crop_subframe(&src, 4, 3, 0, 2, 1, 2).unwrap_err();
    }
}
