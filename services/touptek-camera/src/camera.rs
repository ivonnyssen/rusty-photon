//! `TouptekCamera` — the ASCOM `Device` + `Camera` implementation over the
//! [`CameraHandle`](crate::backend::CameraHandle) seam.
//!
//! Behaviour follows `docs/services/touptek-camera.md`, with these deliberate
//! divergences from the `zwo-camera` precedent (all driven by the ToupCam SDK):
//! - **Digital binning** (`OPTION_BINNING`): symmetric only
//!   (`CanAsymmetricBin = false`), reported to ASCOM as binning but not advertised
//!   as hardware binning.
//! - **Even ROI**: offsets *and* sizes must be even and at least 8×8 (vs ZWO's
//!   `NumX % 8` / `NumY % 2`).
//! - **`CanStopExposure = false`**: trigger mode yields one whole frame with no
//!   partial readout, so there is no data-preserving graceful stop; abort
//!   discards (E8, the divergence from `zwo-camera`).
//! - **`ElectronsPerADU` / `FullWellCapacity` are `NOT_IMPLEMENTED`**: the ToupCam
//!   SDK exposes no native fields (the QHY-like divergence from ZWO).
//! - **Gain is `ExpoAGain` percent** (100 = 1.0×), `Offset` is `OPTION_BLACKLEVEL`
//!   with `OffsetMax` computed per bit depth (the SDK exposes no offset accessor).
//! - **Native ST4 `PulseGuide`** (`Toupcam_ST4PlusGuide`): asynchronous, the SDK
//!   times the pulse, so the call returns immediately and `IsPulseGuiding` tracks
//!   the deadline.
//!
//! Discrete exposures are driven by trigger mode plus the `touptek-rs`
//! callback→pull bridge inside [`crate::backend::CameraHandle::capture`]. The
//! blocking capture runs on `spawn_blocking` inside a detached task; a generation
//! counter lets abort/disconnect invalidate a late-completing task.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ascom_alpaca::api::camera::{CameraState, GuideDirection, ImageArray, SensorType};
use ascom_alpaca::api::{Camera, Device};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use ndarray::Array2;
use parking_lot::Mutex;
use touptek_rs::CameraInfo;
use tracing::{debug, warn};

use crate::backend::{CameraHandle, CaptureRequest};
use crate::config::DeviceOverride;
use crate::config_actions::TouptekCameraDriver;
use rusty_photon_driver::ConfigActionCtx;

/// 0x500 — driver-specific catch-all for an asynchronous capture failure
/// surfaced lazily via `image_array` (E9).
const UNSPECIFIED_ERROR: ASCOMErrorCode = ASCOMErrorCode::new_for_driver(0);

/// ToupTek exposure control is in microseconds, so the smallest step is 1 µs.
const EXPOSURE_RESOLUTION: Duration = Duration::from_micros(1);

/// The driver's named readout-mode list. The ToupCam SDK exposes no named readout
/// modes, so v0 advertises a single 16-bit RAW mode (ASCOM requires a non-empty
/// list); switching validates the index and updates cached state (RM1).
const READOUT_MODES: [&str; 1] = ["RAW16"];

/// A region of interest in *binned* pixel coordinates.
#[derive(Debug, Clone, Copy)]
struct Roi {
    start_x: u32,
    start_y: u32,
    width: u32,
    height: u32,
}

/// Per-device runtime state: caches populated at connect plus the exposure state
/// machine. Atomics for the hot/simple flags; `parking_lot::Mutex` for the
/// `Option<…>` caches and the captured image. Locks are never held across an
/// `await`.
#[derive(Debug)]
struct DeviceState {
    /// Current symmetric bin (init 1).
    bin: AtomicU8,
    /// Current readout-mode index into [`READOUT_MODES`].
    readout_mode: AtomicU8,
    /// Intended ROI in *binned* pixel coordinates (rescaled on bin change).
    intended_roi: Mutex<Option<Roi>>,
    /// `(min, max)` exposure microseconds from `Toupcam_get_ExpTimeRange`.
    exposure_range_us: Mutex<Option<(u32, u32)>>,
    /// `(min, max)` analog-gain percent from `Toupcam_get_ExpoAGainRange`, or
    /// `None` when the model has no gain control (GO1).
    gain_min_max: Mutex<Option<(u16, u16)>>,
    /// Whether the model exposes the black-level (`Offset`) control (GO1).
    offset_supported: Mutex<bool>,
    /// Whether the model exposes a sensor-temperature read, cached at the open
    /// handshake. Decoupled from cooling: a model may read temperature without a
    /// cooler, so `CCDTemperature` is reported whenever this is set, while the
    /// cooler-setpoint members stay gated on [`CameraInfo::has_tec`].
    temperature_available: Mutex<bool>,
    target_temperature: Mutex<Option<f64>>,

    exposure_in_flight: AtomicBool,
    image_ready: AtomicBool,
    /// Bumped on each start / abort / disconnect so a late-completing capture task
    /// can tell it has been superseded and discard its result.
    exposure_generation: AtomicU64,
    last_exposure_start_time: Mutex<Option<SystemTime>>,
    last_exposure_duration: Mutex<Option<Duration>>,
    last_image: Mutex<Option<ImageArray>>,
    /// Set on a mid-exposure SDK failure → `CameraState::Error` (E9).
    last_error: Mutex<Option<String>>,
    /// Serializes the capture task's "check generation + commit result" against
    /// `cancel_exposure`'s "bump generation + clear image_ready".
    result_lock: Mutex<()>,
    /// Deadline of an in-flight ST4 guide pulse (asynchronous `PulseGuide`);
    /// `None` when not guiding. `IsPulseGuiding` is `now < deadline` (PG2).
    pulse_guide_until: Mutex<Option<SystemTime>>,
}

impl DeviceState {
    fn new() -> Self {
        Self {
            bin: AtomicU8::new(1),
            readout_mode: AtomicU8::new(0),
            intended_roi: Mutex::new(None),
            exposure_range_us: Mutex::new(None),
            gain_min_max: Mutex::new(None),
            offset_supported: Mutex::new(false),
            temperature_available: Mutex::new(false),
            target_temperature: Mutex::new(None),
            exposure_in_flight: AtomicBool::new(false),
            image_ready: AtomicBool::new(false),
            exposure_generation: AtomicU64::new(0),
            last_exposure_start_time: Mutex::new(None),
            last_exposure_duration: Mutex::new(None),
            last_image: Mutex::new(None),
            last_error: Mutex::new(None),
            result_lock: Mutex::new(()),
            pulse_guide_until: Mutex::new(None),
        }
    }

