//! `SvbonyCamera` — the ASCOM `Device` + `Camera` implementation over the
//! [`CameraHandle`](crate::backend::CameraHandle) seam.
//!
//! Behaviour follows `docs/services/svbony-camera.md`'s "Behavioral
//! contracts", with the load-bearing divergence from the `zwo-camera`
//! template being the exposure path: SVBony has no snap-exposure API, so
//! every exposure rides the soft-trigger video-capture state machine (mode
//! selection + video-capture start once at connect; each `StartExposure` is
//! set-exposure → soft-trigger → `SVBGetVideoData` with a deadline) instead
//! of ZWO's `ASIStartExposure`/`ASIStopExposure` snap model. Consequences:
//! - **No data-preserving stop**: `CanStopExposure = false`, `StopExposure`
//!   is `NOT_IMPLEMENTED` unconditionally (E8) — the opposite of
//!   `zwo-camera`'s graceful stop.
//! - **`AbortExposure` never touches the SDK**: see `backend.rs`'s module
//!   docs for why — it only bumps the exposure generation counter so a
//!   late-completing capture's result is discarded (E7).
//! - **Dark frames are accepted** on every model — there is no mechanical
//!   shutter in video mode (`HasShutter = false`), so `Light = false`
//!   captures identically (E4/E7).
//! - **`ElectronsPerADU`** is a permanent `NOT_IMPLEMENTED` placeholder (ST2)
//!   — `SVB_CAMERA_PROPERTY` carries no native electrons-per-ADU field.
//! - Sensor/capability data (`SVB_CAMERA_PROPERTY`/`_EX`, pixel size) is
//!   readable only once the camera is **open**, unlike ZWO where the SDK
//!   hands back full info as part of `CameraInfo`. This device therefore
//!   caches it in [`DeviceState`] at the connect handshake, not at
//!   construction time.
//!
//! Blocking capture SDK calls run on `spawn_blocking` inside a detached
//! task; a generation counter lets abort/disconnect invalidate a
//! late-completing task — the same discipline `zwo-camera`'s
//! `run_exposure`/`result_lock` pattern uses.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ascom_alpaca::api::camera::{CameraState, GuideDirection, ImageArray, SensorType};
use ascom_alpaca::api::{Camera, Device};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use ndarray::Array2;
use parking_lot::Mutex;
use svbony_rs::{BayerPattern, CameraInfo, ControlType};
use tracing::{debug, warn};

use crate::backend::{CameraHandle, CaptureRequest};
use crate::config::DeviceOverride;
use crate::config_actions::SvbonyCameraDriver;
use rusty_photon_driver::ConfigActionCtx;

/// 0x500 — driver-specific catch-all for an asynchronous capture failure
/// surfaced lazily via `image_array` (E9).
const UNSPECIFIED_ERROR: ASCOMErrorCode = ASCOMErrorCode::new_for_driver(0);

/// `SVB_EXPOSURE`'s assumed unit is microseconds, so the smallest step is 1 µs
/// (see `svbony_rs::ControlType::Exposure`'s doc comment for the unit
/// caveat, to be confirmed against real hardware).
const EXPOSURE_RESOLUTION: Duration = Duration::from_micros(1);

/// The driver's named readout-mode list — a cosmetic, cached label (RM1:
/// switching validates the index and updates cached state only) mirroring
/// the two acquisition modes the exposure state machine uses internally
/// (`SVB_MODE_TRIG_SOFT` vs `SVB_MODE_NORMAL`); exact names are a Phase E
/// choice, not an SDK-driven list (`docs/services/svbony-camera.md`'s
/// "Gain / offset / readout" contract, RM1).
const READOUT_MODES: [&str; 2] = ["SoftTrigger", "FreeRunning"];

/// A region of interest in *binned* pixel coordinates.
#[derive(Debug, Clone, Copy)]
struct Roi {
    start_x: u32,
    start_y: u32,
    width: u32,
    height: u32,
}

/// Sensor geometry and capability data cached from `SVB_CAMERA_PROPERTY`/
/// `_EX` and `SVBGetSensorPixelSize` at the connect handshake — unlike
/// `zwo-camera`, these are **not** available at construction time because
/// SVBony's SDK only returns them for an *open* camera (see the module
/// docs).
#[derive(Debug, Clone)]
struct SensorInfo {
    max_width: u32,
    max_height: u32,
    is_color: bool,
    bayer_pattern: BayerPattern,
    supported_bins: Vec<u32>,
    /// `(2^max_bit_depth) - 1` precomputed for `MaxADU` (ST3).
    max_adu: u32,
    pixel_size_um: f32,
    is_trigger_cam: bool,
    supports_control_temp: bool,
    supports_pulse_guide: bool,
}

/// Per-device runtime state: the connect-time property cache plus the
/// exposure state machine. Atomics for the hot/simple flags;
/// `parking_lot::Mutex` for the `Option<…>` caches and the captured image.
/// Locks are never held across an `await`.
#[derive(Debug)]
struct DeviceState {
    sensor: Mutex<Option<SensorInfo>>,

    /// Current symmetric bin (init 1).
    bin: AtomicU8,
    /// Current readout-mode index into [`READOUT_MODES`].
    readout_mode: AtomicU8,
    /// Intended ROI in *binned* pixel coordinates (rescaled on bin change).
    intended_roi: Mutex<Option<Roi>>,
    /// `(min, max)` exposure microseconds from `SVBGetControlCaps(SVB_EXPOSURE)`.
    exposure_range_us: Mutex<Option<(i64, i64)>>,
    gain_min_max: Mutex<Option<(i64, i64)>>,
    offset_min_max: Mutex<Option<(i64, i64)>>,
    target_temperature: Mutex<Option<f64>>,

    exposure_in_flight: AtomicBool,
    image_ready: AtomicBool,
    /// Set by `cancel_exposure` (abort or disconnect) and cleared by the next
    /// `start_exposure`/reconnect. `exposure_in_flight` itself deliberately
    /// stays `true` until the still-running, un-interruptible capture task
    /// drains (see `cancel_exposure`'s doc comment) — but `CameraState`/
    /// `PercentCompleted` must not keep reporting `Exposing`/a climbing
    /// percentage for that whole window just because the SDK can't be
    /// interrupted; this flag lets them report the operator's requested
    /// state (aborted → idle, not still exposing) promptly instead.
    aborted: AtomicBool,
    /// Bumped on each start / abort / disconnect so a late-completing capture
    /// task can tell it has been superseded and discard its result.
    exposure_generation: AtomicU64,
    last_exposure_start_time: Mutex<Option<SystemTime>>,
    last_exposure_duration: Mutex<Option<Duration>>,
    last_image: Mutex<Option<ImageArray>>,
    /// Set on a mid-exposure SDK failure or an exceeded `SVBGetVideoData`
    /// deadline → `CameraState::Error` (E9).
    last_error: Mutex<Option<String>>,
    /// Serializes the capture task's "check generation + commit result"
    /// against `cancel_exposure`'s "bump generation + clear image_ready".
    result_lock: Mutex<()>,

    /// True only for the duration of a blocking `PulseGuide` SDK call (v0
    /// keeps `PulseGuide` synchronous — see `pulse_guide`'s doc comment).
    pulse_guiding: AtomicBool,
}

impl DeviceState {
    fn new() -> Self {
        Self {
            sensor: Mutex::new(None),
            bin: AtomicU8::new(1),
            readout_mode: AtomicU8::new(0),
            intended_roi: Mutex::new(None),
            exposure_range_us: Mutex::new(None),
            gain_min_max: Mutex::new(None),
            offset_min_max: Mutex::new(None),
            target_temperature: Mutex::new(None),
            exposure_in_flight: AtomicBool::new(false),
            image_ready: AtomicBool::new(false),
            aborted: AtomicBool::new(false),
            exposure_generation: AtomicU64::new(0),
            last_exposure_start_time: Mutex::new(None),
            last_exposure_duration: Mutex::new(None),
            last_image: Mutex::new(None),
            last_error: Mutex::new(None),
            result_lock: Mutex::new(()),
            pulse_guiding: AtomicBool::new(false),
        }
    }

    /// Reset the exposure state machine to a clean idle state. Called on
    /// connect so a stale `Error` / `ImageReady` / image from a previous
    /// session does not survive a reconnect (C3).
    fn reset_exposure_state(&self) {
        let _guard = self.result_lock.lock();
        self.exposure_generation.fetch_add(1, Ordering::AcqRel);
        self.exposure_in_flight.store(false, Ordering::Release);
        self.image_ready.store(false, Ordering::Release);
        self.aborted.store(false, Ordering::Release);
        *self.last_image.lock() = None;
        *self.last_error.lock() = None;
        *self.last_exposure_start_time.lock() = None;
        *self.last_exposure_duration.lock() = None;
        self.pulse_guiding.store(false, Ordering::Release);
    }
}

