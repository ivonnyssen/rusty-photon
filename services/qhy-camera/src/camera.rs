//! `QhyCameraDevice` — the ASCOM `Device` + `Camera` implementation over the
//! [`CameraHandle`](crate::backend::CameraHandle) seam.
//!
//! Behaviour is ported from the author's standalone `qhyccd-alpaca` driver and
//! re-expressed against rusty-photon conventions, with these deliberate
//! divergences (see `docs/services/qhy-camera.md`):
//! - **MaxADU** = `2^OutputDataActualBits - 1` (65535 for 16-bit), not `2^bits`.
//! - **ROI validation** rejects a zero or out-of-bounds sub-frame via
//!   `StartX + NumX > CameraXSize / BinX` (contract R2), not the reference's
//!   `StartX > NumX`.
//! - **Dark frames** return `NOT_IMPLEMENTED` (qhyccd-rs 0.1.9 has no shutter
//!   actuation; contract E4 degraded form).
//! - A real **`Error` `CameraState`** (E9) when a mid-exposure SDK call fails.
//! - **PercentCompleted** is percent *done*, clamped, 100 when idle (E6).
//!
//! Blocking exposure SDK calls run on `spawn_blocking` inside a detached task; a
//! generation counter lets abort/disconnect invalidate a late-completing task.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ascom_alpaca::api::camera::{CameraState, ImageArray, SensorType};
use ascom_alpaca::api::{Camera, Device};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use ndarray::Array2;
use parking_lot::Mutex;
use qhyccd_rs::{BayerMode, CCDChipArea, Control, ImageData};
use rusty_photon_driver::ConfigActionCtx;
use tracing::{debug, warn};

use crate::backend::{BackendError, CameraHandle};
use crate::config::DeviceOverride;
use crate::config_actions::QhyCameraDriver;

/// 0x500 — driver-specific catch-all for an asynchronous capture failure
/// surfaced lazily via `image_array`.
const UNSPECIFIED_ERROR: ASCOMErrorCode = ASCOMErrorCode::new_for_driver(0);

/// Per-device runtime state: caches populated at connect plus the exposure state
/// machine. Atomics for the hot/simple flags; `parking_lot::Mutex` for the
/// `Option<…>` caches and the captured image. Locks are never held across an
/// `await`.
#[derive(Debug)]
struct DeviceState {
    /// Current symmetric bin (init 1).
    bin: AtomicU8,
    valid_bins: Mutex<Vec<u8>>,
    ccd_info: Mutex<Option<CachedCcdInfo>>,
    /// Intended ROI in *binned* pixel coordinates (rescaled on bin change).
    intended_roi: Mutex<Option<CCDChipArea>>,
    exposure_range_us: Mutex<Option<(f64, f64, f64)>>,
    gain_min_max: Mutex<Option<(f64, f64)>>,
    offset_min_max: Mutex<Option<(f64, f64)>>,
    target_temperature: Mutex<Option<f64>>,

    exposure_in_flight: AtomicBool,
    image_ready: AtomicBool,
    /// Bumped on each start / abort / disconnect so a late-completing capture
    /// task can tell it has been superseded and discard its result.
    exposure_generation: AtomicU64,
    expected_duration_us: AtomicU64,
    last_exposure_start_time: Mutex<Option<SystemTime>>,
    last_exposure_duration: Mutex<Option<Duration>>,
    last_image: Mutex<Option<ImageArray>>,
    /// Set on a mid-exposure SDK failure → `CameraState::Error` (E9).
    last_error: Mutex<Option<String>>,
    /// Serializes the capture task's "check generation + commit result" against
    /// `cancel_exposure`'s "bump generation + clear image_ready", so an abort
    /// landing at the wrong instant can't leave a stale `ImageReady = true`.
    result_lock: Mutex<()>,
}

/// Cached sensor geometry. `image_width`/`image_height` track the active readout
/// mode (mutated by `set_readout_mode`); the rest is fixed at connect.
#[derive(Debug, Clone, Copy)]
struct CachedCcdInfo {
    image_width: u32,
    image_height: u32,
    pixel_width: f64,
    pixel_height: f64,
    bits_per_pixel: u32,
}

impl DeviceState {
    fn new() -> Self {
        Self {
            bin: AtomicU8::new(1),
            valid_bins: Mutex::new(Vec::new()),
            ccd_info: Mutex::new(None),
            intended_roi: Mutex::new(None),
            exposure_range_us: Mutex::new(None),
            gain_min_max: Mutex::new(None),
            offset_min_max: Mutex::new(None),
            target_temperature: Mutex::new(None),
            exposure_in_flight: AtomicBool::new(false),
            image_ready: AtomicBool::new(false),
            exposure_generation: AtomicU64::new(0),
            expected_duration_us: AtomicU64::new(0),
            last_exposure_start_time: Mutex::new(None),
            last_exposure_duration: Mutex::new(None),
            last_image: Mutex::new(None),
            last_error: Mutex::new(None),
            result_lock: Mutex::new(()),
        }
    }

    /// Reset the exposure state machine to a clean idle state. Called on connect
    /// so a stale `Error` / `ImageReady` / image from a previous session does not
    /// survive a reconnect (C3).
    fn reset_exposure_state(&self) {
        let _guard = self.result_lock.lock();
        self.exposure_generation.fetch_add(1, Ordering::AcqRel);
        self.exposure_in_flight.store(false, Ordering::Release);
        self.image_ready.store(false, Ordering::Release);
        self.expected_duration_us.store(0, Ordering::Release);
        *self.last_image.lock() = None;
        *self.last_error.lock() = None;
        *self.last_exposure_start_time.lock() = None;
        *self.last_exposure_duration.lock() = None;
    }
}

/// One ASCOM Camera device per discovered QHY camera.
#[derive(Clone, derive_more::Debug)]
pub struct QhyCameraDevice {
    #[debug(skip)]
    handle: Arc<dyn CameraHandle>,
    unique_id: String,
    name: String,
    description: String,
    state: Arc<DeviceState>,
    #[debug(skip)]
    config_ctx: Option<ConfigActionCtx<QhyCameraDriver>>,
}

impl QhyCameraDevice {
    /// Build a device from an SDK handle and an optional per-serial config
    /// override. The ASCOM `UniqueID` is the SDK serial; `name`/`description`
    /// fall back to SDK-derived defaults.
    pub fn new(handle: Arc<dyn CameraHandle>, overrides: Option<&DeviceOverride>) -> Self {
        let id = handle.id();
        let name = overrides
            .and_then(|o| o.name.clone())
            .unwrap_or_else(|| id.clone());
        let description = overrides
            .and_then(|o| o.description.clone())
            .unwrap_or_else(|| "QHYCCD camera".to_string());
        Self {
            handle,
            unique_id: id,
            name,
            description,
            state: Arc::new(DeviceState::new()),
            config_ctx: None,
        }
    }

    /// Attach config-action wiring (enables `config.get`/`apply`/`schema`).
    #[must_use]
    pub fn with_config_actions(mut self, ctx: ConfigActionCtx<QhyCameraDriver>) -> Self {
        self.config_ctx = Some(ctx);
        self
    }