    /// Reset the exposure state machine to a clean idle state. Called on connect so
    /// a stale `Error` / `ImageReady` / image from a previous session does not
    /// survive a reconnect (C3).
    fn reset_exposure_state(&self) {
        let _guard = self.result_lock.lock();
        self.exposure_generation.fetch_add(1, Ordering::AcqRel);
        self.exposure_in_flight.store(false, Ordering::Release);
        self.image_ready.store(false, Ordering::Release);
        *self.last_image.lock() = None;
        *self.last_error.lock() = None;
        *self.last_exposure_start_time.lock() = None;
        *self.last_exposure_duration.lock() = None;
        *self.pulse_guide_until.lock() = None;
    }
}

/// One ASCOM Camera device per discovered ToupTek camera.
#[derive(Clone, derive_more::Debug)]
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
            state: Arc::new(DeviceState::new()),
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

    fn connect(&self) -> ASCOMResult<()> {
        self.handle.open().map_err(|_| ASCOMError::NOT_CONNECTED)?;
        // A failed post-open handshake must leave the device disconnected (C2),
        // not opened-but-unusable, so close before propagating.
        if let Err(e) = self.open_handshake() {
            if let Err(close_err) = self.handle.close() {
                debug!(error = %close_err, "close after a failed connect handshake also failed");
            }
            return Err(e);
        }
        // A reconnect must not surface a previous session's Error / ImageReady /
        // stale frame (C3).
        self.state.reset_exposure_state();
        debug!(camera = %self.unique_id, "camera connected");
        Ok(())
    }

    /// Read and cache the camera's control ranges after `open()`, and reset the ROI
    /// to the full frame at bin 1. The exposure range is required; gain/offset are
    /// cached as present-or-absent (GO1).
    fn open_handshake(&self) -> ASCOMResult<()> {
        let caps = self
            .handle
            .capabilities()
            .map_err(|_| ASCOMError::NOT_CONNECTED)?;
        *self.state.exposure_range_us.lock() = Some(caps.exposure_range_us);
        *self.state.gain_min_max.lock() = caps.gain_range;
        *self.state.offset_supported.lock() = caps.offset_supported;
        *self.state.temperature_available.lock() = caps.temperature_available;

        self.state.bin.store(1, Ordering::Release);
        self.state.readout_mode.store(0, Ordering::Release);
        *self.state.intended_roi.lock() = Some(Roi {
            start_x: 0,
            start_y: 0,
            width: self.reported_width(),
            height: self.reported_height(),
        });
        *self.state.target_temperature.lock() = None;
        Ok(())
    }

    fn disconnect(&self) -> ASCOMResult<()> {
        // An in-flight exposure is cancelled (C3) before the handle closes.
        self.cancel_exposure();
        self.handle.close().map_err(|_| ASCOMError::NOT_CONNECTED)?;
        debug!(camera = %self.unique_id, "camera disconnected");
        Ok(())
    }

    /// Cancel any in-flight exposure (abort): bump the generation so the capture
    /// task discards its result, clear `image_ready`/`last_error`, and signal the
    /// SDK to abort. Deliberately does NOT clear `exposure_in_flight` — the capture
    /// task clears that once its blocking SDK chain drains, so a new exposure
    /// cannot race the still-running one (the design's "one owner per device").
    fn cancel_exposure(&self) {
        if !self.state.exposure_in_flight.load(Ordering::Acquire) {
            return;
        }
        {
            // Atomic with the capture task's commit so an abort can never be
            // overwritten by a just-completing capture.
            let _guard = self.state.result_lock.lock();
            self.state
                .exposure_generation
                .fetch_add(1, Ordering::AcqRel);
            self.state.image_ready.store(false, Ordering::Release);
            *self.state.last_error.lock() = None;
        }
        self.handle.request_abort();
    }

    /// Reported `CameraXSize`: the sensor width reduced (if needed) so the floored
    /// full frame at every supported bin stays even (see [`aligned_sensor_extent`]).
    fn reported_width(&self) -> u32 {
        aligned_sensor_extent(self.info.max_width, &self.info.supported_bins)
    }

    /// Reported `CameraYSize`: the sensor height aligned likewise.
    fn reported_height(&self) -> u32 {
        aligned_sensor_extent(self.info.max_height, &self.info.supported_bins)
    }

    /// Validate the cached ROI against the binned sensor geometry (R2/R3),
    /// returning the [`Roi`] to push to the SDK.
    fn validated_geometry(&self, bin: u32) -> ASCOMResult<Roi> {
        let roi = (*self.state.intended_roi.lock())
            .ok_or_else(|| ASCOMError::invalid_value("no ROI defined for camera"))?;
        check_geometry(roi, self.reported_width(), self.reported_height(), bin)?;
        Ok(roi)
    }

    fn gain_available(&self) -> bool {
        self.state.gain_min_max.lock().is_some()
    }

    fn offset_available(&self) -> bool {
        *self.state.offset_supported.lock()
    }

    /// Run a blocking SDK-seam call off the async executor. The ToupCam FFI calls
    /// do USB I/O, so running them directly on a Tokio worker could stall other
    /// Alpaca requests; offload them like the capture, connect and pulse-guide
    /// paths.
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
/// sub-frame. ToupTek requires every ROI offset and size to be even, and the
/// width/height to be at least 8.
fn check_geometry(roi: Roi, sensor_w: u32, sensor_h: u32, bin: u32) -> ASCOMResult<()> {
    if roi.width == 0 || roi.height == 0 {
        return Err(ASCOMError::invalid_value(
            "NumX and NumY must be greater than 0",
        ));
    }
    if !roi.start_x.is_multiple_of(2)
        || !roi.start_y.is_multiple_of(2)
        || !roi.width.is_multiple_of(2)
        || !roi.height.is_multiple_of(2)
    {
        return Err(ASCOMError::invalid_value(
            "ToupTek requires even ROI offsets and sizes",
        ));
    }
    if roi.width < 8 || roi.height < 8 {
        return Err(ASCOMError::invalid_value(
            "NumX and NumY must be at least 8",
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

/// The largest sensor extent (≤ `max`) the driver reports such that the floored
/// full frame at *every* supported bin (`extent / bin`, integer division) is even
/// — the ToupTek ROI rule.
///
/// ConformU takes a full frame at every bin via `NumX = CameraXSize / bin` (and
/// likewise `NumY`); reporting an extent whose floored binned frame is odd at some
/// bin would make that frame an invalid ROI. For the simulated 6248×4176 sensor
/// every floored binned full frame is already even, so this is a no-op (a
/// simplification over `zwo-camera`, where `NumX` must be a multiple of 8).
fn aligned_sensor_extent(max: u32, supported_bins: &[u32]) -> u32 {
    let even_at_every_bin = |e: u32| {
        supported_bins
            .iter()
            .copied()
            .filter(|&b| b > 0)
            .all(|b| (e / b).is_multiple_of(2))
    };
    (1..=max)
        .rev()
        .find(|&e| even_at_every_bin(e))
        .unwrap_or(max)
}

/// `MaxADU = 2^bit_depth - 1` (e.g. 65535 for a 16-bit sensor), saturating.
fn max_adu_from_bit_depth(bit_depth: u32) -> u32 {
    1u32.checked_shl(bit_depth).map_or(u32::MAX, |v| v - 1)
}

/// `OffsetMax` for a bit depth: the ToupTek black-level range is
/// `[0, 31 * 2^(bit_depth - 8)]` (`TOUPCAM_BLACKLEVEL*_MAX`), e.g. 7936 at 16-bit.
fn offset_max_from_bit_depth(bit_depth: u32) -> i32 {
    let shift = bit_depth.saturating_sub(8);
    31i64
        .checked_shl(shift)
        .map_or(i32::MAX, |v| v.min(i64::from(i32::MAX)) as i32)
}

/// Map an ASCOM guide direction onto the `touptek-rs` one.
fn guide_direction(direction: GuideDirection) -> touptek_rs::GuideDirection {
    match direction {
        GuideDirection::North => touptek_rs::GuideDirection::North,
        GuideDirection::South => touptek_rs::GuideDirection::South,
        GuideDirection::East => touptek_rs::GuideDirection::East,
        GuideDirection::West => touptek_rs::GuideDirection::West,
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
        .chunks_exact(2)
        .map(|c| u16::from_ne_bytes([c[0], c[1]]))
        .collect();
    let arr = Array2::from_shape_vec((h, w), pixels).map_err(|e| e.to_string())?;
    Ok(ImageArray::from(arr.reversed_axes()))
}

/// The detached capture task: runs the blocking single-frame SDK chain *and* the
/// CPU-heavy frame transform on `spawn_blocking`, then stores the image (or
/// records a failure as the `Error` state) — unless a newer generation has
/// superseded it.
///
/// Both the SDK capture and [`to_image_array`] run inside the one `spawn_blocking`
/// closure on purpose: a full-frame `u16`→`i32` widen+transpose is hundreds of
/// milliseconds in a debug/CI build, so offloading it keeps the Tokio workers free
/// for HTTP and holds `result_lock` only for the cheap commit below.
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
            .map(|frame| frame.map(|bytes| to_image_array(&bytes, width, height)))
    })
    .await;

    {
        // No await is held across the lock (the blocking await is above), so this
        // "check generation + record" is atomic against cancel_exposure. Only the
        // cheap commit runs here — the transform already happened off-thread.
        let _guard = state.result_lock.lock();
        if state.exposure_generation.load(Ordering::Acquire) == generation {
            match result {
                Ok(Ok(Some(Ok(array)))) => {
                    *state.last_image.lock() = Some(array);
                    *state.last_error.lock() = None;
                    state.image_ready.store(true, Ordering::Release);
                }
                Ok(Ok(Some(Err(e)))) => {
                    warn!(error = %e, "failed to transform captured image");
                    *state.last_image.lock() = None;
                    *state.last_error.lock() = Some(format!("image transform failed: {e}"));
                }
                // Aborted: discard the frame, leave no Error state (E7).
                Ok(Ok(None)) => {}
                Ok(Err(e)) => {
                    warn!(error = %e.0, "mid-exposure SDK error");
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
        // `connect`/`disconnect` do blocking SDK I/O — `Toupcam_OpenByIndex`
        // enumerates over USB and the handshake reads the control ranges — so
        // offload them off the executor (TouptekCamera is cheap to clone: it is
        // `Arc`-backed).
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
    // --- geometry ---------------------------------------------------------------

    async fn camera_x_size(&self) -> ASCOMResult<u32> {
        self.ensure_connected()?;
        Ok(self.reported_width())
    }

    async fn camera_y_size(&self) -> ASCOMResult<u32> {
        self.ensure_connected()?;
        Ok(self.reported_height())
    }

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
        Ok(max_adu_from_bit_depth(self.info.bit_depth))
    }

    async fn sensor_name(&self) -> ASCOMResult<String> {
        self.ensure_connected()?;
        Ok(self.info.model_name.clone())
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
        if !self.info.supported_bins.contains(&u32::from(bin_x)) {
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
        self.info
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
        Ok(Duration::from_micros(u64::from(min)))
    }

    async fn exposure_max(&self) -> ASCOMResult<Duration> {
        self.ensure_connected()?;
        let (_, max) = (*self.state.exposure_range_us.lock()).ok_or(ASCOMError::INVALID_VALUE)?;
        Ok(Duration::from_micros(u64::from(max)))
    }

    async fn exposure_resolution(&self) -> ASCOMResult<Duration> {
        self.ensure_connected()?;
        Ok(EXPOSURE_RESOLUTION)
    }

    // --- gain / offset ----------------------------------------------------------

    async fn gain(&self) -> ASCOMResult<i32> {
        self.ensure_connected()?;
        if !self.gain_available() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        self.on_handle(|h| {
            h.gain()
                .map(i32::from)
                .map_err(|_| ASCOMError::new(UNSPECIFIED_ERROR, "failed to read gain"))
        })
        .await
    }

    async fn gain_min(&self) -> ASCOMResult<i32> {
        self.ensure_connected()?;
        (*self.state.gain_min_max.lock())
            .map(|(min, _)| i32::from(min))
            .ok_or(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn gain_max(&self) -> ASCOMResult<i32> {
        self.ensure_connected()?;
        (*self.state.gain_min_max.lock())
            .map(|(_, max)| i32::from(max))
            .ok_or(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn set_gain(&self, gain: i32) -> ASCOMResult<()> {
        self.ensure_connected()?;
        let (min, max) = (*self.state.gain_min_max.lock()).ok_or(ASCOMError::NOT_IMPLEMENTED)?;
        if gain < i32::from(min) || gain > i32::from(max) {
            return Err(ASCOMError::invalid_value(format!(
                "gain {gain} outside [{min}, {max}]"
            )));
        }
        // Validated into `[min, max]` (a `u16` range), so the cast cannot truncate.
        let percent = gain as u16;
        self.on_handle(move |h| {
            h.set_gain(percent)
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
            h.offset()
                .map_err(|_| ASCOMError::new(UNSPECIFIED_ERROR, "failed to read offset"))
        })
        .await
    }

    async fn offset_min(&self) -> ASCOMResult<i32> {
        self.ensure_connected()?;
        if !self.offset_available() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        Ok(0)
    }

    async fn offset_max(&self) -> ASCOMResult<i32> {
        self.ensure_connected()?;
        if !self.offset_available() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        Ok(offset_max_from_bit_depth(self.info.bit_depth))
    }

    async fn set_offset(&self, offset: i32) -> ASCOMResult<()> {
        self.ensure_connected()?;
        if !self.offset_available() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        let max = offset_max_from_bit_depth(self.info.bit_depth);
        if offset < 0 || offset > max {
            return Err(ASCOMError::invalid_value(format!(
                "offset {offset} outside [0, {max}]"
            )));
        }
        self.on_handle(move |h| {
            h.set_offset(offset)
                .map_err(|_| ASCOMError::INVALID_OPERATION)
        })
        .await
    }

    // --- readout modes ----------------------------------------------------------

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

    // --- sensor type / bayer ----------------------------------------------------

    async fn sensor_type(&self) -> ASCOMResult<SensorType> {
        self.ensure_connected()?;
        Ok(if self.info.is_color {
            SensorType::RGGB
        } else {
            SensorType::Monochrome
        })
    }

    async fn bayer_offset_x(&self) -> ASCOMResult<u8> {
        self.ensure_connected()?;
        if !self.info.is_color {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        Ok(0)
    }

    async fn bayer_offset_y(&self) -> ASCOMResult<u8> {
        self.ensure_connected()?;
        if !self.info.is_color {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        Ok(0)
    }

    // --- cooling ----------------------------------------------------------------

    async fn can_set_ccd_temperature(&self) -> ASCOMResult<bool> {
        Ok(self.info.has_tec())
    }

    async fn can_get_cooler_power(&self) -> ASCOMResult<bool> {
        Ok(self.info.has_tec())
    }

    async fn ccd_temperature(&self) -> ASCOMResult<f64> {
        self.ensure_connected()?;
        // Decoupled from cooling: report the sensor temperature whenever the model
        // advertises a temperature read (cached at the open handshake), cooled or
        // not. A model without it is genuinely `NOT_IMPLEMENTED`.
        if !*self.state.temperature_available.lock() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        self.on_handle(|h| {
            h.temperature_celsius().map_err(|_| {
                ASCOMError::new(UNSPECIFIED_ERROR, "failed to read sensor temperature")
            })
        })
        .await
    }

    async fn set_ccd_temperature(&self) -> ASCOMResult<f64> {
        self.ensure_connected()?;
        if !self.info.has_tec() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        // Report the last setpoint written this session; before any write, fall
        // back to the model's current `OPTION_TECTARGET` (which has a power-on
        // default) so the getter never returns `VALUE_NOT_SET` — ConformU flags
        // that as an issue for a writable, supported property.
        if let Some(target) = *self.state.target_temperature.lock() {
            return Ok(target);
        }
        self.on_handle(|h| {
            h.target_temperature_tenths()
                .map(|tenths| f64::from(tenths) / 10.0)
                .map_err(|_| {
                    ASCOMError::new(
                        UNSPECIFIED_ERROR,
                        "failed to read cooler target temperature",
                    )
                })
        })
        .await
    }

    async fn set_set_ccd_temperature(&self, set_ccd_temperature: f64) -> ASCOMResult<()> {
        self.ensure_connected()?;
        if !self.info.has_tec() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        if !(-273.15..=80.0).contains(&set_ccd_temperature) {
            return Err(ASCOMError::invalid_value(format!(
                "target temperature {set_ccd_temperature} outside [-273.15, 80]"
            )));
        }
        // ToupTek `OPTION_TECTARGET` is in 0.1 °C units.
        let tenths = (set_ccd_temperature * 10.0).round() as i16;
        self.on_handle(move |h| {
            h.set_target_temperature_tenths(tenths)
                .map_err(|_| ASCOMError::invalid_operation("failed to set target temperature"))
        })
        .await?;
        *self.state.target_temperature.lock() = Some(set_ccd_temperature);
        Ok(())
    }

    async fn cooler_on(&self) -> ASCOMResult<bool> {
        self.ensure_connected()?;
        if !self.info.has_tec() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        self.on_handle(|h| {
            h.cooler_on()
                .map_err(|_| ASCOMError::new(UNSPECIFIED_ERROR, "failed to read cooler state"))
        })
        .await
    }

    async fn set_cooler_on(&self, cooler_on: bool) -> ASCOMResult<()> {
        self.ensure_connected()?;
        if !self.info.has_tec() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        self.on_handle(move |h| {
            h.set_cooler(cooler_on)
                .map_err(|_| ASCOMError::invalid_operation("failed to set cooler state"))
        })
        .await
    }

    async fn cooler_power(&self) -> ASCOMResult<f64> {
        self.ensure_connected()?;
        if !self.info.has_tec() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        self.on_handle(|h| {
            h.cooler_power_percent()
                .map(f64::from)
                .map_err(|_| ASCOMError::new(UNSPECIFIED_ERROR, "failed to read cooler power"))
        })
        .await
    }

    // --- shutter / capability flags ---------------------------------------------

    async fn has_shutter(&self) -> ASCOMResult<bool> {
        // ToupTek sensors are shutterless; darks/bias differ only in client
        // metadata (E4).
        Ok(false)
    }

    async fn can_abort_exposure(&self) -> ASCOMResult<bool> {
        Ok(true)
    }

    async fn can_stop_exposure(&self) -> ASCOMResult<bool> {
        // Trigger mode yields one whole frame with no partial readout, so there is
        // no data-preserving graceful stop (E8, the divergence from zwo-camera).
        Ok(false)
    }

    async fn can_pulse_guide(&self) -> ASCOMResult<bool> {
        Ok(self.info.has_st4())
    }

    async fn is_pulse_guiding(&self) -> ASCOMResult<bool> {
        // Asynchronous: `pulse_guide` returns immediately and records a deadline;
        // the pulse is in progress until that deadline passes (PG2).
        Ok(match *self.state.pulse_guide_until.lock() {
            Some(deadline) => SystemTime::now() < deadline,
            None => false,
        })
    }

    // --- exposure state ---------------------------------------------------------

    async fn camera_state(&self) -> ASCOMResult<CameraState> {
        if self.state.last_error.lock().is_some() {
            return Ok(CameraState::Error);
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
        // ASCOM: `ImageArray` is valid only once `ImageReady` is true. Mirror the
        // `image_ready()` condition so a client can never read a stale frame.
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

    // --- exposure control -------------------------------------------------------

    async fn start_exposure(&self, duration: Duration, light: bool) -> ASCOMResult<()> {
        self.ensure_connected()?;
        if self.state.exposure_in_flight.load(Ordering::Acquire) {
            return Err(ASCOMError::invalid_operation(
                "an exposure is already in flight",
            ));
        }

        let (min_us, max_us) =
            (*self.state.exposure_range_us.lock()).ok_or(ASCOMError::INVALID_VALUE)?;
        let exposure_us = (duration.as_secs_f64() * 1_000_000.0).round();
        if exposure_us < f64::from(min_us) || exposure_us > f64::from(max_us) {
            return Err(ASCOMError::invalid_value(format!(
                "exposure {exposure_us}us outside [{min_us}, {max_us}]"
            )));
        }
        // Bounded into `[min_us, max_us]` (a `u32` range), so the cast is exact.
        let exposure_us = exposure_us as u32;

        let bin = u32::from(self.state.bin.load(Ordering::Acquire)).max(1);
        let roi = self.validated_geometry(bin)?;

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
        *self.state.last_error.lock() = None;
        *self.state.last_exposure_start_time.lock() = Some(SystemTime::now());
        *self.state.last_exposure_duration.lock() = Some(duration);

        let request = CaptureRequest {
            width: roi.width,
            height: roi.height,
            bin,
            start_x: roi.start_x,
            start_y: roi.start_y,
            exposure_us,
            bit_depth: self.info.bit_depth,
            duration,
            is_dark: !light,
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

    async fn pulse_guide(&self, direction: GuideDirection, duration: Duration) -> ASCOMResult<()> {
        self.ensure_connected()?;
        if !self.info.has_st4() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        let dir = guide_direction(direction);
        let duration_ms = u32::try_from(duration.as_millis()).unwrap_or(u32::MAX);

        // ASCOM `PulseGuide` is asynchronous: start the pulse now (so a failed
        // start is reported to the caller) and return immediately. The ToupCam SDK
        // times the pulse internally, so there is no separate "stop" call;
        // `IsPulseGuiding` is true until the recorded deadline (PG2).
        let on_handle = Arc::clone(&self.handle);
        tokio::task::spawn_blocking(move || on_handle.pulse_guide(dir, duration_ms))
            .await
            .map_err(|e| ASCOMError::invalid_operation(format!("pulse guide task failed: {e}")))?
            .map_err(|e| ASCOMError::invalid_operation(format!("pulse guide failed: {e}")))?;

        *self.state.pulse_guide_until.lock() = Some(SystemTime::now() + duration);

        let state = Arc::clone(&self.state);
        tokio::spawn(async move {
            tokio::time::sleep(duration).await;
            *state.pulse_guide_until.lock() = None;
        });
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::backend::mock::MockCameraHandle;

    fn roi(start_x: u32, start_y: u32, width: u32, height: u32) -> Roi {
        Roi {
            start_x,
            start_y,
            width,
            height,
        }
    }

    fn connected_device(handle: MockCameraHandle) -> TouptekCamera {
        let device = TouptekCamera::new(Arc::new(handle), None);
        device.connect().unwrap();
        device
    }

    // Deadline-bounded polls (no fixed nap count): `tokio::time::timeout` caps the
    // wait in real time, so a contended runtime can't turn a fixed iteration count
    // into an unbounded wall-clock wait.
    async fn wait_image_ready(device: &TouptekCamera) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !device.image_ready().await.unwrap() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("exposure did not complete");
    }

    async fn wait_camera_state(device: &TouptekCamera, want: CameraState) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while device.camera_state().await.unwrap() != want {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("camera did not reach {want:?}"));
    }

    // --- pure helpers -----------------------------------------------------------

    #[test]
    fn max_adu_is_two_pow_bits_minus_one() {
        assert_eq!(max_adu_from_bit_depth(16), 65_535);
        assert_eq!(max_adu_from_bit_depth(14), 16_383);
        assert_eq!(max_adu_from_bit_depth(12), 4_095);
        assert_eq!(max_adu_from_bit_depth(0), 0);
    }

    #[test]
    fn offset_max_scales_with_bit_depth() {
        // TOUPCAM_BLACKLEVEL*_MAX: 31 * 2^(bits - 8).
        assert_eq!(offset_max_from_bit_depth(8), 31);
        assert_eq!(offset_max_from_bit_depth(12), 31 * 16);
        assert_eq!(offset_max_from_bit_depth(16), 31 * 256);
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
    fn check_geometry_rejects_zero_misaligned_and_out_of_bounds() {
        // zero
        assert!(check_geometry(roi(0, 0, 0, 64), 6248, 4176, 1).is_err());
        assert!(check_geometry(roi(0, 0, 64, 0), 6248, 4176, 1).is_err());
        // misaligned (odd offset or size)
        assert!(check_geometry(roi(0, 0, 65, 64), 6248, 4176, 1).is_err());
        assert!(check_geometry(roi(0, 0, 64, 47), 6248, 4176, 1).is_err());
        assert!(check_geometry(roi(9, 0, 64, 64), 6248, 4176, 1).is_err());
        // too small (even but < 8)
        assert!(check_geometry(roi(0, 0, 4, 64), 6248, 4176, 1).is_err());
        // out of bounds in x and y
        assert!(check_geometry(roi(0, 0, 8000, 64), 6248, 4176, 1).is_err());
        assert!(check_geometry(roi(0, 0, 64, 6000), 6248, 4176, 1).is_err());
        assert!(check_geometry(roi(6248, 0, 64, 64), 6248, 4176, 1).is_err());
        assert!(check_geometry(roi(0, 4176, 64, 64), 6248, 4176, 1).is_err());
        // valid full + sub frames
        assert!(check_geometry(roi(0, 0, 6248, 4176), 6248, 4176, 1).is_ok());
        assert!(check_geometry(roi(0, 0, 64, 48), 6248, 4176, 1).is_ok());
        // binned full frames at every supported bin
        assert!(check_geometry(roi(0, 0, 3124, 2088), 6248, 4176, 2).is_ok());
        assert!(check_geometry(roi(0, 0, 2082, 1392), 6248, 4176, 3).is_ok());
        assert!(check_geometry(roi(0, 0, 1562, 1044), 6248, 4176, 4).is_ok());
    }

    #[test]
    fn aligned_extent_is_a_no_op_for_the_simulated_sensor() {
        // 6248 / {1,2,3,4} and 4176 / {1,2,3,4} are all even, so the reported
        // extent is the raw sensor size (the simplification over zwo-camera).
        assert_eq!(aligned_sensor_extent(6248, &[1, 2, 3, 4]), 6248);
        assert_eq!(aligned_sensor_extent(4176, &[1, 2, 3, 4]), 4176);
        for bin in [1u32, 2, 3, 4] {
            assert_eq!((6248 / bin) % 2, 0, "width / {bin} not even");
            assert_eq!((4176 / bin) % 2, 0, "height / {bin} not even");
        }
        // A model whose floored binned full frame would be odd at some bin is
        // reduced: 6250 / 4 = 1562 (even), but 6254 / 4 = 1563 (odd) → reduce.
        let aligned = aligned_sensor_extent(6254, &[1, 2, 3, 4]);
        for bin in [1u32, 2, 3, 4] {
            assert_eq!((aligned / bin) % 2, 0, "reduced width / {bin} not even");
        }
        assert!(aligned <= 6254);
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
    fn to_image_array_rejects_short_buffer() {
        assert!(to_image_array(&[0u8; 10], 64, 48).is_err());
    }

    #[test]
    fn guide_direction_maps_every_direction() {
        assert_eq!(
            guide_direction(GuideDirection::North),
            touptek_rs::GuideDirection::North
        );
        assert_eq!(
            guide_direction(GuideDirection::South),
            touptek_rs::GuideDirection::South
        );
        assert_eq!(
            guide_direction(GuideDirection::East),
            touptek_rs::GuideDirection::East
        );
        assert_eq!(
            guide_direction(GuideDirection::West),
            touptek_rs::GuideDirection::West
        );
    }

    // --- device behaviour via the mock seam -------------------------------------

    #[tokio::test]
    async fn connect_caches_geometry_and_limits() {
        let device = connected_device(MockCameraHandle::default());
        assert_eq!(device.camera_x_size().await.unwrap(), 6248);
        assert_eq!(device.camera_y_size().await.unwrap(), 4176);
        assert_eq!(device.max_adu().await.unwrap(), 65_535);
        assert_eq!(device.max_bin_x().await.unwrap(), 4);
        assert!(!device.can_asymmetric_bin().await.unwrap());
        assert_eq!(device.sensor_type().await.unwrap(), SensorType::Monochrome);
        assert!(!device.has_shutter().await.unwrap());
        assert_eq!(device.gain_min().await.unwrap(), 100);
        assert_eq!(device.gain_max().await.unwrap(), 1000);
        assert_eq!(device.offset_min().await.unwrap(), 0);
        assert_eq!(device.offset_max().await.unwrap(), 31 * 256);
        // ElectronsPerADU / FullWellCapacity are NOT_IMPLEMENTED (the QHY-like
        // divergence) — both fall through to the trait default.
        assert_eq!(
            device.electrons_per_adu().await.unwrap_err().code,
            ASCOMErrorCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            device.full_well_capacity().await.unwrap_err().code,
            ASCOMErrorCode::NOT_IMPLEMENTED
        );
    }

    #[tokio::test]
    async fn connect_open_failure_leaves_disconnected() {
        // C2: an SDK open failure leaves the device *disconnected*, not
        // opened-but-unusable.
        let handle = MockCameraHandle::default();
        handle.fail_open.store(true, Ordering::SeqCst);
        let device = TouptekCamera::new(Arc::new(handle), None);
        assert_eq!(
            device.set_connected(true).await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert!(!device.connected().await.unwrap());
    }

    #[tokio::test]
    async fn roi_getters_reflect_the_connected_roi() {
        let device = connected_device(MockCameraHandle::default());
        // The default ROI is the full frame at the origin.
        assert_eq!(device.num_x().await.unwrap(), 6248);
        assert_eq!(device.num_y().await.unwrap(), 4176);
        assert_eq!(device.start_x().await.unwrap(), 0);
        assert_eq!(device.start_y().await.unwrap(), 0);
        // The relaxed setters round-trip through the getters (R1).
        device.set_num_x(800).await.unwrap();
        device.set_num_y(600).await.unwrap();
        device.set_start_x(16).await.unwrap();
        device.set_start_y(8).await.unwrap();
        assert_eq!(device.num_x().await.unwrap(), 800);
        assert_eq!(device.num_y().await.unwrap(), 600);
        assert_eq!(device.start_x().await.unwrap(), 16);
        assert_eq!(device.start_y().await.unwrap(), 8);
    }

    #[tokio::test]
    async fn exposure_range_getters_reflect_the_caps() {
        let device = connected_device(MockCameraHandle::default());
        assert_eq!(
            device.exposure_min().await.unwrap(),
            Duration::from_micros(100)
        );
        assert_eq!(
            device.exposure_max().await.unwrap(),
            Duration::from_micros(3_600_000_000)
        );
        assert_eq!(
            device.exposure_resolution().await.unwrap(),
            Duration::from_micros(1)
        );
    }

    #[tokio::test]
    async fn cooling_round_trips_on_a_cooled_model() {
        let device = connected_device(MockCameraHandle::default());
        assert!(device.can_set_ccd_temperature().await.unwrap());
        assert!(device.can_get_cooler_power().await.unwrap());
        // Before any setpoint write, the setpoint getter falls back to the
        // model's power-on `OPTION_TECTARGET` default (0 °C in the mock) rather
        // than erroring — ConformU flags a `VALUE_NOT_SET` read as an issue.
        assert_eq!(device.set_ccd_temperature().await.unwrap(), 0.0);
        // ...and reflects the cached value after a write.
        device.set_set_ccd_temperature(-10.0).await.unwrap();
        assert!((device.set_ccd_temperature().await.unwrap() - (-10.0)).abs() < f64::EPSILON);
        // The cooler toggles and drives the reported sensor temperature + power.
        assert!(!device.cooler_on().await.unwrap());
        device.set_cooler_on(true).await.unwrap();
        assert!(device.cooler_on().await.unwrap());
        assert!((device.ccd_temperature().await.unwrap() - (-10.0)).abs() < f64::EPSILON);
        assert!(device.cooler_power().await.unwrap() > 0.0);
    }

    #[tokio::test]
    async fn out_of_range_target_temperature_is_rejected() {
        let device = connected_device(MockCameraHandle::default());
        assert_eq!(
            device
                .set_set_ccd_temperature(-300.0)
                .await
                .unwrap_err()
                .code,
            ASCOMErrorCode::INVALID_VALUE
        );
        assert_eq!(
            device
                .set_set_ccd_temperature(100.0)
                .await
                .unwrap_err()
                .code,
            ASCOMErrorCode::INVALID_VALUE
        );
    }

    #[tokio::test]
    async fn cooling_setpoint_members_not_implemented_on_uncooled_model() {
        // K1: the cooler-setpoint members stay gated on FLAG_TEC...
        let device = connected_device(MockCameraHandle::default().without_cooler());
        assert!(!device.can_set_ccd_temperature().await.unwrap());
        assert!(!device.can_get_cooler_power().await.unwrap());
        assert_eq!(
            device.set_ccd_temperature().await.unwrap_err().code,
            ASCOMErrorCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            device.cooler_power().await.unwrap_err().code,
            ASCOMErrorCode::NOT_IMPLEMENTED
        );
    }

    #[tokio::test]
    async fn ccd_temperature_not_implemented_without_sensor() {
        // K2: a model with no sensor-temperature read reports NOT_IMPLEMENTED.
        let device = connected_device(MockCameraHandle::default().without_temperature());
        assert_eq!(
            device.ccd_temperature().await.unwrap_err().code,
            ASCOMErrorCode::NOT_IMPLEMENTED
        );
    }

    #[tokio::test]
    async fn sensor_type_and_bayer_gate_on_color() {
        // Mono: BayerOffsetX/Y are NOT_IMPLEMENTED (ST1).
        let mono = connected_device(MockCameraHandle::default());
        assert_eq!(mono.sensor_type().await.unwrap(), SensorType::Monochrome);
        assert_eq!(
            mono.bayer_offset_x().await.unwrap_err().code,
            ASCOMErrorCode::NOT_IMPLEMENTED
        );
        // Colour: SensorType is RGGB and BayerOffsetX/Y answer.
        let color = connected_device(MockCameraHandle::default().with_color());
        assert_eq!(color.sensor_type().await.unwrap(), SensorType::RGGB);
        assert_eq!(color.bayer_offset_x().await.unwrap(), 0);
        assert_eq!(color.bayer_offset_y().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn gain_out_of_range_is_rejected_and_round_trips() {
        let device = connected_device(MockCameraHandle::default());
        let max = device.gain_max().await.unwrap();
        device.set_gain(max).await.unwrap();
        assert_eq!(device.gain().await.unwrap(), max);
        let err = device.set_gain(max + 1).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
    }

    #[tokio::test]
    async fn gain_not_implemented_without_control() {
        // GO1: a model with no gain control reports NOT_IMPLEMENTED.
        let device = connected_device(MockCameraHandle::default().without_gain());
        assert_eq!(
            device.gain().await.unwrap_err().code,
            ASCOMErrorCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            device.gain_min().await.unwrap_err().code,
            ASCOMErrorCode::NOT_IMPLEMENTED
        );
    }

    #[tokio::test]
    async fn offset_round_trips_and_out_of_range_is_rejected() {
        let device = connected_device(MockCameraHandle::default());
        device.set_offset(100).await.unwrap();
        assert_eq!(device.offset().await.unwrap(), 100);
        assert_eq!(
            device.set_offset(-1).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_VALUE
        );
        let max = device.offset_max().await.unwrap();
        assert_eq!(
            device.set_offset(max + 1).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_VALUE
        );
    }

    #[tokio::test]
    async fn offset_not_implemented_without_control() {
        // GO1: a model with no black-level control reports NOT_IMPLEMENTED.
        let device = connected_device(MockCameraHandle::default().without_offset());
        assert_eq!(
            device.offset().await.unwrap_err().code,
            ASCOMErrorCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            device.offset_max().await.unwrap_err().code,
            ASCOMErrorCode::NOT_IMPLEMENTED
        );
    }

    #[tokio::test]
    async fn readout_modes_are_listed_and_out_of_range_is_rejected() {
        let device = connected_device(MockCameraHandle::default());
        assert!(!device.readout_modes().await.unwrap().is_empty());
        assert!(device.readout_mode().await.unwrap() < READOUT_MODES.len());
        assert_eq!(
            device.set_readout_mode(9999).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_VALUE
        );
    }

    #[tokio::test]
    async fn bin_change_rescales_roi_and_rejects_unsupported() {
        let device = connected_device(MockCameraHandle::default());
        device.set_num_x(3124).await.unwrap();
        device.set_num_y(2088).await.unwrap();
        device.set_bin_x(2).await.unwrap();
        assert_eq!(device.bin_x().await.unwrap(), 2);
        assert_eq!(device.bin_y().await.unwrap(), 2);
        assert_eq!(device.num_x().await.unwrap(), 1562);
        assert_eq!(device.num_y().await.unwrap(), 1044);
        assert_eq!(
            device.set_bin_x(99).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_VALUE
        );
        assert_eq!(
            device.set_bin_x(0).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_VALUE
        );
    }

    #[tokio::test]
    async fn binned_full_frame_passes_geometry_at_every_bin() {
        // ConformU takes the full frame at every bin via NumX = CameraXSize / bin;
        // every floored binned full frame is even and exposes (R4).
        for bin in [2u8, 3, 4] {
            let device = connected_device(MockCameraHandle::default());
            let w = device.camera_x_size().await.unwrap() / u32::from(bin);
            let h = device.camera_y_size().await.unwrap() / u32::from(bin);
            assert_eq!(w % 2, 0);
            assert_eq!(h % 2, 0);
            device.set_bin_x(bin).await.unwrap();
            device.set_start_x(0).await.unwrap();
            device.set_start_y(0).await.unwrap();
            device.set_num_x(w).await.unwrap();
            device.set_num_y(h).await.unwrap();
            device
                .start_exposure(Duration::from_millis(10), true)
                .await
                .unwrap_or_else(|e| panic!("bin {bin} full frame rejected: {e:?}"));
            wait_image_ready(&device).await;
            assert!(device.image_ready().await.unwrap(), "bin {bin} no image");
        }
    }

    #[tokio::test]
    async fn disconnected_reads_and_start_exposure_are_not_connected() {
        let device = TouptekCamera::new(Arc::new(MockCameraHandle::default()), None);
        assert_eq!(
            device.camera_x_size().await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device
                .start_exposure(Duration::from_millis(10), true)
                .await
                .unwrap_err()
                .code,
            ASCOMErrorCode::NOT_CONNECTED
        );
    }

    #[tokio::test]
    async fn dark_frame_is_accepted_on_shutterless_camera() {
        // E4: darks are accepted (no mechanical shutter).
        let device = connected_device(MockCameraHandle::default());
        assert!(!device.has_shutter().await.unwrap());
        device.set_num_x(64).await.unwrap();
        device.set_num_y(48).await.unwrap();
        device
            .start_exposure(Duration::from_millis(10), false)
            .await
            .unwrap();
        wait_image_ready(&device).await;
        assert!(device.image_ready().await.unwrap());
    }

    #[tokio::test]
    async fn successful_exposure_produces_image() {
        let device = connected_device(MockCameraHandle::default());
        device.set_num_x(64).await.unwrap();
        device.set_num_y(48).await.unwrap();
        device.set_start_x(0).await.unwrap();
        device.set_start_y(0).await.unwrap();
        device
            .start_exposure(Duration::from_millis(10), true)
            .await
            .unwrap();
        wait_image_ready(&device).await;
        assert_eq!(device.camera_state().await.unwrap(), CameraState::Idle);
        assert_eq!(device.percent_completed().await.unwrap(), 100);
        device.last_exposure_start_time().await.unwrap();
        let image = device.image_array().await.unwrap();
        assert_eq!(image.dim().0, 64);
        assert_eq!(image.dim().1, 48);
    }

    #[tokio::test]
    async fn out_of_range_duration_is_rejected() {
        let device = connected_device(MockCameraHandle::default());
        device.set_num_x(64).await.unwrap();
        device.set_num_y(48).await.unwrap();
        // 100000 s = 1e11 us, beyond the cached max (3.6e9 us).
        let err = device
            .start_exposure(Duration::from_secs(100_000), true)
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
    }

    #[tokio::test]
    async fn mid_exposure_error_transitions_to_error_state() {
        // E9: a mid-exposure SDK failure surfaces as the Error state + a failing
        // image_array.
        let handle = MockCameraHandle::default();
        handle.fail_capture.store(true, Ordering::SeqCst);
        let device = connected_device(handle);
        device.set_num_x(64).await.unwrap();
        device.set_num_y(48).await.unwrap();
        device
            .start_exposure(Duration::from_millis(10), true)
            .await
            .unwrap();
        wait_camera_state(&device, CameraState::Error).await;
        assert!(!device.image_ready().await.unwrap());
        assert_eq!(
            device.image_array().await.unwrap_err().code,
            UNSPECIFIED_ERROR
        );
    }

    #[tokio::test]
    async fn reconnect_clears_error_state() {
        let handle = Arc::new(MockCameraHandle::default());
        handle.fail_capture.store(true, Ordering::SeqCst);
        let device = TouptekCamera::new(handle.clone(), None);
        device.set_connected(true).await.unwrap();
        device.set_num_x(64).await.unwrap();
        device.set_num_y(48).await.unwrap();
        device
            .start_exposure(Duration::from_millis(10), true)
            .await
            .unwrap();
        wait_camera_state(&device, CameraState::Error).await;
        device.set_connected(false).await.unwrap();
        handle.fail_capture.store(false, Ordering::SeqCst);
        device.set_connected(true).await.unwrap();
        assert_eq!(device.camera_state().await.unwrap(), CameraState::Idle);
        assert!(!device.image_ready().await.unwrap());
    }

    #[tokio::test]
    async fn second_exposure_while_in_flight_is_rejected() {
        // E2: a second StartExposure while one is in flight is rejected.
        let handle = MockCameraHandle::default();
        handle.set_capture_delay(Duration::from_secs(5));
        let device = connected_device(handle);
        device.set_num_x(64).await.unwrap();
        device.set_num_y(48).await.unwrap();
        device
            .start_exposure(Duration::from_secs(5), true)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(device.camera_state().await.unwrap(), CameraState::Exposing);
        let err = device
            .start_exposure(Duration::from_millis(10), true)
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
        device.abort_exposure().await.unwrap();
    }

    #[tokio::test]
    async fn abort_discards_the_frame() {
        // E7: abort discards the in-flight frame.
        let handle = MockCameraHandle::default();
        handle.set_capture_delay(Duration::from_secs(5));
        let device = connected_device(handle);
        device.set_num_x(64).await.unwrap();
        device.set_num_y(48).await.unwrap();
        assert!(device.can_abort_exposure().await.unwrap());
        device
            .start_exposure(Duration::from_secs(5), true)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        device.abort_exposure().await.unwrap();
        // No fresh frame is ready after an abort. Best-effort, deadline-bounded
        // wait (not a fixed nap count) for the in-flight flag to clear.
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            while device.state.exposure_in_flight.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        assert!(!device.image_ready().await.unwrap());
        assert_eq!(
            device.image_array().await.unwrap_err().code,
            ASCOMErrorCode::INVALID_OPERATION
        );
    }

    #[tokio::test]
    async fn graceful_stop_is_not_supported() {
        // E8: trigger mode has no data-preserving stop (the divergence from ZWO).
        let device = connected_device(MockCameraHandle::default());
        assert!(!device.can_stop_exposure().await.unwrap());
    }

    #[tokio::test]
    async fn pulse_guide_is_asynchronous() {
        // PG2: PulseGuide returns promptly and IsPulseGuiding is true until the
        // deadline, then false.
        let device = connected_device(MockCameraHandle::default());
        assert!(device.can_pulse_guide().await.unwrap());
        assert!(!device.is_pulse_guiding().await.unwrap());
        device
            .pulse_guide(GuideDirection::North, Duration::from_millis(200))
            .await
            .unwrap();
        assert!(device.is_pulse_guiding().await.unwrap());
        tokio::time::sleep(Duration::from_millis(260)).await;
        assert!(!device.is_pulse_guiding().await.unwrap());
    }

    #[tokio::test]
    async fn pulse_guide_not_implemented_without_st4() {
        // PG2: a model with no ST4 port reports NOT_IMPLEMENTED.
        let device = connected_device(MockCameraHandle::default().without_st4());
        assert!(!device.can_pulse_guide().await.unwrap());
        assert_eq!(
            device
                .pulse_guide(GuideDirection::North, Duration::from_millis(1))
                .await
                .unwrap_err()
                .code,
            ASCOMErrorCode::NOT_IMPLEMENTED
        );
    }

    #[tokio::test]
    async fn pulse_guide_disconnected_is_not_connected() {
        let device = TouptekCamera::new(Arc::new(MockCameraHandle::default()), None);
        assert_eq!(
            device
                .pulse_guide(GuideDirection::North, Duration::from_millis(1))
                .await
                .unwrap_err()
                .code,
            ASCOMErrorCode::NOT_CONNECTED
        );
    }

    #[tokio::test]
    async fn disconnect_cancels_in_flight_exposure() {
        // C3: a disconnect cancels an in-flight exposure.
        let handle = MockCameraHandle::default();
        handle.set_capture_delay(Duration::from_secs(5));
        let device = connected_device(handle);
        device.set_num_x(64).await.unwrap();
        device.set_num_y(48).await.unwrap();
        device
            .start_exposure(Duration::from_secs(5), true)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        device.set_connected(false).await.unwrap();
        assert!(!device.connected().await.unwrap());
        assert!(!device.image_ready().await.unwrap());
    }

    #[tokio::test]
    async fn unique_id_is_id_derived_and_non_empty() {
        let device = TouptekCamera::new(Arc::new(MockCameraHandle::default()), None);
        assert!(!device.unique_id().is_empty());
        assert!(device.unique_id().contains("sim-0"));
    }

    #[tokio::test]
    async fn connection_flag_round_trips() {
        let device = TouptekCamera::new(Arc::new(MockCameraHandle::default()), None);
        assert!(!device.connected().await.unwrap());
        device.set_connected(true).await.unwrap();
        assert!(device.connected().await.unwrap());
        device.set_connected(false).await.unwrap();
        assert!(!device.connected().await.unwrap());
    }

    #[tokio::test]
    async fn connect_is_per_device_and_independent() {
        // C4: each device owns its handle + state, so connecting one does not
        // affect another enumerated on the same service. (The single-camera
        // simulator cannot exercise this in BDD, so it is unit-tested here with
        // two independent mock-backed devices.)
        let a = TouptekCamera::new(Arc::new(MockCameraHandle::default()), None);
        let b = TouptekCamera::new(Arc::new(MockCameraHandle::default()), None);
        a.set_connected(true).await.unwrap();
        assert!(a.connected().await.unwrap());
        assert!(
            !b.connected().await.unwrap(),
            "connecting A must not connect B"
        );
        b.set_connected(true).await.unwrap();
        a.set_connected(false).await.unwrap();
        assert!(!a.connected().await.unwrap());
        assert!(
            b.connected().await.unwrap(),
            "disconnecting A must not disconnect B"
        );
    }
}