/// One ASCOM Camera device per discovered SVBony camera.
#[derive(Clone, derive_more::Debug)]
pub struct SvbonyCamera {
    #[debug(skip)]
    handle: Arc<dyn CameraHandle>,
    info: CameraInfo,
    unique_id: String,
    name: String,
    description: String,
    state: Arc<DeviceState>,
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
            state: Arc::new(DeviceState::new()),
            config_ctx: None,
        }
    }

    /// Attach config-action wiring (enables `config.get`/`apply`/`schema`).
    #[must_use]
    pub fn with_config_actions(mut self, ctx: ConfigActionCtx<SvbonyCameraDriver>) -> Self {
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

    /// The connect-time sensor property cache. `ensure_connected` should
    /// already have been checked by the caller; `NOT_CONNECTED` here is a
    /// defensive fallback for the race between a connected handle and a
    /// not-yet-populated cache.
    fn sensor(&self) -> ASCOMResult<SensorInfo> {
        (*self.state.sensor.lock())
            .clone()
            .ok_or(ASCOMError::NOT_CONNECTED)
    }

    fn connect(&self) -> ASCOMResult<()> {
        self.handle.open().map_err(|_| ASCOMError::NOT_CONNECTED)?;
        // A failed post-open handshake must leave the device disconnected
        // (C2), not opened-but-unusable, so close before propagating.
        if let Err(e) = self.open_handshake() {
            if let Err(close_err) = self.handle.close() {
                debug!(error = %close_err, "close after a failed connect handshake also failed");
            }
            return Err(e);
        }
        // A reconnect must not surface a previous session's Error /
        // ImageReady / stale frame (C3).
        self.state.reset_exposure_state();
        debug!(camera = %self.unique_id, "camera connected");
        Ok(())
    }

    /// Read and cache the camera's properties/controls after `open()`, then
    /// run the exposure state machine's connect-time step for trigger
    /// cameras (mode selection + video-capture start — never for a
    /// non-trigger camera, see this method's body — per
    /// `docs/services/svbony-camera.md` "Behavioral contracts → Exposure"
    /// step 1). Tenet 3 (K5): this method never
    /// touches `SVB_COOLER_ENABLE`/`SVB_TARGET_TEMPERATURE` — cooling is
    /// engaged only by an explicit operator `CoolerOn`/`SetCCDTemperature`
    /// call, never here.
    fn open_handshake(&self) -> ASCOMResult<()> {
        let property = self
            .handle
            .property()
            .map_err(|_| ASCOMError::NOT_CONNECTED)?;
        let property_ex = self
            .handle
            .property_ex()
            .map_err(|_| ASCOMError::NOT_CONNECTED)?;
        let pixel_size_um = self
            .handle
            .pixel_size_microns()
            .map_err(|_| ASCOMError::NOT_CONNECTED)?;

        let caps = self
            .handle
            .control_caps()
            .map_err(|_| ASCOMError::NOT_CONNECTED)?;
        let find = |ct: ControlType| caps.iter().find(|c| c.control_type == ct);

        let exposure = find(ControlType::Exposure).ok_or_else(|| {
            warn!("camera does not advertise an exposure control");
            ASCOMError::NOT_CONNECTED
        })?;
        *self.state.exposure_range_us.lock() = Some((exposure.min, exposure.max));
        *self.state.gain_min_max.lock() = find(ControlType::Gain).map(|c| (c.min, c.max));
        *self.state.offset_min_max.lock() = find(ControlType::BlackLevel).map(|c| (c.min, c.max));

        self.state.bin.store(1, Ordering::Release);
        self.state.readout_mode.store(0, Ordering::Release);
        let max_width = u32::try_from(property.max_width).unwrap_or(0);
        let max_height = u32::try_from(property.max_height).unwrap_or(0);
        *self.state.intended_roi.lock() = Some(Roi {
            start_x: 0,
            start_y: 0,
            width: max_width,
            height: max_height,
        });
        *self.state.target_temperature.lock() = None;

        *self.state.sensor.lock() = Some(SensorInfo {
            max_width,
            max_height,
            is_color: property.is_color,
            bayer_pattern: property.bayer_pattern,
            supported_bins: property.supported_bins.clone(),
            max_adu: max_adu_from_bit_depth(u32::try_from(property.max_bit_depth).unwrap_or(0)),
            pixel_size_um,
            is_trigger_cam: property.is_trigger_cam,
            supports_control_temp: property_ex.supports_control_temp,
            supports_pulse_guide: property_ex.supports_pulse_guide,
        });

        // State-machine step 1, trigger cameras only (tenet 3): select
        // `SVB_MODE_TRIG_SOFT` and arm video capture once, here, never
        // repeated per-exposure. A trigger-gated capture produces no frames
        // — and therefore does not physically actuate the imaging chain —
        // until an operator's `StartExposure` sends the soft trigger, so
        // this is a read of camera-mode capability plus an armed-but-idle
        // mode-select, not actuation (see the design doc's exposure contract
        // point 1). A **non**-trigger camera has no such gate: its only mode
        // is free-running `SVB_MODE_NORMAL`, so starting video capture here
        // would begin the sensor continuously integrating and streaming
        // frames as a side effect of connecting — genuine actuation with no
        // operator action, which tenet 3 bans outright. So for non-trigger
        // cameras, video capture is left unarmed at connect; `capture`'s
        // non-trigger fallback (state-machine step 5) already
        // stops-then-starts it fresh on every operator-initiated
        // `StartExposure`, which is where a non-trigger camera's capture
        // must first arm.
        if property.is_trigger_cam {
            self.handle
                .set_camera_mode(svbony_rs::CameraMode::TrigSoft)
                .map_err(|_| ASCOMError::NOT_CONNECTED)?;
            self.handle
                .start_video_capture()
                .map_err(|_| ASCOMError::NOT_CONNECTED)?;
        }

        Ok(())
    }

    fn disconnect(&self) -> ASCOMResult<()> {
        // An in-flight exposure is cancelled (C3) before the handle closes.
        self.cancel_exposure();
        self.handle.close().map_err(|_| ASCOMError::NOT_CONNECTED)?;
        debug!(camera = %self.unique_id, "camera disconnected");
        Ok(())
    }

    /// Cancel any in-flight exposure (abort): bump the generation so the
    /// capture task discards its result, clear `image_ready`/`last_error`,
    /// and set `aborted` so `CameraState`/`PercentCompleted` promptly report
    /// idle instead of a still-running exposure (see `DeviceState::aborted`'s
    /// doc comment). Deliberately does NOT clear `exposure_in_flight` — the
    /// capture task clears that once its blocking SDK chain drains, so a new
    /// exposure cannot race the still-running one (the design's "one owner
    /// per device"). Unlike `zwo-camera`, this never signals the SDK — see
    /// `backend.rs`'s module docs for why `capture` has no interrupt path.
    fn cancel_exposure(&self) {
        if !self.state.exposure_in_flight.load(Ordering::Acquire) {
            return;
        }
        // Atomic with the capture task's commit so an abort can never be
        // overwritten by a just-completing capture.
        let _guard = self.state.result_lock.lock();
        self.state
            .exposure_generation
            .fetch_add(1, Ordering::AcqRel);
        self.state.image_ready.store(false, Ordering::Release);
        self.state.aborted.store(true, Ordering::Release);
        *self.state.last_error.lock() = None;
    }

    /// Validate the cached ROI against the binned sensor geometry (R2/R3),
    /// returning the [`Roi`] to push to the SDK.
    fn validated_geometry(&self, sensor: &SensorInfo, bin: u32) -> ASCOMResult<Roi> {
        let roi = (*self.state.intended_roi.lock())
            .ok_or_else(|| ASCOMError::invalid_value("no ROI defined for camera"))?;
        check_geometry(roi, sensor.max_width, sensor.max_height, bin)?;
        Ok(roi)
    }

    fn gain_available(&self) -> bool {
        self.state.gain_min_max.lock().is_some()
    }

    fn offset_available(&self) -> bool {
        self.state.offset_min_max.lock().is_some()
    }

    /// Run a blocking SDK-seam call off the async executor. The SVBony FFI
    /// calls do USB I/O, so running them directly on a Tokio worker could
    /// stall other Alpaca requests; offload them like the capture, connect,
    /// and pulse-guide paths.
    async fn on_handle<T, F>(&self, f: F) -> ASCOMResult<T>
    where
        F: FnOnce(&dyn CameraHandle) -> ASCOMResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let handle = Arc::clone(&self.handle);
        tokio::task::spawn_blocking(move || f(handle.as_ref()))
            .await
            .map_err(|e| ASCOMError::invalid_operation(format!("SDK task failed: {e}")))?
    }
}