    fn ensure_connected(&self) -> ASCOMResult<()> {
        match self.handle.is_open() {
            Ok(true) => Ok(()),
            _ => Err(ASCOMError::NOT_CONNECTED),
        }
    }

    fn connect(&self) -> ASCOMResult<()> {
        // `handle.open()` refcounts the shared physical connection
        // (`backend::SharedCameraConnection`): the open + refcount transition is
        // atomic. The post-open handshake below is not serialized against a racing
        // connect on the same device, but it is idempotent (re-applies stream mode
        // / readout / cached geometry on the shared handle), so a redundant run
        // from a concurrent connect is harmless.
        self.handle.open().map_err(|_| ASCOMError::NOT_CONNECTED)?;
        // If any step of the post-open handshake fails, close the handle before
        // propagating so a failed connect leaves Connected == false (C2) rather
        // than an opened-but-unusable camera.
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

    /// The open → single-frame → readout-mode-0 → init → 16-bit → cache handshake,
    /// run after `open()`. Caches CCD info, effective area, valid binning modes,
    /// and the exposure/gain/offset limits.
    fn open_handshake(&self) -> ASCOMResult<()> {
        let h = &self.handle;
        let nc = |_e: BackendError| ASCOMError::NOT_CONNECTED;
        if h.is_control_available(Control::CamSingleFrameMode)
            .is_none()
        {
            warn!("camera does not advertise single-frame mode");
            return Err(ASCOMError::NOT_CONNECTED);
        }
        h.set_stream_mode_single().map_err(nc)?;
        h.set_readout_mode(0).map_err(nc)?;
        h.init().map_err(nc)?;
        // Best-effort 16-bit transfer; not every model exposes the control.
        if let Err(e) = h.set_transfer_bit_16() {
            debug!(error = %e, "16-bit transfer not set");
        }

        let ccd = h.get_ccd_info().map_err(nc)?;
        *self.state.ccd_info.lock() = Some(CachedCcdInfo {
            image_width: ccd.image_width,
            image_height: ccd.image_height,
            pixel_width: ccd.pixel_width,
            pixel_height: ccd.pixel_height,
            bits_per_pixel: ccd.bits_per_pixel,
        });
        let area = h.get_effective_area().map_err(nc)?;
        *self.state.intended_roi.lock() = Some(area);
        *self.state.valid_bins.lock() = self.valid_binning_modes();

        let exposure = h
            .get_parameter_min_max_step(Control::Exposure)
            .map_err(nc)?;
        *self.state.exposure_range_us.lock() = Some(exposure);

        if h.is_control_available(Control::Gain).is_some() {
            let (min, max, _) = h.get_parameter_min_max_step(Control::Gain).map_err(nc)?;
            *self.state.gain_min_max.lock() = Some((min, max));
        }
        if h.is_control_available(Control::Offset).is_some() {
            let (min, max, _) = h.get_parameter_min_max_step(Control::Offset).map_err(nc)?;
            *self.state.offset_min_max.lock() = Some((min, max));
        }

        self.state.bin.store(1, Ordering::Release);
        Ok(())
    }

    async fn disconnect(&self) -> ASCOMResult<()> {
        // An in-flight exposure is cancelled (C3) before the handle closes.
        self.cancel_exposure();
        // CRITICAL: wait for the detached `run_exposure` capture task to finish its
        // blocking SDK chain before closing the handle. `cancel_exposure` issues
        // `abort_exposure_and_readout`, so `get_single_frame` returns promptly and
        // the task clears `exposure_in_flight`. Closing while that task is still
        // inside an FFI/libusb call would free the handle out from under a live USB
        // transfer — a use-after-free that trips libusb's `usbi_mutex_lock`
        // assertion and can corrupt the SDK's shared libusb context. Bounded so a
        // wedged SDK call cannot hang disconnect forever.
        let mut drained = !self.state.exposure_in_flight.load(Ordering::Acquire);
        for _ in 0..600 {
            if drained {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
            drained = !self.state.exposure_in_flight.load(Ordering::Acquire);
        }
        if !drained {
            warn!(camera = %self.unique_id, "in-flight exposure did not drain within 3s of disconnect; closing anyway");
        }
        // Refcounted close (`backend::SharedCameraConnection`): when a CFW device
        // shares this camera's SDK id, the physical handle is closed only once
        // both devices have disconnected, so disconnecting the camera no longer
        // breaks a concurrently-connected filter wheel. See the design doc.
        self.handle.close().map_err(|_| ASCOMError::NOT_CONNECTED)?;
        debug!(camera = %self.unique_id, "camera disconnected");
        Ok(())
    }

    /// Cancel any in-flight exposure: bump the generation (so the capture task
    /// discards its result), clear `image_ready`/`last_error`, and abort at the
    /// SDK. Deliberately does NOT clear `exposure_in_flight` — the capture task
    /// clears that once its blocking SDK chain has fully drained, so a new
    /// exposure cannot start and race the still-running SDK calls on the single
    /// device (the design's "one logical owner per device").
    fn cancel_exposure(&self) {
        if !self.state.exposure_in_flight.load(Ordering::Acquire) {
            return;
        }
        {
            // Atomic with the capture task's commit (run_exposure) so an abort can
            // never be overwritten by a just-completing capture.
            let _guard = self.state.result_lock.lock();
            self.state
                .exposure_generation
                .fetch_add(1, Ordering::AcqRel);
            self.state.image_ready.store(false, Ordering::Release);
            *self.state.last_error.lock() = None;
        }
        // Outside the result lock (the capture task takes it to drain) and not on
        // the async runtime thread's critical path — `abort_exposure_and_readout`
        // is the designed-concurrent SDK cancel for an in-progress readout.
        if let Err(e) = self.handle.abort_exposure_and_readout() {
            debug!(error = %e, "abort_exposure_and_readout failed");
        }
    }

    fn valid_binning_modes(&self) -> Vec<u8> {
        let mut bins = Vec::new();
        for (control, bin) in [
            (Control::CamBin1x1mode, 1u8),
            (Control::CamBin2x2mode, 2),
            (Control::CamBin3x3mode, 3),
            (Control::CamBin4x4mode, 4),
            (Control::CamBin6x6mode, 6),
            (Control::CamBin8x8mode, 8),
        ] {
            if self.handle.is_control_available(control).is_some() {
                bins.push(bin);
            }
        }
        bins
    }

    /// Validate the cached ROI against the binned sensor geometry (R2), returning
    /// the `CCDChipArea` to push to the SDK.
    fn validated_roi(&self) -> ASCOMResult<CCDChipArea> {
        let roi = (*self.state.intended_roi.lock())
            .ok_or_else(|| ASCOMError::invalid_value("no ROI defined for camera"))?;
        let ccd = (*self.state.ccd_info.lock()).ok_or(ASCOMError::VALUE_NOT_SET)?;
        let bin = u32::from(self.state.bin.load(Ordering::Acquire)).max(1);
        check_geometry(roi, ccd.image_width, ccd.image_height, bin)?;
        Ok(roi)
    }
}

/// Geometry validation shared by `validated_roi` (R2). Rejects a zero or
/// out-of-bounds sub-frame.
fn check_geometry(roi: CCDChipArea, ccd_w: u32, ccd_h: u32, bin: u32) -> ASCOMResult<()> {
    if roi.width == 0 || roi.height == 0 {
        return Err(ASCOMError::invalid_value(
            "NumX and NumY must be greater than 0",
        ));
    }
    let max_x = ccd_w / bin;
    let max_y = ccd_h / bin;
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

/// Rescale a ROI (binned coords) by the `old/new` bin ratio (B3). Width/height
/// are clamped to a minimum of 1: rescaling a 1-pixel ROI to a larger bin would
/// otherwise truncate to 0, then the next `StartExposure` would fail geometry
/// validation with a confusing "NumX/NumY must be > 0" (which the user never set).
fn rescale_roi(roi: CCDChipArea, old: u8, new: u8) -> CCDChipArea {
    let factor = f64::from(old) / f64::from(new);
    CCDChipArea {
        start_x: (f64::from(roi.start_x) * factor) as u32,
        start_y: (f64::from(roi.start_y) * factor) as u32,
        width: ((f64::from(roi.width) * factor) as u32).max(1),
        height: ((f64::from(roi.height) * factor) as u32).max(1),
    }
}

/// `MaxADU = 2^bits - 1` (e.g. 65535 for a 16-bit sensor), saturating.
fn max_adu_from_bits(bits: u32) -> u32 {
    2u32.checked_pow(bits)
        .map_or(u32::MAX, |full| full.saturating_sub(1))
}

/// Bayer-pattern → ASCOM `BayerOffsetX/Y`.
fn bayer_offsets(mode: BayerMode) -> (u8, u8) {
    match mode {
        BayerMode::GBRG => (0, 1),
        BayerMode::GRBG => (1, 0),
        BayerMode::BGGR => (1, 1),
        BayerMode::RGGB => (0, 0),
    }
}

/// Convert a single-plane SDK frame into an ASCOM `ImageArray` with `[x][y]` axis
/// order (ASCOM stores width-major).
fn to_image_array(image: &ImageData) -> Result<ImageArray, String> {
    if image.channels != 1 {
        return Err(format!("unsupported channel count {}", image.channels));
    }
    let (w, h) = (image.width as usize, image.height as usize);
    match image.bits_per_pixel {
        8 => {
            let needed = w * h;
            if image.data.len() < needed {
                return Err("8-bit buffer too small for frame".to_string());
            }
            let arr = Array2::from_shape_vec((h, w), image.data[..needed].to_vec())
                .map_err(|e| e.to_string())?;
            Ok(ImageArray::from(arr.reversed_axes()))
        }
        16 => {
            let needed = w * h * 2;
            if image.data.len() < needed {
                return Err("16-bit buffer too small for frame".to_string());
            }
            let pixels: Vec<u16> = image.data[..needed]
                .chunks_exact(2)
                .map(|c| u16::from_ne_bytes([c[0], c[1]]))
                .collect();
            let arr = Array2::from_shape_vec((h, w), pixels).map_err(|e| e.to_string())?;
            Ok(ImageArray::from(arr.reversed_axes()))
        }
        other => Err(format!("unsupported bit depth {other}")),
    }
}

/// The detached capture task: runs the blocking single-frame SDK chain on
/// `spawn_blocking`, then stores the image (or records the failure as the
/// `Error` state) — unless a newer generation has superseded it.
async fn run_exposure(handle: Arc<dyn CameraHandle>, state: Arc<DeviceState>, generation: u64) {
    let blocking_handle = Arc::clone(&handle);
    let result = tokio::task::spawn_blocking(move || -> Result<ImageData, BackendError> {
        blocking_handle.start_single_frame_exposure()?;
        let size = blocking_handle.get_image_size()?;
        blocking_handle.get_single_frame(size)
    })
    .await;

    // Commit the outcome under the result lock so this "check generation +
    // record" is atomic against cancel_exposure's "bump generation + clear
    // image_ready" — an abort can never be overwritten by a just-completing
    // capture. (No await is held across the lock: the blocking await is above.)
    {
        let _guard = state.result_lock.lock();
        // Discard silently if a newer start / abort / disconnect superseded us.
        if state.exposure_generation.load(Ordering::Acquire) == generation {
            match result {
                Ok(Ok(image)) => match to_image_array(&image) {
                    Ok(array) => {
                        *state.last_image.lock() = Some(array);
                        *state.last_error.lock() = None;
                        state.image_ready.store(true, Ordering::Release);
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to transform captured image");
                        *state.last_image.lock() = None;
                        *state.last_error.lock() = Some(format!("image transform failed: {e}"));
                    }
                },
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
    // The CAS in start_exposure guarantees only one capture task runs at a time,
    // so this task owns clearing the flag once its SDK chain has fully drained —
    // even when superseded. Until it does, a new start_exposure is rejected.
    state.exposure_in_flight.store(false, Ordering::Release);
}

#[async_trait::async_trait]
impl Device for QhyCameraDevice {
    fn static_name(&self) -> &str {
        &self.name
    }

    fn unique_id(&self) -> &str {
        &self.unique_id
    }

    async fn connected(&self) -> ASCOMResult<bool> {
        self.handle.is_open().map_err(|_| ASCOMError::NOT_CONNECTED)
    }

    async fn set_connected(&self, connected: bool) -> ASCOMResult<()> {
        let current = self
            .handle
            .is_open()
            .map_err(|_| ASCOMError::NOT_CONNECTED)?;
        if current == connected {
            return Ok(());
        }
        if connected {
            self.connect()
        } else {
            self.disconnect().await
        }
    }

    async fn description(&self) -> ASCOMResult<String> {
        Ok(self.description.clone())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("rusty-photon qhy-camera".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }

    async fn supported_actions(&self) -> ASCOMResult<Vec<String>> {
        Ok(rusty_photon_driver::supported_actions(&self.config_ctx))
    }

    async fn action(&self, action: String, parameters: String) -> ASCOMResult<String> {
        rusty_photon_driver::dispatch::<QhyCameraDriver>(&self.config_ctx, action, parameters).await
    }
}

#[async_trait::async_trait]
impl Camera for QhyCameraDevice {
    // --- geometry ---------------------------------------------------------------

    async fn camera_x_size(&self) -> ASCOMResult<u32> {
        self.ensure_connected()?;
        (*self.state.ccd_info.lock())
            .map(|c| c.image_width)
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn camera_y_size(&self) -> ASCOMResult<u32> {
        self.ensure_connected()?;
        (*self.state.ccd_info.lock())
            .map(|c| c.image_height)
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn pixel_size_x(&self) -> ASCOMResult<f64> {
        self.ensure_connected()?;
        (*self.state.ccd_info.lock())
            .map(|c| c.pixel_width)
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn pixel_size_y(&self) -> ASCOMResult<f64> {
        self.ensure_connected()?;
        (*self.state.ccd_info.lock())
            .map(|c| c.pixel_height)
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn max_adu(&self) -> ASCOMResult<u32> {
        self.ensure_connected()?;
        let bits = match self.handle.get_parameter(Control::OutputDataActualBits) {
            Ok(b) => b as u32,
            Err(_) => (*self.state.ccd_info.lock())
                .map(|c| c.bits_per_pixel)
                .ok_or(ASCOMError::VALUE_NOT_SET)?,
        };
        Ok(max_adu_from_bits(bits))
    }

    async fn sensor_name(&self) -> ASCOMResult<String> {
        self.ensure_connected()?;
        self.unique_id
            .split('-')
            .next()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| ASCOMError::invalid_operation("could not derive sensor name"))
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
        let valid = self.state.valid_bins.lock().clone();
        if !valid.contains(&bin_x) {
            return Err(ASCOMError::invalid_value(format!(
                "bin {bin_x} is not a supported binning mode"
            )));
        }
        let old = self.state.bin.load(Ordering::Acquire);
        if old == bin_x {
            return Ok(());
        }
        self.handle
            .set_bin_mode(u32::from(bin_x), u32::from(bin_x))
            .map_err(|e| {
                ASCOMError::invalid_operation(format!("failed to set binning mode: {e}"))
            })?;
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
        self.state
            .valid_bins
            .lock()
            .iter()
            .copied()
            .max()
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
                *roi = Some(CCDChipArea {
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
                *roi = Some(CCDChipArea {
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
                *roi = Some(CCDChipArea { start_x, ..area });
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
                *roi = Some(CCDChipArea { start_y, ..area });
                Ok(())
            }
            None => Err(ASCOMError::INVALID_VALUE),
        }
    }

    // --- exposure range ---------------------------------------------------------

    async fn exposure_min(&self) -> ASCOMResult<Duration> {
        self.ensure_connected()?;
        let (min, _, _) =
            (*self.state.exposure_range_us.lock()).ok_or(ASCOMError::INVALID_VALUE)?;
        Ok(Duration::from_micros(min as u64))
    }

    async fn exposure_max(&self) -> ASCOMResult<Duration> {
        self.ensure_connected()?;
        let (_, max, _) =
            (*self.state.exposure_range_us.lock()).ok_or(ASCOMError::INVALID_VALUE)?;
        Ok(Duration::from_micros(max as u64))
    }

    async fn exposure_resolution(&self) -> ASCOMResult<Duration> {
        self.ensure_connected()?;
        let (_, _, step) =
            (*self.state.exposure_range_us.lock()).ok_or(ASCOMError::INVALID_VALUE)?;
        Ok(Duration::from_micros(step as u64))
    }

    // --- gain / offset ----------------------------------------------------------

    async fn gain(&self) -> ASCOMResult<i32> {
        self.ensure_connected()?;
        if self.handle.is_control_available(Control::Gain).is_none() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        self.handle
            .get_parameter(Control::Gain)
            .map(|g| g as i32)
            .map_err(|_| ASCOMError::INVALID_OPERATION)
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
        if self.handle.is_control_available(Control::Gain).is_none() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        let (min, max) = (*self.state.gain_min_max.lock()).ok_or_else(|| {
            ASCOMError::invalid_operation("gain control available but min/max not cached")
        })?;
        if gain < min as i32 || gain > max as i32 {
            return Err(ASCOMError::invalid_value(format!(
                "gain {gain} outside [{}, {}]",
                min as i32, max as i32
            )));
        }
        self.handle
            .set_parameter(Control::Gain, f64::from(gain))
            .map_err(|_| ASCOMError::INVALID_OPERATION)
    }

    async fn offset(&self) -> ASCOMResult<i32> {
        self.ensure_connected()?;
        if self.handle.is_control_available(Control::Offset).is_none() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        self.handle
            .get_parameter(Control::Offset)
            .map(|o| o as i32)
            .map_err(|_| ASCOMError::INVALID_OPERATION)
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
        if self.handle.is_control_available(Control::Offset).is_none() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        let (min, max) = (*self.state.offset_min_max.lock()).ok_or_else(|| {
            ASCOMError::invalid_operation("offset control available but min/max not cached")
        })?;
        if offset < min as i32 || offset > max as i32 {
            return Err(ASCOMError::invalid_value(format!(
                "offset {offset} outside [{}, {}]",
                min as i32, max as i32
            )));
        }
        self.handle
            .set_parameter(Control::Offset, f64::from(offset))
            .map_err(|_| ASCOMError::INVALID_OPERATION)
    }

    // --- readout modes ----------------------------------------------------------

    async fn readout_mode(&self) -> ASCOMResult<usize> {
        self.ensure_connected()?;
        self.handle
            .get_readout_mode()
            .map(|m| m as usize)
            .map_err(|_| ASCOMError::INVALID_OPERATION)
    }

    async fn readout_modes(&self) -> ASCOMResult<Vec<String>> {
        self.ensure_connected()?;
        let count = self
            .handle
            .get_number_of_readout_modes()
            .map_err(|_| ASCOMError::INVALID_OPERATION)?;
        let mut modes = Vec::with_capacity(count as usize);
        for index in 0..count {
            modes.push(
                self.handle
                    .get_readout_mode_name(index)
                    .map_err(|_| ASCOMError::INVALID_OPERATION)?,
            );
        }
        Ok(modes)
    }

    async fn set_readout_mode(&self, readout_mode: usize) -> ASCOMResult<()> {
        self.ensure_connected()?;
        let mode = readout_mode as u32;
        let count = self
            .handle
            .get_number_of_readout_modes()
            .map_err(|_| ASCOMError::INVALID_VALUE)?;
        if mode >= count {
            return Err(ASCOMError::invalid_value(format!(
                "readout mode {readout_mode} out of range (0..{count})"
            )));
        }
        let (width, height) = self
            .handle
            .get_readout_mode_resolution(mode)
            .map_err(|_| ASCOMError::INVALID_VALUE)?;
        self.handle.set_readout_mode(mode).map_err(|e| {
            ASCOMError::invalid_operation(format!("failed to set readout mode: {e}"))
        })?;
        if let Some(info) = self.state.ccd_info.lock().as_mut() {
            info.image_width = width;
            info.image_height = height;
        }
        Ok(())
    }

    // --- sensor type / bayer ----------------------------------------------------

    async fn sensor_type(&self) -> ASCOMResult<SensorType> {
        self.ensure_connected()?;
        if self
            .handle
            .is_control_available(Control::CamIsColor)
            .is_none()
        {
            return Ok(SensorType::Monochrome);
        }
        match self.handle.is_control_available(Control::CamColor) {
            Some(_) => Ok(SensorType::RGGB),
            None => Err(ASCOMError::INVALID_VALUE),
        }
    }

    async fn bayer_offset_x(&self) -> ASCOMResult<u8> {
        self.ensure_connected()?;
        if self
            .handle
            .is_control_available(Control::CamIsColor)
            .is_none()
        {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        let raw = self
            .handle
            .is_control_available(Control::CamColor)
            .ok_or(ASCOMError::INVALID_VALUE)?;
        let mode = BayerMode::try_from(raw).map_err(|_| ASCOMError::INVALID_VALUE)?;
        Ok(bayer_offsets(mode).0)
    }

    async fn bayer_offset_y(&self) -> ASCOMResult<u8> {
        self.ensure_connected()?;
        if self
            .handle
            .is_control_available(Control::CamIsColor)
            .is_none()
        {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        let raw = self
            .handle
            .is_control_available(Control::CamColor)
            .ok_or(ASCOMError::INVALID_VALUE)?;
        let mode = BayerMode::try_from(raw).map_err(|_| ASCOMError::INVALID_VALUE)?;
        Ok(bayer_offsets(mode).1)
    }

    // --- cooling ----------------------------------------------------------------

    async fn can_set_ccd_temperature(&self) -> ASCOMResult<bool> {
        Ok(self.handle.is_control_available(Control::Cooler).is_some())
    }

    async fn can_get_cooler_power(&self) -> ASCOMResult<bool> {
        self.can_set_ccd_temperature().await
    }

    async fn ccd_temperature(&self) -> ASCOMResult<f64> {
        self.ensure_connected()?;
        if self.handle.is_control_available(Control::Cooler).is_none() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        self.handle
            .get_parameter(Control::CurTemp)
            .map_err(|_| ASCOMError::INVALID_VALUE)
    }

    async fn set_ccd_temperature(&self) -> ASCOMResult<f64> {
        self.ensure_connected()?;
        if self.handle.is_control_available(Control::Cooler).is_none() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        if let Some(target) = *self.state.target_temperature.lock() {
            return Ok(target);
        }
        self.handle
            .get_parameter(Control::CurTemp)
            .map_err(|_| ASCOMError::INVALID_VALUE)
    }

    async fn set_set_ccd_temperature(&self, set_ccd_temperature: f64) -> ASCOMResult<()> {
        self.ensure_connected()?;
        if self.handle.is_control_available(Control::Cooler).is_none() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        if !(-273.15..=80.0).contains(&set_ccd_temperature) {
            return Err(ASCOMError::invalid_value(format!(
                "target temperature {set_ccd_temperature} outside [-273.15, 80]"
            )));
        }
        self.handle
            .set_parameter(Control::Cooler, set_ccd_temperature)
            .map_err(|_| ASCOMError::invalid_operation("failed to set target temperature"))?;
        *self.state.target_temperature.lock() = Some(set_ccd_temperature);
        Ok(())
    }

    async fn cooler_on(&self) -> ASCOMResult<bool> {
        self.ensure_connected()?;
        if self.handle.is_control_available(Control::Cooler).is_none() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        let pwm = self
            .handle
            .get_parameter(Control::CurPWM)
            .map_err(|_| ASCOMError::INVALID_VALUE)?;
        Ok(pwm > 0.0)
    }

    async fn set_cooler_on(&self, cooler_on: bool) -> ASCOMResult<()> {
        self.ensure_connected()?;
        if self.handle.is_control_available(Control::Cooler).is_none() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        // "On" engages a nominal 1% manual PWM (255/100); "off" is 0.
        let pwm = if cooler_on { 255.0 / 100.0 } else { 0.0 };
        self.handle
            .set_parameter(Control::ManualPWM, pwm)
            .map_err(|_| ASCOMError::invalid_operation("failed to set cooler state"))
    }

    async fn cooler_power(&self) -> ASCOMResult<f64> {
        self.ensure_connected()?;
        if self.handle.is_control_available(Control::Cooler).is_none() {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        let pwm = self
            .handle
            .get_parameter(Control::CurPWM)
            .map_err(|_| ASCOMError::INVALID_VALUE)?;
        Ok(pwm / 255.0 * 100.0)
    }

    // --- shutter / capability flags ---------------------------------------------

    async fn has_shutter(&self) -> ASCOMResult<bool> {
        Ok(self
            .handle
            .is_control_available(Control::CamMechanicalShutter)
            .is_some())
    }

    async fn can_abort_exposure(&self) -> ASCOMResult<bool> {
        Ok(true)
    }

    async fn can_stop_exposure(&self) -> ASCOMResult<bool> {
        Ok(false)
    }

    async fn can_pulse_guide(&self) -> ASCOMResult<bool> {
        Ok(false)
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
            // Idle: 100 once a frame is ready, 0 in the Error state (so a camera
            // reporting CameraState::Error never also reports 100% complete).
            return Ok(if self.state.last_error.lock().is_some() {
                0
            } else {
                100
            });
        }
        let expected = self.state.expected_duration_us.load(Ordering::Acquire);
        if expected == 0 {
            return Ok(0);
        }
        // `get_remaining_exposure_us` reads 0 both just-before the SDK exposure
        // actually begins and at completion; while still in flight, never report
        // 100 (that is reserved for the Idle/ready state above).
        let remaining = u64::from(
            self.handle
                .get_remaining_exposure_us()
                .map_err(|_| ASCOMError::invalid_operation("failed to read remaining exposure"))?,
        );
        let done = expected.saturating_sub(remaining);
        let pct = (done as f64 / expected as f64 * 100.0).clamp(0.0, 99.0);
        Ok(pct as u8)
    }

    async fn last_exposure_start_time(&self) -> ASCOMResult<SystemTime> {
        (*self.state.last_exposure_start_time.lock()).ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn last_exposure_duration(&self) -> ASCOMResult<Duration> {
        // Return the stored Duration as-is; round-tripping through secs_f64 only
        // introduces floating-point rounding (the value is already exact).
        (*self.state.last_exposure_duration.lock()).ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn image_array(&self) -> ASCOMResult<ImageArray> {
        self.ensure_connected()?;
        if let Some(msg) = self.state.last_error.lock().clone() {
            return Err(ASCOMError::new(UNSPECIFIED_ERROR, msg));
        }
        // ASCOM: `ImageArray` is only valid once `ImageReady` is true. Mirror the
        // `image_ready()` condition (a frame is committed and no exposure is in
        // flight) and error otherwise, so a client can never read a stale frame
        // from a previous exposure during a new capture or after an abort.
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
        if !light {
            // Dark/bias frames need the mechanical shutter closed. qhyccd-rs 0.1.9
            // exposes shutter *presence* (CamMechanicalShutter) but no open/close
            // actuation, so v0 cannot capture a true dark on any model — darks are
            // rejected. See docs/services/qhy-camera.md E4 / Future Work. The
            // simulated QHY178M-Simulated is shutterless.
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }

        let (min_us, max_us) = {
            let (min, max, _) =
                (*self.state.exposure_range_us.lock()).ok_or(ASCOMError::INVALID_VALUE)?;
            (min, max)
        };
        let exposure_us = (duration.as_secs_f64() * 1_000_000.0).round();
        if exposure_us < min_us || exposure_us > max_us {
            return Err(ASCOMError::invalid_value(format!(
                "exposure {exposure_us}us outside [{min_us}, {max_us}]"
            )));
        }

        let roi = self.validated_roi()?;

        // Claim the in-flight slot; lose the race → already exposing.
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

        if let Err(e) = self.handle.set_roi(roi) {
            self.state
                .exposure_in_flight
                .store(false, Ordering::Release);
            return Err(ASCOMError::invalid_value(format!("failed to set ROI: {e}")));
        }
        if let Err(e) = self.handle.set_parameter(Control::Exposure, exposure_us) {
            self.state
                .exposure_in_flight
                .store(false, Ordering::Release);
            return Err(ASCOMError::invalid_operation(format!(
                "failed to set exposure time: {e}"
            )));
        }

        self.state.image_ready.store(false, Ordering::Release);
        *self.state.last_error.lock() = None;
        *self.state.last_exposure_start_time.lock() = Some(SystemTime::now());
        *self.state.last_exposure_duration.lock() = Some(duration);
        self.state
            .expected_duration_us
            .store(exposure_us as u64, Ordering::Release);

        let handle = Arc::clone(&self.handle);
        let state = Arc::clone(&self.state);
        tokio::spawn(run_exposure(handle, state, generation));
        Ok(())
    }

    async fn abort_exposure(&self) -> ASCOMResult<()> {
        self.ensure_connected()?;
        self.cancel_exposure();
        Ok(())
    }

    async fn stop_exposure(&self) -> ASCOMResult<()> {
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

    fn area(start_x: u32, start_y: u32, width: u32, height: u32) -> CCDChipArea {
        CCDChipArea {
            start_x,
            start_y,
            width,
            height,
        }
    }

    fn connected_device(handle: MockCameraHandle) -> QhyCameraDevice {
        let device = QhyCameraDevice::new(Arc::new(handle), None);
        device.connect().unwrap();
        device
    }

    // --- pure helpers -----------------------------------------------------------

    #[test]
    fn max_adu_is_two_pow_bits_minus_one() {
        assert_eq!(max_adu_from_bits(16), 65535);
        assert_eq!(max_adu_from_bits(8), 255);
        // Saturating, not panicking, at 32 bits.
        assert_eq!(max_adu_from_bits(32), u32::MAX);
    }

    #[test]
    fn rescale_roi_scales_by_bin_ratio() {
        let scaled = rescale_roi(area(100, 200, 800, 600), 1, 2);
        assert_eq!(scaled, area(50, 100, 400, 300));
    }

    #[test]
    fn rescale_roi_clamps_tiny_dimensions_to_one() {
        // A 1×1 ROI binned 1→2 would truncate to 0×0 and make the next
        // StartExposure fail geometry validation; clamp keeps it at 1×1.
        let scaled = rescale_roi(area(0, 0, 1, 1), 1, 2);
        assert_eq!(scaled, area(0, 0, 1, 1));
        assert!(scaled.width >= 1 && scaled.height >= 1);
    }

    #[test]
    fn bayer_offset_mapping() {
        assert_eq!(bayer_offsets(BayerMode::RGGB), (0, 0));
        assert_eq!(bayer_offsets(BayerMode::GBRG), (0, 1));
        assert_eq!(bayer_offsets(BayerMode::GRBG), (1, 0));
        assert_eq!(bayer_offsets(BayerMode::BGGR), (1, 1));
    }

    #[test]
    fn check_geometry_rejects_zero_and_out_of_bounds() {
        // zero
        assert!(check_geometry(area(0, 0, 0, 100), 3072, 2048, 1).is_err());
        assert!(check_geometry(area(0, 0, 100, 0), 3072, 2048, 1).is_err());
        // out of bounds in x and y
        assert!(check_geometry(area(0, 0, 4000, 100), 3072, 2048, 1).is_err());
        assert!(check_geometry(area(0, 0, 100, 3000), 3072, 2048, 1).is_err());
        assert!(check_geometry(area(3000, 0, 100, 100), 3072, 2048, 1).is_err());
        assert!(check_geometry(area(0, 2000, 100, 100), 3072, 2048, 1).is_err());
        // valid full + sub frames
        assert!(check_geometry(area(0, 0, 3072, 2048), 3072, 2048, 1).is_ok());
        assert!(check_geometry(area(0, 0, 64, 48), 3072, 2048, 1).is_ok());
        // binned bounds shrink
        assert!(check_geometry(area(0, 0, 1536, 1024), 3072, 2048, 2).is_ok());
        assert!(check_geometry(area(0, 0, 2000, 100), 3072, 2048, 2).is_err());
    }

    #[test]
    fn to_image_array_16bit_has_width_major_axes() {
        let image = ImageData {
            data: vec![0u8; 64 * 48 * 2],
            width: 64,
            height: 48,
            bits_per_pixel: 16,
            channels: 1,
        };
        let array = to_image_array(&image).unwrap();
        // ASCOM [x][y]: first axis = width.
        assert_eq!(array.dim().0, 64);
        assert_eq!(array.dim().1, 48);
    }

    #[test]
    fn to_image_array_rejects_multichannel() {
        let image = ImageData {
            data: vec![0u8; 64 * 48 * 4],
            width: 64,
            height: 48,
            bits_per_pixel: 16,
            channels: 4,
        };
        assert!(to_image_array(&image).is_err());
    }

    #[test]
    fn to_image_array_8bit_has_width_major_axes() {
        let image = ImageData {
            data: vec![0u8; 64 * 48],
            width: 64,
            height: 48,
            bits_per_pixel: 8,
            channels: 1,
        };
        let array = to_image_array(&image).unwrap();
        assert_eq!(array.dim().0, 64);
        assert_eq!(array.dim().1, 48);
    }

    #[test]
    fn to_image_array_rejects_undersized_buffers() {
        for bits in [8, 16] {
            let image = ImageData {
                data: vec![0u8; 10], // far too small for a 64×48 frame
                width: 64,
                height: 48,
                bits_per_pixel: bits,
                channels: 1,
            };
            assert!(
                to_image_array(&image).is_err(),
                "{bits}-bit undersized buffer must be rejected"
            );
        }
    }

    // --- device behaviour via the mock seam -------------------------------------

    #[tokio::test]
    async fn connect_caches_geometry_and_limits() {
        let device = connected_device(MockCameraHandle::default());
        assert_eq!(device.camera_x_size().await.unwrap(), 3072);
        assert_eq!(device.camera_y_size().await.unwrap(), 2048);
        assert_eq!(device.max_adu().await.unwrap(), 65535);
        assert_eq!(device.max_bin_x().await.unwrap(), 2);
        assert!(!device.can_asymmetric_bin().await.unwrap());
        assert_eq!(device.sensor_type().await.unwrap(), SensorType::Monochrome);
        assert!(!device.has_shutter().await.unwrap());
    }

    #[tokio::test]
    async fn failed_connect_leaves_camera_closed() {
        // C2: a handshake failure after open() must not leave the camera open.
        let handle = MockCameraHandle::default();
        handle.fail_handshake.store(true, Ordering::SeqCst);
        let device = QhyCameraDevice::new(Arc::new(handle), None);
        let err = device.set_connected(true).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
        assert!(!device.connected().await.unwrap());
    }

    #[tokio::test]
    async fn reconnect_clears_error_state() {
        // E9 puts the camera in Error; a disconnect + reconnect must clear it (C3).
        let mock = Arc::new(MockCameraHandle::default());
        mock.fail_single_frame.store(true, Ordering::SeqCst);
        let device = QhyCameraDevice::new(mock.clone(), None);
        device.set_connected(true).await.unwrap();
        device.set_num_x(64).await.unwrap();
        device.set_num_y(48).await.unwrap();
        device
            .start_exposure(Duration::from_millis(10), true)
            .await
            .unwrap();
        for _ in 0..200 {
            if device.camera_state().await.unwrap() == CameraState::Error {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(device.camera_state().await.unwrap(), CameraState::Error);

        device.set_connected(false).await.unwrap();
        mock.fail_single_frame.store(false, Ordering::SeqCst);
        device.set_connected(true).await.unwrap();
        assert_eq!(device.camera_state().await.unwrap(), CameraState::Idle);
        assert!(!device.image_ready().await.unwrap());
    }

    #[tokio::test]
    async fn gain_out_of_range_is_rejected() {
        let device = connected_device(MockCameraHandle::default());
        let max = device.gain_max().await.unwrap();
        device.set_gain(max).await.unwrap();
        assert_eq!(device.gain().await.unwrap(), max);
        let err = device.set_gain(max + 1).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
    }

    #[tokio::test]
    async fn gain_not_implemented_without_control() {
        let device = connected_device(MockCameraHandle::default().without_control(Control::Gain));
        let err = device.gain().await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn color_sensor_reports_rggb_and_bayer_offsets() {
        // A colour model the mono simulation backend cannot exercise via BDD.
        let handle = MockCameraHandle::default()
            .with_control(Control::CamIsColor, 1)
            .with_control(Control::CamColor, BayerMode::RGGB as u32);
        let device = connected_device(handle);
        assert_eq!(device.sensor_type().await.unwrap(), SensorType::RGGB);
        assert_eq!(device.bayer_offset_x().await.unwrap(), 0);
        assert_eq!(device.bayer_offset_y().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn shutter_model_reports_has_shutter() {
        let device = connected_device(
            MockCameraHandle::default().with_control(Control::CamMechanicalShutter, 1),
        );
        assert!(device.has_shutter().await.unwrap());
    }

    #[tokio::test]
    async fn cooling_turns_on_and_reports_power() {
        let device = connected_device(MockCameraHandle::default());
        assert!(device.can_set_ccd_temperature().await.unwrap());
        device.set_set_ccd_temperature(-10.0).await.unwrap();
        assert_eq!(device.set_ccd_temperature().await.unwrap(), -10.0);
        device.set_cooler_on(true).await.unwrap();
        assert!(device.cooler_on().await.unwrap());
        let power = device.cooler_power().await.unwrap();
        assert!((0.0..=100.0).contains(&power), "{power}");
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
    async fn gain_min_max_reflect_cached_range() {
        let device = connected_device(MockCameraHandle::default());
        assert_eq!(device.gain_min().await.unwrap(), 0);
        assert_eq!(device.gain_max().await.unwrap(), 100);
    }

    #[tokio::test]
    async fn offset_round_trips_and_rejects_out_of_range() {
        let device = connected_device(MockCameraHandle::default());
        assert_eq!(device.offset_min().await.unwrap(), 0);
        let max = device.offset_max().await.unwrap();
        assert_eq!(max, 255);
        device.set_offset(max).await.unwrap();
        assert_eq!(device.offset().await.unwrap(), max);
        assert_eq!(
            device.set_offset(max + 1).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_VALUE
        );
    }

    #[tokio::test]
    async fn offset_not_implemented_without_control() {
        let device = connected_device(MockCameraHandle::default().without_control(Control::Offset));
        assert_eq!(
            device.offset().await.unwrap_err().code,
            ASCOMErrorCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            device.set_offset(5).await.unwrap_err().code,
            ASCOMErrorCode::NOT_IMPLEMENTED
        );
    }

    #[tokio::test]
    async fn exposure_limits_reflect_cached_range() {
        let device = connected_device(MockCameraHandle::default());
        assert_eq!(
            device.exposure_min().await.unwrap(),
            Duration::from_micros(1)
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
    async fn readout_modes_list_select_and_reject_out_of_range() {
        let device = connected_device(MockCameraHandle::default());
        assert_eq!(
            device.readout_modes().await.unwrap(),
            vec!["Standard".to_string()]
        );
        assert_eq!(device.readout_mode().await.unwrap(), 0);
        device.set_readout_mode(0).await.unwrap();
        // Only one mode (0); selecting 1 is out of range.
        assert_eq!(
            device.set_readout_mode(1).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_VALUE
        );
    }

    #[tokio::test]
    async fn sensor_name_is_the_serial_prefix() {
        // unique_id "SIM-QHY178M" → "SIM".
        let device = connected_device(MockCameraHandle::default());
        assert_eq!(device.sensor_name().await.unwrap(), "SIM");
    }

    #[tokio::test]
    async fn device_metadata_reports_expected_strings() {
        let device = connected_device(MockCameraHandle::default());
        assert_eq!(
            device.driver_info().await.unwrap(),
            "rusty-photon qhy-camera"
        );
        assert_eq!(
            device.driver_version().await.unwrap(),
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(device.description().await.unwrap(), "QHYCCD camera");
        // Delegates to rusty_photon_driver; exercise the path (always Ok).
        device.supported_actions().await.unwrap();
    }

    #[tokio::test]
    async fn capability_flags_are_fixed() {
        let device = connected_device(MockCameraHandle::default());
        assert!(device.can_abort_exposure().await.unwrap());
        assert!(!device.can_stop_exposure().await.unwrap());
        assert!(!device.can_pulse_guide().await.unwrap());
    }

    #[tokio::test]
    async fn cooling_capabilities_and_temperature_readback() {
        let device = connected_device(MockCameraHandle::default());
        assert!(device.can_get_cooler_power().await.unwrap());
        // CurTemp default is 20.0 °C on the simulated mono model.
        assert_eq!(device.ccd_temperature().await.unwrap(), 20.0);
    }

    #[tokio::test]
    async fn cooling_is_not_implemented_without_cooler_control() {
        let device = connected_device(MockCameraHandle::default().without_control(Control::Cooler));
        assert!(!device.can_set_ccd_temperature().await.unwrap());
        assert!(!device.can_get_cooler_power().await.unwrap());
        for code in [
            device.ccd_temperature().await.unwrap_err().code,
            device.cooler_on().await.unwrap_err().code,
            device.set_cooler_on(true).await.unwrap_err().code,
            device.cooler_power().await.unwrap_err().code,
            device
                .set_set_ccd_temperature(-10.0)
                .await
                .unwrap_err()
                .code,
        ] {
            assert_eq!(code, ASCOMErrorCode::NOT_IMPLEMENTED);
        }
    }

    #[tokio::test]
    async fn geometry_roi_and_bin_getters_report_cached_values() {
        let device = connected_device(MockCameraHandle::default());
        assert_eq!(device.pixel_size_x().await.unwrap(), 2.4);
        assert_eq!(device.pixel_size_y().await.unwrap(), 2.4);
        // The intended ROI defaults to the full effective area at connect.
        assert_eq!(device.num_x().await.unwrap(), 3072);
        assert_eq!(device.num_y().await.unwrap(), 2048);
        assert_eq!(device.start_x().await.unwrap(), 0);
        assert_eq!(device.start_y().await.unwrap(), 0);
        assert_eq!(device.bin_x().await.unwrap(), 1);
        assert_eq!(device.bin_y().await.unwrap(), 1);
        assert_eq!(device.max_bin_y().await.unwrap(), 2);
        assert!(!device.can_asymmetric_bin().await.unwrap());
    }

    #[tokio::test]
    async fn roi_setters_round_trip() {
        let device = connected_device(MockCameraHandle::default());
        device.set_num_x(64).await.unwrap();
        device.set_num_y(48).await.unwrap();
        device.set_start_x(10).await.unwrap();
        device.set_start_y(20).await.unwrap();
        assert_eq!(device.num_x().await.unwrap(), 64);
        assert_eq!(device.num_y().await.unwrap(), 48);
        assert_eq!(device.start_x().await.unwrap(), 10);
        assert_eq!(device.start_y().await.unwrap(), 20);
    }

    #[tokio::test]
    async fn set_bin_y_mirrors_bin_x() {
        let device = connected_device(MockCameraHandle::default());
        device.set_bin_y(2).await.unwrap();
        assert_eq!(device.bin_x().await.unwrap(), 2);
        assert_eq!(device.bin_y().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn last_exposure_metadata_is_unset_before_first_exposure() {
        let device = connected_device(MockCameraHandle::default());
        assert_eq!(
            device.last_exposure_start_time().await.unwrap_err().code,
            ASCOMErrorCode::VALUE_NOT_SET
        );
        assert_eq!(
            device.last_exposure_duration().await.unwrap_err().code,
            ASCOMErrorCode::VALUE_NOT_SET
        );
    }

    #[tokio::test]
    async fn set_bin_surfaces_sdk_failure_as_invalid_operation() {
        let mock = Arc::new(MockCameraHandle::default());
        let device = QhyCameraDevice::new(mock.clone(), None);
        device.set_connected(true).await.unwrap();
        mock.fail_set_controls.store(true, Ordering::SeqCst);
        // bin 2 is valid and differs from the current 1, so it reaches the SDK.
        assert_eq!(
            device.set_bin_x(2).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_OPERATION
        );
    }

    #[tokio::test]
    async fn set_readout_mode_surfaces_sdk_failure_as_invalid_operation() {
        let mock = Arc::new(MockCameraHandle::default());
        let device = QhyCameraDevice::new(mock.clone(), None);
        device.set_connected(true).await.unwrap();
        mock.fail_set_controls.store(true, Ordering::SeqCst);
        assert_eq!(
            device.set_readout_mode(0).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_OPERATION
        );
    }

    #[tokio::test]
    async fn handshake_rejects_camera_without_single_frame_mode() {
        // open() succeeds, but a camera that doesn't advertise single-frame mode
        // can't be driven — connect must fail and leave it disconnected.
        let handle = MockCameraHandle::default().without_control(Control::CamSingleFrameMode);
        let device = QhyCameraDevice::new(Arc::new(handle), None);
        assert_eq!(
            device.set_connected(true).await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert!(!device.connected().await.unwrap());
    }

    #[tokio::test]
    async fn bin_change_rescales_roi_and_rejects_unsupported() {
        let device = connected_device(MockCameraHandle::default());
        device.set_num_x(3072).await.unwrap();
        device.set_num_y(2048).await.unwrap();
        device.set_bin_x(2).await.unwrap();
        assert_eq!(device.bin_x().await.unwrap(), 2);
        assert_eq!(device.num_x().await.unwrap(), 1536);
        assert_eq!(device.num_y().await.unwrap(), 1024);
        assert_eq!(
            device.set_bin_x(99).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_VALUE
        );
    }

    #[tokio::test]
    async fn disconnected_start_exposure_is_not_connected() {
        let device = QhyCameraDevice::new(Arc::new(MockCameraHandle::default()), None);
        let err = device
            .start_exposure(Duration::from_millis(10), true)
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn dark_frame_is_not_implemented() {
        let device = connected_device(MockCameraHandle::default());
        let err = device
            .start_exposure(Duration::from_millis(10), false)
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::NOT_IMPLEMENTED);
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
        // Wait for the detached capture task.
        for _ in 0..200 {
            if device.image_ready().await.unwrap() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(device.image_ready().await.unwrap());
        assert_eq!(device.camera_state().await.unwrap(), CameraState::Idle);
        assert_eq!(device.percent_completed().await.unwrap(), 100);
        let image = device.image_array().await.unwrap();
        assert_eq!(image.dim().0, 64);
        assert_eq!(image.dim().1, 48);
    }

    #[tokio::test]
    async fn mid_exposure_error_transitions_to_error_state() {
        let handle = MockCameraHandle::default();
        handle.fail_single_frame.store(true, Ordering::SeqCst);
        let device = connected_device(handle);
        device
            .start_exposure(Duration::from_millis(10), true)
            .await
            .unwrap();
        for _ in 0..200 {
            if device.camera_state().await.unwrap() == CameraState::Error {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(device.camera_state().await.unwrap(), CameraState::Error);
        assert!(!device.image_ready().await.unwrap());
        assert_eq!(
            device.image_array().await.unwrap_err().code,
            UNSPECIFIED_ERROR
        );
    }

    #[tokio::test]
    async fn second_exposure_while_in_flight_is_rejected() {
        let handle = MockCameraHandle::default();
        handle.set_single_frame_delay(Duration::from_secs(5));
        let device = connected_device(handle);
        device
            .start_exposure(Duration::from_secs(5), true)
            .await
            .unwrap();
        // Give the background task a moment to enter the blocking capture.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(device.camera_state().await.unwrap(), CameraState::Exposing);
        let err = device
            .start_exposure(Duration::from_millis(10), true)
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
        device.abort_exposure().await.unwrap();
    }

    #[tokio::test]
    async fn stop_exposure_is_not_implemented() {
        let device = connected_device(MockCameraHandle::default());
        assert!(!device.can_stop_exposure().await.unwrap());
        assert_eq!(
            device.stop_exposure().await.unwrap_err().code,
            ASCOMErrorCode::NOT_IMPLEMENTED
        );
    }

    #[tokio::test]
    async fn image_array_errors_after_abort_instead_of_returning_stale_frame() {
        let handle = MockCameraHandle::default();
        handle.set_single_frame_delay(Duration::from_secs(5));
        let device = connected_device(handle);
        device
            .start_exposure(Duration::from_millis(10), true)
            .await
            .unwrap();
        device.abort_exposure().await.unwrap();
        // No fresh frame is ready after an abort, so ImageArray must error
        // rather than hand back a stale image from a previous exposure.
        assert!(!device.image_ready().await.unwrap());
        assert_eq!(
            device.image_array().await.unwrap_err().code,
            ASCOMErrorCode::INVALID_OPERATION
        );
    }
}