/// Geometry validation (R2/R3). Rejects a zero, misaligned, or out-of-bounds
/// sub-frame. SVBony's `SVBSetROIFormat` requires `width % 8 == 0` and
/// `height % 2 == 0` — byte-for-byte the same rule `zwo-camera` enforces for
/// ASI.
fn check_geometry(roi: Roi, sensor_w: u32, sensor_h: u32, bin: u32) -> ASCOMResult<()> {
    if roi.width == 0 || roi.height == 0 {
        return Err(ASCOMError::invalid_value(
            "NumX and NumY must be greater than 0",
        ));
    }
    if !roi.width.is_multiple_of(8) || !roi.height.is_multiple_of(2) {
        return Err(ASCOMError::invalid_value(
            "SVBony requires NumX a multiple of 8 and NumY a multiple of 2",
        ));
    }
    let max_x = sensor_w / bin;
    let max_y = sensor_h / bin;
    if roi.start_x.saturating_add(roi.width) > max_x {
        return Err(ASCOMError::invalid_value(
            "StartX + NumX exceeds CameraXSize / BinX",
        ));
    }
    if roi.start_y.saturating_add(roi.height) > max_y {
        return Err(ASCOMError::invalid_value(
            "StartY + NumY exceeds CameraYSize / BinY",
        ));
    }
    Ok(())
}

/// Rescale a ROI (binned coords) by the `old/new` bin ratio (B3).
fn rescale_roi(roi: Roi, old: u8, new: u8) -> Roi {
    let factor = f64::from(old) / f64::from(new);
    Roi {
        start_x: (f64::from(roi.start_x) * factor) as u32,
        start_y: (f64::from(roi.start_y) * factor) as u32,
        width: (f64::from(roi.width) * factor) as u32,
        height: (f64::from(roi.height) * factor) as u32,
    }
}

/// `MaxADU = 2^bit_depth - 1` (16383 for the SV605CC's 14-bit ADC),
/// saturating (ST3).
fn max_adu_from_bit_depth(bit_depth: u32) -> u32 {
    1u32.checked_shl(bit_depth).map_or(u32::MAX, |v| v - 1)
}

/// Bayer pattern → ASCOM `BayerOffsetX/Y` (ST1).
fn bayer_offsets(pattern: BayerPattern) -> (u8, u8) {
    match pattern {
        BayerPattern::Rg => (0, 0),
        BayerPattern::Bg => (1, 1),
        BayerPattern::Gr => (1, 0),
        BayerPattern::Gb => (0, 1),
    }
}

/// Map an ASCOM guide direction onto the `svbony-rs` one.
fn guide_direction(direction: GuideDirection) -> svbony_rs::GuideDirection {
    match direction {
        GuideDirection::North => svbony_rs::GuideDirection::North,
        GuideDirection::South => svbony_rs::GuideDirection::South,
        GuideDirection::East => svbony_rs::GuideDirection::East,
        GuideDirection::West => svbony_rs::GuideDirection::West,
    }
}

/// Convert a single-plane Raw16 frame into an ASCOM `ImageArray` with `[x][y]`
/// axis order (ASCOM stores width-major).
fn to_image_array(bytes: &[u8], width: u32, height: u32) -> Result<ImageArray, String> {
    let (w, h) = (width as usize, height as usize);
    let needed = w * h * 2;
    if bytes.len() < needed {
        return Err("16-bit buffer too small for frame".to_string());
    }
    let pixels: Vec<u16> = bytes[..needed]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_ne_bytes(*c))
        .collect();
    let arr = Array2::from_shape_vec((h, w), pixels).map_err(|e| e.to_string())?;
    Ok(ImageArray::from(arr.reversed_axes()))
}

/// The detached capture task: runs the blocking soft-trigger SDK chain *and*
/// the CPU-heavy frame transform on `spawn_blocking`, then stores the image
/// (or records a failure as the `Error` state, E9) — unless a newer
/// generation has superseded it (an abort or disconnect).
///
/// Both the SDK download and [`to_image_array`] run inside the one
/// `spawn_blocking` closure on purpose — see `zwo-camera`'s equivalent
/// `run_exposure` doc comment for the full rationale (a full-frame transform
/// is CPU-heavy enough in an unoptimised build to matter, and running it
/// while holding `result_lock` would contend `cancel_exposure`).
async fn run_exposure(
    handle: Arc<dyn CameraHandle>,
    state: Arc<DeviceState>,
    generation: u64,
    request: CaptureRequest,
) {
    let blocking_handle = Arc::clone(&handle);
    let (width, height) = (request.width, request.height);
    let result = tokio::task::spawn_blocking(move || {
        blocking_handle
            .capture(request)
            .map(|bytes| to_image_array(&bytes, width, height))
    })
    .await;

    {
        // No await is held across the lock (the blocking await is above), so
        // this "check generation + record" is atomic against
        // cancel_exposure. Only the cheap commit runs here — the transform
        // already happened off-thread.
        let _guard = state.result_lock.lock();
        if state.exposure_generation.load(Ordering::Acquire) == generation {
            match result {
                Ok(Ok(Ok(array))) => {
                    *state.last_image.lock() = Some(array);
                    *state.last_error.lock() = None;
                    state.image_ready.store(true, Ordering::Release);
                }
                Ok(Ok(Err(e))) => {
                    warn!(error = %e, "failed to transform captured image");
                    *state.last_image.lock() = None;
                    *state.last_error.lock() = Some(format!("image transform failed: {e}"));
                }
                Ok(Err(e)) => {
                    warn!(error = %e.0, "mid-exposure SDK error or SVBGetVideoData deadline exceeded");
                    *state.last_error.lock() = Some(e.0);
                }
                Err(join_err) => {
                    warn!(error = %join_err, "exposure task panicked");
                    *state.last_error.lock() = Some(format!("exposure task failed: {join_err}"));
                }
            }
        }
    }
    state.exposure_in_flight.store(false, Ordering::Release);
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
        // `connect`/`disconnect` do blocking SDK I/O, so offload off the
        // executor (SvbonyCamera is cheap to clone: it is `Arc`-backed).
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

#[async_trait::async_trait]
impl Camera for SvbonyCamera {
    // --- geometry ---------------------------------------------------------------

    async fn camera_x_size(&self) -> ASCOMResult<u32> {
        self.ensure_connected()?;
        Ok(self.sensor()?.max_width)
    }

    async fn camera_y_size(&self) -> ASCOMResult<u32> {
        self.ensure_connected()?;
        Ok(self.sensor()?.max_height)
    }

    async fn pixel_size_x(&self) -> ASCOMResult<f64> {
        self.ensure_connected()?;
        Ok(f64::from(self.sensor()?.pixel_size_um))
    }

    async fn pixel_size_y(&self) -> ASCOMResult<f64> {
        // SVBony exposes a single pixel size, so X == Y trivially.
        self.pixel_size_x().await
    }

    async fn max_adu(&self) -> ASCOMResult<u32> {
        self.ensure_connected()?;
        Ok(self.sensor()?.max_adu)
    }

    async fn electrons_per_adu(&self) -> ASCOMResult<f64> {
        // ST2: permanent NOT_IMPLEMENTED placeholder — SVB_CAMERA_PROPERTY
        // carries no native electrons-per-ADU field (unlike ZWO's ElecPerADU).
        self.ensure_connected()?;
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn sensor_name(&self) -> ASCOMResult<String> {
        self.ensure_connected()?;
        Ok(self.info.friendly_name.clone())
    }

    // --- binning ----------------------------------------------------------------

    async fn bin_x(&self) -> ASCOMResult<u8> {
        self.ensure_connected()?;
        Ok(self.state.bin.load(Ordering::Acquire))
    }

    async fn bin_y(&self) -> ASCOMResult<u8> {
        self.bin_x().await
    }

    async fn set_bin_x(&self, bin_x: u8) -> ASCOMResult<()> {
        self.ensure_connected()?;
        let sensor = self.sensor()?;
        if !sensor.supported_bins.contains(&u32::from(bin_x)) {
            return Err(ASCOMError::invalid_value(format!(
                "bin {bin_x} is not a supported binning mode"
            )));
        }
        let old = self.state.bin.load(Ordering::Acquire);
        if old == bin_x {
            return Ok(());
        }
        {
            let mut roi = self.state.intended_roi.lock();
            if let Some(area) = *roi {
                *roi = Some(rescale_roi(area, old, bin_x));
            }
        }
        self.state.bin.store(bin_x, Ordering::Release);
        Ok(())
    }

    async fn set_bin_y(&self, bin_y: u8) -> ASCOMResult<()> {
        self.set_bin_x(bin_y).await
    }

    async fn max_bin_x(&self) -> ASCOMResult<u8> {
        self.ensure_connected()?;
        self.sensor()?
            .supported_bins
            .iter()
            .copied()
            .max()
            .and_then(|m| u8::try_from(m).ok())
            .ok_or_else(|| ASCOMError::invalid_operation("no valid binning modes"))
    }

    async fn max_bin_y(&self) -> ASCOMResult<u8> {
        self.max_bin_x().await
    }

    async fn can_asymmetric_bin(&self) -> ASCOMResult<bool> {
        Ok(false)
    }

    // --- ROI (relaxed setters; validated at start_exposure) ---------------------

    async fn num_x(&self) -> ASCOMResult<u32> {
        self.ensure_connected()?;
        (*self.state.intended_roi.lock())
            .map(|r| r.width)
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn num_y(&self) -> ASCOMResult<u32> {
        self.ensure_connected()?;
        (*self.state.intended_roi.lock())
            .map(|r| r.height)
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn start_x(&self) -> ASCOMResult<u32> {
        self.ensure_connected()?;
        (*self.state.intended_roi.lock())
            .map(|r| r.start_x)
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn start_y(&self) -> ASCOMResult<u32> {
        self.ensure_connected()?;
        (*self.state.intended_roi.lock())
            .map(|r| r.start_y)
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn set_num_x(&self, num_x: u32) -> ASCOMResult<()> {
        self.ensure_connected()?;
        let mut roi = self.state.intended_roi.lock();
        match *roi {
            Some(area) => {
                *roi = Some(Roi {
                    width: num_x,
                    ..area
                });
                Ok(())
            }
            None => Err(ASCOMError::INVALID_VALUE),
        }
    }

    async fn set_num_y(&self, num_y: u32) -> ASCOMResult<()> {
        self.ensure_connected()?;
        let mut roi = self.state.intended_roi.lock();
        match *roi {
            Some(area) => {
                *roi = Some(Roi {
                    height: num_y,
                    ..area
                });
                Ok(())
            }
            None => Err(ASCOMError::INVALID_VALUE),
        }
    }

    async fn set_start_x(&self, start_x: u32) -> ASCOMResult<()> {
        self.ensure_connected()?;
        let mut roi = self.state.intended_roi.lock();
        match *roi {
            Some(area) => {
                *roi = Some(Roi { start_x, ..area });
                Ok(())
            }
            None => Err(ASCOMError::INVALID_VALUE),
        }
    }

    async fn set_start_y(&self, start_y: u32) -> ASCOMResult<()> {
        self.ensure_connected()?;
        let mut roi = self.state.intended_roi.lock();
        match *roi {
            Some(area) => {
                *roi = Some(Roi { start_y, ..area });
                Ok(())
            }
            None => Err(ASCOMError::INVALID_VALUE),
        }
    }

    // --- exposure range ---------------------------------------------------------

    async fn exposure_min(&self) -> ASCOMResult<Duration> {
        self.ensure_connected()?;
        let (min, _) = (*self.state.exposure_range_us.lock()).ok_or(ASCOMError::INVALID_VALUE)?;
        Ok(Duration::from_micros(min.max(0) as u64))
    }

    async fn exposure_max(&self) -> ASCOMResult<Duration> {
        self.ensure_connected()?;
        let (_, max) = (*self.state.exposure_range_us.lock()).ok_or(ASCOMError::INVALID_VALUE)?;
        Ok(Duration::from_micros(max.max(0) as u64))
    }

    async fn exposure_resolution(&self) -> ASCOMResult<Duration> {
        self.ensure_connected()?;
        Ok(EXPOSURE_RESOLUTION)
    }

    // --- gain / offset ------------------------------------------------------------

    async fn gain(&self) -> ASCOMResult<i32> {
        self.ensure_connected()?;
        if !self.gain_available() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        self.on_handle(|h| {
            h.control_value(ControlType::Gain)
                .map(|g| g as i32)
                .map_err(|_| ASCOMError::INVALID_OPERATION)
        })
        .await
    }

    async fn gain_min(&self) -> ASCOMResult<i32> {
        self.ensure_connected()?;
        (*self.state.gain_min_max.lock())
            .map(|(min, _)| min as i32)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn gain_max(&self) -> ASCOMResult<i32> {
        self.ensure_connected()?;
        (*self.state.gain_min_max.lock())
            .map(|(_, max)| max as i32)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn set_gain(&self, gain: i32) -> ASCOMResult<()> {
        self.ensure_connected()?;
        let (min, max) = (*self.state.gain_min_max.lock()).ok_or(ASCOMError::NOT_IMPLEMENTED)?;
        if i64::from(gain) < min || i64::from(gain) > max {
            return Err(ASCOMError::invalid_value(format!(
                "gain {gain} outside [{min}, {max}]"
            )));
        }
        self.on_handle(move |h| {
            h.set_control_value(ControlType::Gain, i64::from(gain))
                .map_err(|_| ASCOMError::INVALID_OPERATION)
        })
        .await
    }

    async fn offset(&self) -> ASCOMResult<i32> {
        self.ensure_connected()?;
        if !self.offset_available() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        self.on_handle(|h| {
            h.control_value(ControlType::BlackLevel)
                .map(|o| o as i32)
                .map_err(|_| ASCOMError::INVALID_OPERATION)
        })
        .await
    }

    async fn offset_min(&self) -> ASCOMResult<i32> {
        self.ensure_connected()?;
        (*self.state.offset_min_max.lock())
            .map(|(min, _)| min as i32)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn offset_max(&self) -> ASCOMResult<i32> {
        self.ensure_connected()?;
        (*self.state.offset_min_max.lock())
            .map(|(_, max)| max as i32)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn set_offset(&self, offset: i32) -> ASCOMResult<()> {
        self.ensure_connected()?;
        let (min, max) = (*self.state.offset_min_max.lock()).ok_or(ASCOMError::NOT_IMPLEMENTED)?;
        if i64::from(offset) < min || i64::from(offset) > max {
            return Err(ASCOMError::invalid_value(format!(
                "offset {offset} outside [{min}, {max}]"
            )));
        }
        self.on_handle(move |h| {
            h.set_control_value(ControlType::BlackLevel, i64::from(offset))
                .map_err(|_| ASCOMError::INVALID_OPERATION)
        })
        .await
    }

    // --- readout modes ------------------------------------------------------------

    async fn readout_mode(&self) -> ASCOMResult<usize> {
        self.ensure_connected()?;
        Ok(usize::from(self.state.readout_mode.load(Ordering::Acquire)))
    }

    async fn readout_modes(&self) -> ASCOMResult<Vec<String>> {
        self.ensure_connected()?;
        Ok(READOUT_MODES.iter().map(|s| (*s).to_string()).collect())
    }

    async fn set_readout_mode(&self, readout_mode: usize) -> ASCOMResult<()> {
        self.ensure_connected()?;
        if readout_mode >= READOUT_MODES.len() {
            return Err(ASCOMError::invalid_value(format!(
                "readout mode {readout_mode} out of range (0..{})",
                READOUT_MODES.len()
            )));
        }
        self.state
            .readout_mode
            .store(readout_mode as u8, Ordering::Release);
        Ok(())
    }

    // --- sensor type / bayer -------------------------------------------------------

    async fn sensor_type(&self) -> ASCOMResult<SensorType> {
        self.ensure_connected()?;
        Ok(if self.sensor()?.is_color {
            SensorType::RGGB
        } else {
            SensorType::Monochrome
        })
    }

    async fn bayer_offset_x(&self) -> ASCOMResult<u8> {
        self.ensure_connected()?;
        let sensor = self.sensor()?;
        if !sensor.is_color {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        Ok(bayer_offsets(sensor.bayer_pattern).0)
    }

    async fn bayer_offset_y(&self) -> ASCOMResult<u8> {
        self.ensure_connected()?;
        let sensor = self.sensor()?;
        if !sensor.is_color {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        Ok(bayer_offsets(sensor.bayer_pattern).1)
    }

    // --- cooling --------------------------------------------------------------------

    async fn can_set_ccd_temperature(&self) -> ASCOMResult<bool> {
        self.ensure_connected()?;
        Ok(self.sensor()?.supports_control_temp)
    }

    async fn can_get_cooler_power(&self) -> ASCOMResult<bool> {
        self.ensure_connected()?;
        Ok(self.sensor()?.supports_control_temp)
    }

    async fn ccd_temperature(&self) -> ASCOMResult<f64> {
        self.ensure_connected()?;
        // K2: unlike zwo-camera's separately-cached temperature_available,
        // SVBony's property_ex exposes a single bSupportControlTemp flag
        // covering both the cooler and the readable sensor temperature, so
        // CCDTemperature is gated on the same flag as CanSetCCDTemperature.
        if !self.sensor()?.supports_control_temp {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        self.on_handle(|h| {
            h.control_value(ControlType::CurrentTemperature)
                .map(|t| t as f64 / 10.0)
                .map_err(|_| {
                    ASCOMError::new(UNSPECIFIED_ERROR, "failed to read sensor temperature")
                })
        })
        .await
    }

    async fn set_ccd_temperature(&self) -> ASCOMResult<f64> {
        self.ensure_connected()?;
        if !self.sensor()?.supports_control_temp {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        if let Some(target) = *self.state.target_temperature.lock() {
            return Ok(target);
        }
        self.on_handle(|h| {
            h.control_value(ControlType::TargetTemperature)
                .map(|t| t as f64 / 10.0)
                .map_err(|_| ASCOMError::INVALID_VALUE)
        })
        .await
    }

    async fn set_set_ccd_temperature(&self, set_ccd_temperature: f64) -> ASCOMResult<()> {
        self.ensure_connected()?;
        if !self.sensor()?.supports_control_temp {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        if !(-273.15..=80.0).contains(&set_ccd_temperature) {
            return Err(ASCOMError::invalid_value(format!(
                "target temperature {set_ccd_temperature} outside [-273.15, 80]"
            )));
        }
        // K3: encode to tenths of a degree (SVB_TARGET_TEMPERATURE's units).
        let tenths = (set_ccd_temperature * 10.0).round() as i64;
        self.on_handle(move |h| {
            h.set_control_value(ControlType::TargetTemperature, tenths)
                .map_err(|_| ASCOMError::invalid_operation("failed to set target temperature"))
        })
        .await?;
        *self.state.target_temperature.lock() = Some(set_ccd_temperature);
        Ok(())
    }

    async fn cooler_on(&self) -> ASCOMResult<bool> {
        self.ensure_connected()?;
        if !self.sensor()?.supports_control_temp {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        self.on_handle(|h| {
            h.control_value(ControlType::CoolerEnable)
                .map(|v| v != 0)
                .map_err(|_| ASCOMError::INVALID_VALUE)
        })
        .await
    }

    // K5 (tenet 3): this is the ONLY code path in this file that may write
    // SVB_COOLER_ENABLE, and it is reachable solely from an explicit
    // operator ASCOM `CoolerOn` call — never from `connect`/`disconnect`/
    // `open_handshake`/`config.apply`.
    async fn set_cooler_on(&self, cooler_on: bool) -> ASCOMResult<()> {
        self.ensure_connected()?;
        if !self.sensor()?.supports_control_temp {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        self.on_handle(move |h| {
            h.set_control_value(ControlType::CoolerEnable, i64::from(cooler_on))
                .map_err(|_| ASCOMError::invalid_operation("failed to set cooler state"))
        })
        .await
    }

    async fn cooler_power(&self) -> ASCOMResult<f64> {
        self.ensure_connected()?;
        if !self.sensor()?.supports_control_temp {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        // K4: SVB_COOLER_POWER is already a 0-100 percent, no normalization.
        self.on_handle(|h| {
            h.control_value(ControlType::CoolerPower)
                .map(|p| p as f64)
                .map_err(|_| ASCOMError::INVALID_VALUE)
        })
        .await
    }

    // --- shutter / capability flags --------------------------------------------------

    async fn has_shutter(&self) -> ASCOMResult<bool> {
        // No mechanical shutter in video mode (E4/E7).
        Ok(false)
    }

    async fn can_abort_exposure(&self) -> ASCOMResult<bool> {
        Ok(true)
    }

    async fn can_stop_exposure(&self) -> ASCOMResult<bool> {
        // E8: no data-preserving stop exists at the SDK level.
        Ok(false)
    }

    async fn can_pulse_guide(&self) -> ASCOMResult<bool> {
        self.ensure_connected()?;
        Ok(self.sensor()?.supports_pulse_guide)
    }

    async fn is_pulse_guiding(&self) -> ASCOMResult<bool> {
        Ok(self.state.pulse_guiding.load(Ordering::Acquire))
    }

    // --- exposure state ---------------------------------------------------------------

    async fn camera_state(&self) -> ASCOMResult<CameraState> {
        if self.state.last_error.lock().is_some() {
            return Ok(CameraState::Error);
        }
        // An abort was requested: report idle promptly even though the
        // still-running, un-interruptible capture task hasn't drained yet
        // (see `DeviceState::aborted`'s doc comment) — `exposure_in_flight`
        // alone would keep reporting `Exposing` for the rest of the
        // deadline.
        if self.state.aborted.load(Ordering::Acquire) {
            return Ok(CameraState::Idle);
        }
        if self.state.exposure_in_flight.load(Ordering::Acquire) {
            return Ok(CameraState::Exposing);
        }
        Ok(CameraState::Idle)
    }

    async fn image_ready(&self) -> ASCOMResult<bool> {
        Ok(self.state.image_ready.load(Ordering::Acquire)
            && !self.state.exposure_in_flight.load(Ordering::Acquire))
    }

    async fn percent_completed(&self) -> ASCOMResult<u8> {
        // Mirror `camera_state`: an abort means no image is ready, so 0 (not
        // the idle-ready branch's 100) is the honest answer while the
        // still-running capture task drains in the background.
        if self.state.aborted.load(Ordering::Acquire) {
            return Ok(0);
        }
        if !self.state.exposure_in_flight.load(Ordering::Acquire) {
            // Idle: 100 once ready, 0 in the Error state.
            return Ok(if self.state.last_error.lock().is_some() {
                0
            } else {
                100
            });
        }
        let start = *self.state.last_exposure_start_time.lock();
        let duration = *self.state.last_exposure_duration.lock();
        let (Some(start), Some(duration)) = (start, duration) else {
            return Ok(0);
        };
        if duration.is_zero() {
            return Ok(99);
        }
        let elapsed = start.elapsed().unwrap_or(Duration::ZERO);
        // Never report 100 while in flight (that is reserved for the ready state).
        let pct = (elapsed.as_secs_f64() / duration.as_secs_f64() * 100.0).clamp(0.0, 99.0);
        Ok(pct as u8)
    }

    async fn last_exposure_start_time(&self) -> ASCOMResult<SystemTime> {
        (*self.state.last_exposure_start_time.lock()).ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn last_exposure_duration(&self) -> ASCOMResult<Duration> {
        (*self.state.last_exposure_duration.lock()).ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn image_array(&self) -> ASCOMResult<ImageArray> {
        self.ensure_connected()?;
        if let Some(msg) = self.state.last_error.lock().clone() {
            return Err(ASCOMError::new(UNSPECIFIED_ERROR, msg));
        }
        // ASCOM: `ImageArray` is valid only once `ImageReady` is true. Mirror
        // the `image_ready()` condition so a client can never read a stale
        // frame.
        let ready = self.state.image_ready.load(Ordering::Acquire)
            && !self.state.exposure_in_flight.load(Ordering::Acquire);
        if !ready {
            return Err(ASCOMError::invalid_operation(
                "no image available; ImageReady is false",
            ));
        }
        self.state
            .last_image
            .lock()
            .clone()
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    // --- exposure control ---------------------------------------------------------------

    async fn start_exposure(&self, duration: Duration, light: bool) -> ASCOMResult<()> {
        self.ensure_connected()?;
        // No mechanical shutter in video mode: dark and light frames are
        // captured identically (E4/E7) — `light` only ever informed a
        // shutter, which SVBony's video mode does not have.
        let _ = light;

        if self.state.exposure_in_flight.load(Ordering::Acquire) {
            return Err(ASCOMError::invalid_operation(
                "an exposure is already in flight",
            ));
        }

        let sensor = self.sensor()?;
        let (min_us, max_us) =
            (*self.state.exposure_range_us.lock()).ok_or(ASCOMError::INVALID_VALUE)?;
        let exposure_us = (duration.as_secs_f64() * 1_000_000.0).round() as i64;
        if exposure_us < min_us || exposure_us > max_us {
            return Err(ASCOMError::invalid_value(format!(
                "exposure {exposure_us}us outside [{min_us}, {max_us}]"
            )));
        }

        let bin = u32::from(self.state.bin.load(Ordering::Acquire)).max(1);
        let roi = self.validated_geometry(&sensor, bin)?;

        // Claim the in-flight slot; lose the race → already exposing (E2).
        if self
            .state
            .exposure_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ASCOMError::invalid_operation(
                "an exposure is already in flight",
            ));
        }
        let generation = self
            .state
            .exposure_generation
            .fetch_add(1, Ordering::AcqRel)
            + 1;

        self.state.image_ready.store(false, Ordering::Release);
        self.state.aborted.store(false, Ordering::Release);
        *self.state.last_error.lock() = None;
        *self.state.last_exposure_start_time.lock() = Some(SystemTime::now());
        *self.state.last_exposure_duration.lock() = Some(duration);

        let request = CaptureRequest {
            start_x: roi.start_x,
            start_y: roi.start_y,
            width: roi.width,
            height: roi.height,
            bin,
            exposure_us,
            is_trigger_cam: sensor.is_trigger_cam,
            duration,
        };
        let handle = Arc::clone(&self.handle);
        let state = Arc::clone(&self.state);
        tokio::spawn(run_exposure(handle, state, generation, request));
        Ok(())
    }

    async fn abort_exposure(&self) -> ASCOMResult<()> {
        self.ensure_connected()?;
        self.cancel_exposure();
        Ok(())
    }

    async fn stop_exposure(&self) -> ASCOMResult<()> {
        // E8: no data-preserving stop exists at the SDK level, so this is
        // unconditionally NOT_IMPLEMENTED rather than pretending to
        // gracefully preserve data it cannot preserve — the opposite of
        // zwo-camera's graceful ASIStopExposure-backed stop.
        self.ensure_connected()?;
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn pulse_guide(&self, direction: GuideDirection, duration: Duration) -> ASCOMResult<()> {
        self.ensure_connected()?;
        let sensor = self.sensor()?;
        if !sensor.supports_pulse_guide {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        // v0 design decision (documented in docs/services/svbony-camera.md's
        // Pulse guiding contract, PG2): unlike zwo-camera's asynchronous
        // ST4 wrapper (returns immediately, `IsPulseGuiding` tracks a
        // deadline), this call stays a literal blocking `SVBPulseGuide` —
        // `svbony_rs::Camera::pulse_guide` blocks at the SDK level for the
        // pulse duration, and no ST4-capable SVBony model has been
        // validated yet (the SV605CC has no ST4 port, so this whole branch
        // is unexercised by the simulation/BDD suite). If a future
        // ST4-capable model's guide pulses are long enough to risk
        // ConformU's ~1s response budget, revisit with the same
        // fire-and-forget-with-deadline pattern zwo-camera uses.
        let dir = guide_direction(direction);
        let duration_ms = i32::try_from(duration.as_millis()).unwrap_or(i32::MAX);
        self.state.pulse_guiding.store(true, Ordering::Release);
        let result = self
            .on_handle(move |h| {
                h.pulse_guide(dir, duration_ms)
                    .map_err(|_| ASCOMError::invalid_operation("pulse guide failed"))
            })
            .await;
        self.state.pulse_guiding.store(false, Ordering::Release);
        result
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::backend::mock::MockCameraHandle;
    use std::sync::atomic::Ordering as AtomicOrdering;

    fn roi(start_x: u32, start_y: u32, width: u32, height: u32) -> Roi {
        Roi {
            start_x,
            start_y,
            width,
            height,
        }
    }

    fn connected_device(handle: MockCameraHandle) -> SvbonyCamera {
        let device = SvbonyCamera::new(Arc::new(handle), None);
        device.connect().unwrap();
        device
    }

    async fn wait_image_ready(device: &SvbonyCamera) {
        tokio::time::timeout(Duration::from_secs(30), async {
            while !device.image_ready().await.unwrap() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("exposure did not complete");
    }

    async fn wait_camera_state(device: &SvbonyCamera, want: CameraState) {
        tokio::time::timeout(Duration::from_secs(30), async {
            while device.camera_state().await.unwrap() != want {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("camera did not reach {want:?}"));
    }

    // --- pure helpers -------------------------------------------------------------

    #[test]
    fn max_adu_is_two_pow_bits_minus_one() {
        assert_eq!(max_adu_from_bit_depth(16), 65_535);
        assert_eq!(max_adu_from_bit_depth(14), 16_383);
        assert_eq!(max_adu_from_bit_depth(12), 4_095);
        assert_eq!(max_adu_from_bit_depth(0), 0);
    }

    #[test]
    fn rescale_roi_scales_by_bin_ratio() {
        let scaled = rescale_roi(roi(100, 200, 800, 600), 1, 2);
        assert_eq!(scaled.start_x, 50);
        assert_eq!(scaled.start_y, 100);
        assert_eq!(scaled.width, 400);
        assert_eq!(scaled.height, 300);
    }

    #[test]
    fn guide_direction_maps_every_ascom_direction() {
        assert_eq!(
            guide_direction(GuideDirection::North),
            svbony_rs::GuideDirection::North
        );
        assert_eq!(
            guide_direction(GuideDirection::South),
            svbony_rs::GuideDirection::South
        );
        assert_eq!(
            guide_direction(GuideDirection::East),
            svbony_rs::GuideDirection::East
        );
        assert_eq!(
            guide_direction(GuideDirection::West),
            svbony_rs::GuideDirection::West
        );
    }

    #[test]
    fn bayer_offset_mapping() {
        assert_eq!(bayer_offsets(BayerPattern::Rg), (0, 0));
        assert_eq!(bayer_offsets(BayerPattern::Bg), (1, 1));
        assert_eq!(bayer_offsets(BayerPattern::Gr), (1, 0));
        assert_eq!(bayer_offsets(BayerPattern::Gb), (0, 1));
    }

    #[test]
    fn check_geometry_rejects_zero_size() {
        assert!(check_geometry(roi(0, 0, 0, 64), 3008, 3008, 1).is_err());
        assert!(check_geometry(roi(0, 0, 64, 0), 3008, 3008, 1).is_err());
    }

    #[test]
    fn check_geometry_rejects_misaligned_size() {
        assert!(check_geometry(roi(0, 0, 100, 64), 3008, 3008, 1).is_err());
        assert!(check_geometry(roi(0, 0, 64, 47), 3008, 3008, 1).is_err());
    }

    #[test]
    fn check_geometry_rejects_out_of_bounds() {
        assert!(check_geometry(roi(0, 0, 4000, 64), 3008, 3008, 1).is_err());
        assert!(check_geometry(roi(0, 0, 64, 4000), 3008, 3008, 1).is_err());
        assert!(check_geometry(roi(3008, 0, 64, 64), 3008, 3008, 1).is_err());
        assert!(check_geometry(roi(0, 3008, 64, 64), 3008, 3008, 1).is_err());
    }

    #[test]
    fn check_geometry_accepts_valid_full_and_sub_frames() {
        assert!(check_geometry(roi(0, 0, 3008, 3008), 3008, 3008, 1).is_ok());
        assert!(check_geometry(roi(0, 0, 64, 48), 3008, 3008, 1).is_ok());
        // Binned bounds shrink: at bin 2 the sensor's addressable extent is
        // 1504x1504, so a 1504x1504 frame is valid but a 1600x1600 one is not.
        assert!(check_geometry(roi(0, 0, 1504, 1504), 3008, 3008, 2).is_ok());
        assert!(check_geometry(roi(0, 0, 1600, 1600), 3008, 3008, 2).is_err());
    }

    #[test]
    fn to_image_array_16bit_has_width_major_axes() {
        let bytes = vec![0u8; 64 * 48 * 2];
        let array = to_image_array(&bytes, 64, 48).unwrap();
        // ASCOM [x][y]: first axis = width.
        assert_eq!(array.dim().0, 64);
        assert_eq!(array.dim().1, 48);
    }

    #[test]
    fn to_image_array_rejects_a_short_buffer() {
        let bytes = vec![0u8; 10];
        assert!(to_image_array(&bytes, 64, 48).is_err());
    }

    // --- connection lifecycle -------------------------------------------------------

    #[tokio::test]
    async fn starts_disconnected() {
        let cam = SvbonyCamera::new(Arc::new(MockCameraHandle::default()), None);
        assert!(!cam.connected().await.unwrap());
    }

    #[tokio::test]
    async fn connect_caches_sensor_properties_from_the_handle() {
        let cam = connected_device(MockCameraHandle::default());
        assert_eq!(cam.camera_x_size().await.unwrap(), 3008);
        assert_eq!(cam.camera_y_size().await.unwrap(), 3008);
        assert_eq!(cam.max_adu().await.unwrap(), 16_383);
    }

    #[tokio::test]
    async fn a_failed_open_leaves_the_device_disconnected() {
        let handle = MockCameraHandle::default();
        handle.fail_open.store(true, AtomicOrdering::SeqCst);
        let cam = SvbonyCamera::new(Arc::new(handle), None);
        cam.set_connected(true).await.unwrap_err();
        assert!(!cam.connected().await.unwrap());
    }

    #[tokio::test]
    async fn missing_exposure_control_fails_connect() {
        let handle = MockCameraHandle::default().without_control(ControlType::Exposure);
        let cam = SvbonyCamera::new(Arc::new(handle), None);
        cam.set_connected(true).await.unwrap_err();
        assert!(!cam.connected().await.unwrap());
    }

    // --- sensor properties (ST1/ST2/ST3) ---------------------------------------------

    #[tokio::test]
    async fn a_color_camera_reports_rggb() {
        let cam = connected_device(MockCameraHandle::default());
        assert_eq!(cam.sensor_type().await.unwrap(), SensorType::RGGB);
        assert_eq!(cam.bayer_offset_x().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn a_monochrome_camera_has_no_bayer_offset() {
        let cam = connected_device(MockCameraHandle::default().monochrome());
        assert_eq!(cam.sensor_type().await.unwrap(), SensorType::Monochrome);
        assert_eq!(
            cam.bayer_offset_x().await.unwrap_err().code,
            ASCOMError::NOT_IMPLEMENTED.code
        );
    }

    #[tokio::test]
    async fn electrons_per_adu_is_a_permanent_placeholder() {
        let cam = connected_device(MockCameraHandle::default());
        assert_eq!(
            cam.electrons_per_adu().await.unwrap_err().code,
            ASCOMError::NOT_IMPLEMENTED.code
        );
    }

    // --- gain / offset (GO1/GO2/GO3) ---------------------------------------------------

    #[tokio::test]
    async fn gain_is_not_implemented_when_the_control_is_absent() {
        let cam = connected_device(MockCameraHandle::default().without_control(ControlType::Gain));
        assert_eq!(
            cam.gain().await.unwrap_err().code,
            ASCOMError::NOT_IMPLEMENTED.code
        );
    }

    #[tokio::test]
    async fn set_gain_rejects_an_out_of_range_value() {
        let cam = connected_device(MockCameraHandle::default());
        let max = cam.gain_max().await.unwrap();
        assert_eq!(
            cam.set_gain(max + 1).await.unwrap_err().code,
            ASCOMError::INVALID_VALUE.code
        );
    }

    #[tokio::test]
    async fn set_gain_round_trips_a_valid_value() {
        let cam = connected_device(MockCameraHandle::default());
        cam.set_gain(50).await.unwrap();
        assert_eq!(cam.gain().await.unwrap(), 50);
    }

    // --- offset (the ASCOM Offset == SVBony BlackLevel control, GO1) ------------------

    #[tokio::test]
    async fn offset_is_not_implemented_when_the_control_is_absent() {
        let cam =
            connected_device(MockCameraHandle::default().without_control(ControlType::BlackLevel));
        assert_eq!(
            cam.offset().await.unwrap_err().code,
            ASCOMError::NOT_IMPLEMENTED.code
        );
        assert_eq!(
            cam.offset_min().await.unwrap_err().code,
            ASCOMError::NOT_IMPLEMENTED.code
        );
        assert_eq!(
            cam.offset_max().await.unwrap_err().code,
            ASCOMError::NOT_IMPLEMENTED.code
        );
        assert_eq!(
            cam.set_offset(0).await.unwrap_err().code,
            ASCOMError::NOT_IMPLEMENTED.code
        );
    }

    #[tokio::test]
    async fn set_offset_rejects_an_out_of_range_value() {
        let cam = connected_device(MockCameraHandle::default());
        let max = cam.offset_max().await.unwrap();
        assert_eq!(
            cam.set_offset(max + 1).await.unwrap_err().code,
            ASCOMError::INVALID_VALUE.code
        );
    }

    #[tokio::test]
    async fn set_offset_round_trips_a_valid_value() {
        let cam = connected_device(MockCameraHandle::default());
        assert_ne!(
            cam.offset().await.unwrap(),
            42,
            "picked a non-default value"
        );
        cam.set_offset(42).await.unwrap();
        assert_eq!(cam.offset().await.unwrap(), 42);
    }

    // --- readout mode -------------------------------------------------------------------

    #[tokio::test]
    async fn readout_mode_defaults_to_zero_and_lists_the_known_modes() {
        let cam = connected_device(MockCameraHandle::default());
        assert_eq!(cam.readout_mode().await.unwrap(), 0);
        assert!(!cam.readout_modes().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn set_readout_mode_rejects_out_of_range_and_round_trips_valid() {
        let cam = connected_device(MockCameraHandle::default());
        let modes = cam.readout_modes().await.unwrap();
        assert_eq!(
            cam.set_readout_mode(modes.len()).await.unwrap_err().code,
            ASCOMError::INVALID_VALUE.code
        );
        cam.set_readout_mode(modes.len() - 1).await.unwrap();
        assert_eq!(cam.readout_mode().await.unwrap(), modes.len() - 1);
    }

    // --- binning / ROI (B1-B3, R1-R3) --------------------------------------------------

    #[tokio::test]
    async fn set_bin_rejects_an_unsupported_value() {
        let cam = connected_device(MockCameraHandle::default());
        assert_eq!(
            cam.set_bin_x(99).await.unwrap_err().code,
            ASCOMError::INVALID_VALUE.code
        );
    }

    #[tokio::test]
    async fn set_bin_rescales_the_cached_roi() {
        let cam = connected_device(MockCameraHandle::default());
        cam.set_start_x(100).await.unwrap();
        cam.set_num_x(800).await.unwrap();
        cam.set_bin_x(2).await.unwrap();
        assert_eq!(cam.start_x().await.unwrap(), 50);
        assert_eq!(cam.num_x().await.unwrap(), 400);
    }

    #[tokio::test]
    async fn roi_setters_accept_any_value_but_start_exposure_validates() {
        let cam = connected_device(MockCameraHandle::default());
        cam.set_num_x(5000).await.unwrap();
        cam.set_num_y(64).await.unwrap();
        cam.set_start_x(0).await.unwrap();
        cam.set_start_y(0).await.unwrap();
        assert_eq!(cam.num_x().await.unwrap(), 5000);
        let err = cam
            .start_exposure(Duration::from_millis(10), true)
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMError::INVALID_VALUE.code);
    }

    // --- cooling (K1-K5) ----------------------------------------------------------------

    #[tokio::test]
    async fn cooling_is_not_implemented_without_temp_control() {
        let cam = connected_device(MockCameraHandle::default().without_temp_control());
        assert!(!cam.can_set_ccd_temperature().await.unwrap());
        assert_eq!(
            cam.ccd_temperature().await.unwrap_err().code,
            ASCOMError::NOT_IMPLEMENTED.code
        );
        assert_eq!(
            cam.cooler_on().await.unwrap_err().code,
            ASCOMError::NOT_IMPLEMENTED.code
        );
    }

    #[tokio::test]
    async fn k5_connecting_never_enables_the_cooler() {
        // Tenet 3: connect must never actuate the cooler.
        let cam = connected_device(MockCameraHandle::default());
        assert!(!cam.cooler_on().await.unwrap());
    }

    #[tokio::test]
    async fn connecting_a_trigger_camera_arms_video_capture_exactly_once() {
        // Trigger-gated capture produces no frames until an operator's soft
        // trigger, so arming it once at connect is not actuation (tenet 3).
        let handle = Arc::new(MockCameraHandle::default());
        let cam = SvbonyCamera::new(handle.clone(), None);
        cam.connect().unwrap();
        assert_eq!(handle.start_video_capture_call_count(), 1);
    }

    #[tokio::test]
    async fn connecting_a_non_trigger_camera_never_starts_video_capture() {
        // Tenet 3: a non-trigger camera's only mode is free-running, so
        // starting video capture at connect would begin the sensor
        // continuously integrating/streaming with no operator action.
        // Capture must stay unarmed until the operator's first StartExposure.
        let handle = Arc::new(MockCameraHandle::default().without_trigger_cam());
        let cam = SvbonyCamera::new(handle.clone(), None);
        cam.connect().unwrap();
        assert_eq!(handle.start_video_capture_call_count(), 0);
        assert_eq!(handle.stop_video_capture_call_count(), 0);
    }

    #[tokio::test]
    async fn set_ccd_temperature_rejects_out_of_range() {
        let cam = connected_device(MockCameraHandle::default());
        assert_eq!(
            cam.set_set_ccd_temperature(-300.0).await.unwrap_err().code,
            ASCOMError::INVALID_VALUE.code
        );
        assert_eq!(
            cam.set_set_ccd_temperature(100.0).await.unwrap_err().code,
            ASCOMError::INVALID_VALUE.code
        );
    }

    #[tokio::test]
    async fn set_ccd_temperature_round_trips() {
        let cam = connected_device(MockCameraHandle::default());
        cam.set_set_ccd_temperature(-10.0).await.unwrap();
        let readback = cam.set_ccd_temperature().await.unwrap();
        assert!((readback - (-10.0)).abs() < 1e-9);
    }

    #[tokio::test]
    async fn turning_the_cooler_on_is_reflected() {
        let cam = connected_device(MockCameraHandle::default());
        cam.set_cooler_on(true).await.unwrap();
        assert!(cam.cooler_on().await.unwrap());
    }

    #[tokio::test]
    async fn ccd_temperature_and_cooler_power_read_the_live_sensor() {
        let cam = connected_device(MockCameraHandle::default());
        assert!(cam.can_get_cooler_power().await.unwrap());
        // The mock's default CurrentTemperature/CoolerPower controls (K4).
        assert!((cam.ccd_temperature().await.unwrap() - 20.0).abs() < 1e-9);
        assert!((cam.cooler_power().await.unwrap() - 0.0).abs() < 1e-9);
        cam.set_cooler_on(true).await.unwrap();
        assert!((cam.cooler_power().await.unwrap() - 60.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn cooler_power_is_not_implemented_without_temp_control() {
        let cam = connected_device(MockCameraHandle::default().without_temp_control());
        assert!(!cam.can_get_cooler_power().await.unwrap());
        assert_eq!(
            cam.cooler_power().await.unwrap_err().code,
            ASCOMError::NOT_IMPLEMENTED.code
        );
    }

    // --- exposure state machine (E1-E9) --------------------------------------------------

    #[tokio::test]
    async fn start_exposure_fails_when_disconnected() {
        let cam = SvbonyCamera::new(Arc::new(MockCameraHandle::default()), None);
        let err = cam
            .start_exposure(Duration::from_millis(10), true)
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMError::NOT_CONNECTED.code);
    }

    #[tokio::test]
    async fn a_second_exposure_while_one_is_in_flight_is_rejected() {
        let handle = MockCameraHandle::default();
        handle.set_capture_delay(Duration::from_millis(200));
        let cam = connected_device(handle);
        cam.set_num_x(64).await.unwrap();
        cam.set_num_y(64).await.unwrap();
        cam.start_exposure(Duration::from_millis(10), true)
            .await
            .unwrap();
        let err = cam
            .start_exposure(Duration::from_millis(10), true)
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMError::INVALID_OPERATION.code);
    }

    #[tokio::test]
    async fn out_of_range_duration_is_rejected() {
        let cam = connected_device(MockCameraHandle::default());
        cam.set_num_x(64).await.unwrap();
        cam.set_num_y(64).await.unwrap();
        let err = cam
            .start_exposure(Duration::from_secs(2500), true)
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMError::INVALID_VALUE.code);
    }

    #[tokio::test]
    async fn a_successful_exposure_produces_an_image() {
        let cam = connected_device(MockCameraHandle::default());
        cam.set_num_x(64).await.unwrap();
        cam.set_num_y(48).await.unwrap();
        cam.start_exposure(Duration::from_millis(10), true)
            .await
            .unwrap();
        wait_image_ready(&cam).await;
        let image = cam.image_array().await.unwrap();
        assert_eq!(image.dim().0, 64);
        assert_eq!(image.dim().1, 48);
        cam.last_exposure_start_time().await.unwrap();
        assert_eq!(
            cam.last_exposure_duration().await.unwrap(),
            Duration::from_millis(10)
        );
    }

    #[tokio::test]
    async fn a_dark_frame_captures_identically_to_a_light_frame() {
        let cam = connected_device(MockCameraHandle::default());
        assert!(!cam.has_shutter().await.unwrap());
        cam.set_num_x(64).await.unwrap();
        cam.set_num_y(48).await.unwrap();
        cam.start_exposure(Duration::from_millis(10), false)
            .await
            .unwrap();
        wait_image_ready(&cam).await;
        assert!(cam.image_ready().await.unwrap());
    }

    #[tokio::test]
    async fn percent_completed_is_100_once_ready() {
        let cam = connected_device(MockCameraHandle::default());
        cam.set_num_x(64).await.unwrap();
        cam.set_num_y(64).await.unwrap();
        cam.start_exposure(Duration::from_millis(10), true)
            .await
            .unwrap();
        wait_image_ready(&cam).await;
        wait_camera_state(&cam, CameraState::Idle).await;
        assert_eq!(cam.percent_completed().await.unwrap(), 100);
    }

    #[tokio::test]
    async fn aborting_an_in_flight_exposure_leaves_no_image_ready() {
        let handle = MockCameraHandle::default();
        handle.set_capture_delay(Duration::from_millis(300));
        let cam = connected_device(handle);
        cam.set_num_x(64).await.unwrap();
        cam.set_num_y(64).await.unwrap();
        assert!(cam.can_abort_exposure().await.unwrap());
        cam.start_exposure(Duration::from_secs(30), true)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        cam.abort_exposure().await.unwrap();
        assert!(!cam.image_ready().await.unwrap());
        // CameraState/PercentCompleted must reflect the abort immediately,
        // not only once the still-running, un-interruptible capture task
        // happens to drain (the whole injected 300ms capture_delay is still
        // in flight here).
        assert_eq!(cam.camera_state().await.unwrap(), CameraState::Idle);
        assert_eq!(cam.percent_completed().await.unwrap(), 0);
        // The late-completing capture task must not resurrect ImageReady or
        // an Error state once it eventually drains (the generation-counter
        // guard, E7).
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!cam.image_ready().await.unwrap());
        assert_eq!(cam.camera_state().await.unwrap(), CameraState::Idle);
    }

    #[tokio::test]
    async fn aborting_with_no_exposure_in_flight_is_a_no_op() {
        let cam = connected_device(MockCameraHandle::default());
        cam.abort_exposure().await.unwrap();
        assert_eq!(cam.camera_state().await.unwrap(), CameraState::Idle);
        assert!(!cam.image_ready().await.unwrap());
    }

    #[tokio::test]
    async fn there_is_no_data_preserving_stop() {
        let cam = connected_device(MockCameraHandle::default());
        assert!(!cam.can_stop_exposure().await.unwrap());
        let err = cam.stop_exposure().await.unwrap_err();
        assert_eq!(err.code, ASCOMError::NOT_IMPLEMENTED.code);
    }

    /// E9: a mid-exposure SDK failure transitions to the Error state — the
    /// design doc explicitly reserves this contract for a mock-backend unit
    /// test since the `svbony-rs` simulation cannot force an SDK error.
    #[tokio::test]
    async fn e9_mid_exposure_sdk_failure_sets_the_error_state() {
        let handle = MockCameraHandle::default();
        handle.fail_capture.store(true, AtomicOrdering::SeqCst);
        let cam = connected_device(handle);
        cam.set_num_x(64).await.unwrap();
        cam.set_num_y(64).await.unwrap();
        cam.start_exposure(Duration::from_millis(10), true)
            .await
            .unwrap();
        wait_camera_state(&cam, CameraState::Error).await;
        assert!(!cam.image_ready().await.unwrap());
        assert!(cam.image_array().await.is_err());
    }

    /// E9's other branch: an exceeded `SVBGetVideoData` deadline is the same
    /// Error-state transition, distinguished only by the recorded message.
    #[tokio::test]
    async fn e9_exceeded_deadline_sets_the_error_state() {
        let handle = MockCameraHandle::default();
        handle.exceed_deadline.store(true, AtomicOrdering::SeqCst);
        let cam = connected_device(handle);
        cam.set_num_x(64).await.unwrap();
        cam.set_num_y(64).await.unwrap();
        cam.start_exposure(Duration::from_millis(10), true)
            .await
            .unwrap();
        wait_camera_state(&cam, CameraState::Error).await;
        assert!(!cam.image_ready().await.unwrap());
    }

    /// State-machine step 5: a non-trigger-capable camera still completes an
    /// exposure (via the backend's free-running restart fallback), and
    /// `camera.rs` correctly threads `sensor.is_trigger_cam = false` through
    /// to the capture request.
    #[tokio::test]
    async fn a_non_trigger_camera_still_completes_an_exposure() {
        let handle = Arc::new(MockCameraHandle::default().without_trigger_cam());
        let cam = SvbonyCamera::new(handle.clone(), None);
        cam.connect().unwrap();
        cam.set_num_x(64).await.unwrap();
        cam.set_num_y(64).await.unwrap();
        cam.start_exposure(Duration::from_millis(10), true)
            .await
            .unwrap();
        wait_image_ready(&cam).await;
        assert!(cam.image_ready().await.unwrap());
        let req = handle
            .last_capture_request()
            .expect("capture should have run");
        assert!(!req.is_trigger_cam);
    }

    // --- pulse guide (PG1/PG2) --------------------------------------------------------

    #[tokio::test]
    async fn pulse_guide_is_not_implemented_without_st4() {
        let cam = connected_device(MockCameraHandle::default());
        assert!(!cam.can_pulse_guide().await.unwrap());
        let err = cam
            .pulse_guide(GuideDirection::North, Duration::from_millis(100))
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMError::NOT_IMPLEMENTED.code);
    }

    #[tokio::test]
    async fn pulse_guide_succeeds_on_an_st4_capable_model() {
        let cam = connected_device(MockCameraHandle::default().with_pulse_guide());
        assert!(cam.can_pulse_guide().await.unwrap());
        cam.pulse_guide(GuideDirection::North, Duration::from_millis(5))
            .await
            .unwrap();
        // The blocking call has already returned by the time `pulse_guide`
        // resolves (v0 keeps it synchronous — see the doc comment).
        assert!(!cam.is_pulse_guiding().await.unwrap());
    }

    // --- disconnect cancels an in-flight exposure (C3b) --------------------------------

    #[tokio::test]
    async fn disconnecting_cancels_an_in_flight_exposure() {
        let handle = MockCameraHandle::default();
        handle.set_capture_delay(Duration::from_millis(300));
        let cam = connected_device(handle);
        cam.set_num_x(64).await.unwrap();
        cam.set_num_y(64).await.unwrap();
        cam.start_exposure(Duration::from_secs(30), true)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        cam.set_connected(false).await.unwrap();
        assert!(!cam.image_ready().await.unwrap());
        assert!(!cam.connected().await.unwrap());
    }

    // --- name / description overrides -----------------------------------------------

    #[tokio::test]
    async fn unique_id_and_name_come_from_the_handle() {
        let cam = SvbonyCamera::new(Arc::new(MockCameraHandle::default()), None);
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
}
