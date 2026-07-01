//! The SDK seam: a thin trait over the blocking `touptek-rs` [`Camera`] surface
//! the ASCOM device drives, plus a production wrapper and a test mock.
//!
//! Why a seam: it (1) collapses [`touptek_rs::Error`] into a typed
//! [`BackendError`] at one boundary, (2) lets the ASCOM device hold an
//! `Arc<dyn CameraHandle>` so unit tests can substitute a mock that forces paths
//! the `touptek-rs` simulation cannot — an SDK open failure (C2), a mid-exposure
//! SDK error (E9), a model without gain/offset (GO1), without ST4 (PG2), or
//! without a cooler (K1) — without hardware, and (3) keeps the open/close
//! lifecycle in one place. `touptek-rs`'s [`Camera`](touptek_rs::Camera) is RAII
//! (open = [`touptek_rs::Sdk::open`], close = drop) and `Send + !Sync`, so the
//! production handle keeps it behind a `parking_lot::Mutex` and re-opens on
//! connect from the cached enumeration `index`.
//!
//! ## The trigger-mode capture
//!
//! Discrete ASCOM exposures use trigger mode plus the callback→pull bridge:
//! [`capture`](CameraHandle::capture) configures the camera, enables trigger
//! mode, starts the pull session, fires one trigger, integrates against a
//! **real-clock deadline** (so an in-flight exposure is observable — the
//! `touptek-rs` simulation queues the frame-ready event immediately), then waits
//! for the event and pulls the frame. The integration releases the camera lock so
//! a concurrent `is_open()` / disconnect is not blocked for the whole exposure;
//! an [`abort`](CameraHandle::request_abort) (or a disconnect that closes the
//! camera mid-integration) discards the frame.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use touptek_rs::{CameraInfo, Event, GuideDirection};

/// A `touptek-rs` SDK call failed. Carries the underlying message; the ASCOM
/// device decides the `ASCOMError` per call site (the SDK error kind does not map
/// 1:1 to an ASCOM code).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct BackendError(pub String);

/// Collapse a [`touptek_rs::Error`] into the typed seam error. The seam keeps only
/// the message string (each call site picks the right `ASCOMError` code); this
/// `From` impl lets `?` convert SDK errors automatically.
impl From<touptek_rs::Error> for BackendError {
    fn from(err: touptek_rs::Error) -> Self {
        Self(err.to_string())
    }
}

impl BackendError {
    fn closed() -> Self {
        Self("camera not open".to_string())
    }
}

pub type BackendResult<T> = std::result::Result<T, BackendError>;

/// The control ranges read once at the connect handshake and cached by the device.
#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    /// `(min, max)` exposure microseconds (`Toupcam_get_ExpTimeRange`).
    pub exposure_range_us: (u32, u32),
    /// `(min, max)` analog-gain percent (`Toupcam_get_ExpoAGainRange`), or `None`
    /// when the model has no gain control (GO1).
    pub gain_range: Option<(u16, u16)>,
    /// Whether the model exposes the black-level control (`OPTION_BLACKLEVEL`).
    pub offset_supported: bool,
    /// Whether the model exposes a sensor-temperature read (`get_Temperature`),
    /// decoupled from cooling (K2).
    pub temperature_available: bool,
}

/// The ROI + exposure parameters for a single capture, validated by the device.
#[derive(Debug, Clone, Copy)]
pub struct CaptureRequest {
    /// Post-binning frame width (`NumX`).
    pub width: u32,
    /// Post-binning frame height (`NumY`).
    pub height: u32,
    /// Symmetric digital-binning factor.
    pub bin: u32,
    /// Post-binning ROI start X (`StartX`).
    pub start_x: u32,
    /// Post-binning ROI start Y (`StartY`).
    pub start_y: u32,
    /// Exposure time in microseconds (`Toupcam_put_ExpoTime`).
    pub exposure_us: u32,
    /// RAW bits per pixel to pull (16 for the `PIXELFORMAT_RAW16` path).
    pub bit_depth: u32,
    /// Wall-clock integration time the capture honours so an in-flight exposure is
    /// observable (the `touptek-rs` simulation queues the event immediately).
    pub duration: Duration,
    /// Request a dark frame (no-op on shutterless ToupTek sensors).
    pub is_dark: bool,
}

/// Real-clock budget for the post-integration frame-ready wait. The integration
/// already sleeps for the requested duration, so the frame-ready event is due
/// shortly after; this only bounds the wait so a missing event cannot hang the
/// capture thread.
const READOUT_TIMEOUT: Duration = Duration::from_secs(5);

/// The blocking camera operations the ASCOM `Camera` device drives. Every method
/// is synchronous (the SDK is blocking C FFI); the device offloads the long
/// [`capture`](CameraHandle::capture) onto `spawn_blocking`.
pub trait CameraHandle: std::fmt::Debug + Send + Sync {
    /// The stable ASCOM `UniqueID` (id-derived; read once at enumeration).
    fn unique_id(&self) -> String;

    /// The camera's enumeration [`CameraInfo`] (cached; no open required).
    fn info(&self) -> CameraInfo;

    fn is_open(&self) -> bool;
    fn open(&self) -> BackendResult<()>;
    fn close(&self) -> BackendResult<()>;

    /// Read the camera's control ranges (`get_ExpTimeRange` / `get_ExpoAGainRange`
    /// / black-level + temperature availability), cached at the open handshake.
    fn capabilities(&self) -> BackendResult<Capabilities>;

    /// Current analog gain in percent (`Toupcam_get_ExpoAGain`).
    fn gain(&self) -> BackendResult<u16>;
    /// Set the analog gain in percent (`Toupcam_put_ExpoAGain`).
    fn set_gain(&self, percent: u16) -> BackendResult<()>;
    /// Current black level (`OPTION_BLACKLEVEL`) — the ASCOM `Offset`.
    fn offset(&self) -> BackendResult<i32>;
    /// Set the black level (`OPTION_BLACKLEVEL`).
    fn set_offset(&self, value: i32) -> BackendResult<()>;

    /// Sensor temperature in °C (decodes the 0.1 °C `get_Temperature` units).
    fn temperature_celsius(&self) -> BackendResult<f64>;
    /// Turn the thermo-electric cooler on/off (`OPTION_TEC`).
    fn set_cooler(&self, on: bool) -> BackendResult<()>;
    /// Whether the cooler is on (`OPTION_TEC`).
    fn cooler_on(&self) -> BackendResult<bool>;
    /// Cooler power as a 0–100 % of max TEC voltage (`OPTION_TEC_VOLTAGE`).
    fn cooler_power_percent(&self) -> BackendResult<u32>;
    /// Set the cooler target temperature in 0.1 °C units (`OPTION_TECTARGET`).
    fn set_target_temperature_tenths(&self, tenths: i16) -> BackendResult<()>;
    /// The current cooler target temperature in 0.1 °C units (`OPTION_TECTARGET`).
    /// Has a power-on default on real hardware, so the `SetCCDTemperature` getter
    /// can report a value before any setpoint is written.
    fn target_temperature_tenths(&self) -> BackendResult<i16>;

    /// Run a single-frame trigger-mode capture: configure, trigger, integrate
    /// (honouring an abort signal), wait for the frame-ready event, pull. Returns
    /// `Ok(Some(frame))` for a completed exposure, `Ok(None)` for an aborted one
    /// (frame discarded), or `Err` on an SDK error.
    fn capture(&self, request: CaptureRequest) -> BackendResult<Option<Vec<u8>>>;

    /// Signal an in-flight [`capture`](Self::capture) to abort (discard the
    /// frame). Trigger mode has no data-preserving stop (`CanStopExposure = false`).
    fn request_abort(&self);

    /// Issue an ST4 guide pulse (`Toupcam_ST4PlusGuide`); the SDK times it.
    fn pulse_guide(&self, direction: GuideDirection, duration_ms: u32) -> BackendResult<()>;
}

// --- production wrapper over touptek-rs ------------------------------------------

/// Production [`CameraHandle`] over a real (or `touptek-rs`-simulated) camera.
///
/// Holds the [`touptek_rs::Sdk`] (a ZST) and the enumeration `index` so it can
/// re-open the RAII [`touptek_rs::Camera`] on connect; the open handle lives
/// behind a `Mutex<Option<…>>` because `Camera` is `Send + !Sync`.
#[derive(derive_more::Debug)]
pub struct TouptekCameraHandle {
    #[debug(skip)]
    sdk: touptek_rs::Sdk,
    index: usize,
    info: CameraInfo,
    unique_id: String,
    // `touptek_rs::Camera` is not `Debug`, so the field is skipped.
    #[debug(skip)]
    camera: Mutex<Option<touptek_rs::Camera>>,
    /// Abort signal read by an in-flight [`capture`](Self::capture). A plain atomic
    /// (not inside the `Mutex`) so an abort can signal while the capture holds the
    /// camera lock.
    abort: AtomicBool,
}

impl TouptekCameraHandle {
    /// Build a handle for the camera at enumeration `index`, with its cached
    /// [`CameraInfo`] and the id-derived `unique_id` read at enumeration.
    pub fn new(sdk: touptek_rs::Sdk, index: usize, info: CameraInfo, unique_id: String) -> Self {
        Self {
            sdk,
            index,
            info,
            unique_id,
            camera: Mutex::new(None),
            abort: AtomicBool::new(false),
        }
    }

    /// Whether the model advertises a sensor-temperature read (cooled cameras and
    /// models with the explicit `GET_TEMPERATURE` flag).
    fn temperature_available(&self) -> bool {
        self.info.has_tec()
            || self.info.flag & u64::from(touptek_rs::sys::TOUPCAM_FLAG_GETTEMPERATURE) != 0
    }
}

impl CameraHandle for TouptekCameraHandle {
    fn unique_id(&self) -> String {
        self.unique_id.clone()
    }

    fn info(&self) -> CameraInfo {
        self.info.clone()
    }

    fn is_open(&self) -> bool {
        self.camera.lock().is_some()
    }

    fn open(&self) -> BackendResult<()> {
        let mut guard = self.camera.lock();
        if guard.is_none() {
            let camera = self.sdk.open(self.index)?;
            // C1: select the 16-bit RAW path at connect (`PIXELFORMAT_RAW16` via
            // `OPTION_RAW=1` + `OPTION_BITDEPTH=1`) so `pull_image(bits = 16)` reads
            // Bayer/mono RAW rather than the camera's default demosaiced output.
            // Set from the owner thread before any pull session (the SDK only
            // forbids re-entry from the callback thread). The colour-model pixel
            // format is refined against real hardware in Phase F.
            camera.set_option(touptek_rs::sys::TOUPCAM_OPTION_RAW, 1)?;
            camera.set_option(touptek_rs::sys::TOUPCAM_OPTION_BITDEPTH, 1)?;
            *guard = Some(camera);
        }
        Ok(())
    }

    fn close(&self) -> BackendResult<()> {
        // Dropping the `Camera` calls `Toupcam_Stop` + `Toupcam_Close`.
        *self.camera.lock() = None;
        Ok(())
    }

    fn capabilities(&self) -> BackendResult<Capabilities> {
        let guard = self.camera.lock();
        let camera = guard.as_ref().ok_or_else(BackendError::closed)?;
        // `gain_range` is `Option` for a reason: a model with no analog-gain
        // control answers `Toupcam_get_ExpoAGainRange` with `E_NOTIMPL`. Map only
        // that to `None` so the driver exposes Gain* as NOT_IMPLEMENTED (GO1)
        // instead of failing the whole connect handshake (which would surface as
        // NOT_CONNECTED). Any other SDK error is a genuine fault and propagates.
        // (Reachable only on such hardware — the sim's `gain_range` always
        // succeeds; the GO1 unit test drives the mock backend's own path.)
        let gain_range = match camera.gain_range() {
            Ok(range) => Some(range),
            Err(touptek_rs::Error::Sdk(touptek_rs::SdkError::NotImplemented)) => {
                tracing::debug!(
                    "gain range not implemented on this model; reporting Gain as NOT_IMPLEMENTED"
                );
                None
            }
            Err(e) => return Err(e.into()),
        };
        Ok(Capabilities {
            exposure_range_us: camera.exposure_range_us()?,
            gain_range,
            // Black level (`Offset`) is model-specific: gate on the SDK's
            // `FLAG_BLACKLEVEL` so a model without it reports `NOT_IMPLEMENTED`
            // (GO1), the same flag INDI gates on.
            offset_supported: self.info.flag & u64::from(touptek_rs::sys::TOUPCAM_FLAG_BLACKLEVEL)
                != 0,
            temperature_available: self.temperature_available(),
        })
    }

    fn gain(&self) -> BackendResult<u16> {
        let guard = self.camera.lock();
        let camera = guard.as_ref().ok_or_else(BackendError::closed)?;
        Ok(camera.gain_percent()?)
    }

    fn set_gain(&self, percent: u16) -> BackendResult<()> {
        let guard = self.camera.lock();
        let camera = guard.as_ref().ok_or_else(BackendError::closed)?;
        Ok(camera.set_gain_percent(percent)?)
    }

    fn offset(&self) -> BackendResult<i32> {
        let guard = self.camera.lock();
        let camera = guard.as_ref().ok_or_else(BackendError::closed)?;
        Ok(camera.black_level()?)
    }

    fn set_offset(&self, value: i32) -> BackendResult<()> {
        let guard = self.camera.lock();
        let camera = guard.as_ref().ok_or_else(BackendError::closed)?;
        Ok(camera.set_black_level(value)?)
    }

    fn temperature_celsius(&self) -> BackendResult<f64> {
        let guard = self.camera.lock();
        let camera = guard.as_ref().ok_or_else(BackendError::closed)?;
        Ok(f64::from(camera.temperature_tenths_c()?) / 10.0)
    }

    fn set_cooler(&self, on: bool) -> BackendResult<()> {
        let guard = self.camera.lock();
        let camera = guard.as_ref().ok_or_else(BackendError::closed)?;
        Ok(camera.set_cooler(on)?)
    }

    fn cooler_on(&self) -> BackendResult<bool> {
        let guard = self.camera.lock();
        let camera = guard.as_ref().ok_or_else(BackendError::closed)?;
        Ok(camera.cooler_on()?)
    }

    fn cooler_power_percent(&self) -> BackendResult<u32> {
        let guard = self.camera.lock();
        let camera = guard.as_ref().ok_or_else(BackendError::closed)?;
        Ok(camera.cooler_power_percent()?)
    }

    fn set_target_temperature_tenths(&self, tenths: i16) -> BackendResult<()> {
        let guard = self.camera.lock();
        let camera = guard.as_ref().ok_or_else(BackendError::closed)?;
        Ok(camera.set_target_temperature_tenths(tenths)?)
    }

    fn target_temperature_tenths(&self) -> BackendResult<i16> {
        let guard = self.camera.lock();
        let camera = guard.as_ref().ok_or_else(BackendError::closed)?;
        Ok(camera.target_temperature_tenths()?)
    }

    fn capture(&self, request: CaptureRequest) -> BackendResult<Option<Vec<u8>>> {
        // Reset the abort signal for this capture. An abort racing in just before
        // this reset is benign: the device bumps the exposure generation on abort,
        // so a swallowed late abort only lets this (already-discarded) frame
        // complete — no frame is ever committed after an abort.
        self.abort.store(false, Ordering::SeqCst);

        // Configure and start the trigger-mode pull session under the lock, then
        // RELEASE it for the integration so a concurrent read / disconnect is not
        // blocked for the whole exposure.
        {
            let mut guard = self.camera.lock();
            let camera = guard.as_mut().ok_or_else(BackendError::closed)?;
            // Digital binning + ROI. The exact binned-vs-sensor coordinate mapping
            // for `put_Roi` against `OPTION_BINNING` is validated on real hardware
            // in Phase F; the simulation backend ignores both.
            camera.set_option(touptek_rs::sys::TOUPCAM_OPTION_BINNING, request.bin as i32)?;
            camera.set_roi(
                request.start_x,
                request.start_y,
                request.width,
                request.height,
            )?;
            camera.set_exposure_time_us(request.exposure_us)?;
            camera.enable_trigger_mode()?;
            camera.start_pull_mode()?;
            camera.trigger_single()?;
        }

        // Integrate against a real-clock DEADLINE (not accumulated intended naps),
        // checking the abort signal so a disconnect/abort returns promptly. The
        // deadline discipline keeps the integration bounded to the requested
        // duration under blocking-pool oversubscription (the zwo-camera macOS
        // ConformU lesson).
        let deadline = Instant::now() + request.duration;
        let step = Duration::from_millis(20);
        loop {
            if self.abort.load(Ordering::SeqCst) {
                if let Some(camera) = self.camera.lock().as_mut() {
                    let _ = camera.stop();
                }
                return Ok(None);
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            std::thread::sleep(step.min(deadline - now));
        }

        // Wait for the frame-ready event and pull, under the lock. If the camera
        // was closed mid-integration (a disconnect), treat the capture as aborted.
        let mut guard = self.camera.lock();
        let Some(camera) = guard.as_mut() else {
            return Ok(None);
        };
        if self.abort.load(Ordering::SeqCst) {
            let _ = camera.stop();
            return Ok(None);
        }
        // Wait for the frame-ready event and pull, then ALWAYS stop the pull
        // session before returning — even on a timeout / error event / failed
        // pull. Leaving a session live would make the next capture re-run
        // `start_pull_mode` with no intervening `Stop`, which overwrites the still-
        // registered callback bridge (a use-after-free of the SDK callback ctx).
        let outcome: BackendResult<touptek_rs::Frame> = match camera.wait_for_event(READOUT_TIMEOUT)
        {
            Ok(Event::Image | Event::StillImage) => camera
                .pull_image(request.width, request.height, request.bit_depth)
                .map_err(BackendError::from),
            Ok(Event::Error) => Err(BackendError("camera reported an error event".to_string())),
            Ok(Event::Disconnected) => Err(BackendError("camera disconnected".to_string())),
            Ok(Event::Other(code)) => Err(BackendError(format!("unexpected camera event {code}"))),
            Err(e) => Err(BackendError::from(e)),
        };
        let _ = camera.stop();
        Ok(Some(outcome?.data))
    }

    fn request_abort(&self) {
        self.abort.store(true, Ordering::SeqCst);
    }

    fn pulse_guide(&self, direction: GuideDirection, duration_ms: u32) -> BackendResult<()> {
        let guard = self.camera.lock();
        let camera = guard.as_ref().ok_or_else(BackendError::closed)?;
        Ok(camera.st4_pulse_guide(direction, duration_ms)?)
    }
}

// --- tests -----------------------------------------------------------------------

/// Exercise the *production* [`TouptekCameraHandle`] against the `touptek-rs`
/// simulation backend (covers the real SDK wrapper that the BDD suite otherwise
/// reaches only via the spawned binary).
#[cfg(all(test, feature = "simulation"))]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod handle_tests {
    use super::*;

    fn sim_handle() -> TouptekCameraHandle {
        let sdk = touptek_rs::Sdk::new().expect("simulation SDK");
        let info = sdk.enumerate().expect("enumerate")[0].clone();
        TouptekCameraHandle::new(sdk, 0, info, "TOUPTEK:Sim:sim-0".to_string())
    }

    #[test]
    fn production_handle_round_trips_against_the_sim_sdk() {
        let handle = sim_handle();
        assert_eq!(handle.unique_id(), "TOUPTEK:Sim:sim-0");
        assert_eq!(handle.info().id, "sim-0");
        assert!(!handle.is_open());
        handle.open().unwrap();
        assert!(handle.is_open());
        // Capabilities + controls round-trip through the production wrapper.
        let caps = handle.capabilities().unwrap();
        let (gmin, gmax) = caps.gain_range.expect("gain present");
        assert!(gmin <= gmax);
        handle.set_gain(gmax).unwrap();
        assert_eq!(handle.gain().unwrap(), gmax);
        handle.set_offset(42).unwrap();
        assert_eq!(handle.offset().unwrap(), 42);
        handle.set_cooler(true).unwrap();
        assert!(handle.cooler_on().unwrap());
        assert!((0..=100).contains(&handle.cooler_power_percent().unwrap()));
        assert!(handle.temperature_celsius().unwrap().is_finite());
        handle.pulse_guide(GuideDirection::North, 5).unwrap();
        handle.close().unwrap();
        assert!(!handle.is_open());
    }

    #[test]
    fn production_handle_capture_produces_a_frame() {
        let handle = sim_handle();
        handle.open().unwrap();
        let request = CaptureRequest {
            width: 64,
            height: 64,
            bin: 1,
            start_x: 0,
            start_y: 0,
            exposure_us: 1_000,
            bit_depth: 16,
            duration: Duration::from_millis(10),
            is_dark: false,
        };
        let frame = handle.capture(request).unwrap().expect("a completed frame");
        assert_eq!(frame.len(), 64 * 64 * 2);
        handle.close().unwrap();
    }
}

/// A configurable in-memory [`CameraHandle`] for the crate's unit tests, so the
/// device logic — including the paths the `touptek-rs` simulation cannot force,
/// like an SDK open failure (C2), a mid-exposure SDK error (E9), a model without
/// gain/offset (GO1), without ST4 (PG2), or without a cooler (K1) — is exercised
/// without hardware.
#[cfg(test)]
pub(crate) mod mock {
    use super::*;

    /// Build the simulated ToupTek model info (the 3008×3008 colour 16-bit
    /// ATR533C, cooler + ST4), mirroring `touptek-rs`'s simulated `CameraInfo`.
    fn default_info() -> CameraInfo {
        CameraInfo {
            id: "sim-0".to_string(),
            display_name: "Simulated ToupTek Camera".to_string(),
            model_name: "ToupTek ATR533C (simulated)".to_string(),
            flag: u64::from(touptek_rs::sys::TOUPCAM_FLAG_TEC)
                | u64::from(touptek_rs::sys::TOUPCAM_FLAG_ST4)
                | u64::from(touptek_rs::sys::TOUPCAM_FLAG_BLACKLEVEL),
            pixel_size_x: 3.76,
            pixel_size_y: 3.76,
            max_width: 3008,
            max_height: 3008,
            bit_depth: 16,
            is_color: true,
            supported_bins: vec![1, 2, 3, 4],
        }
    }

    #[derive(Debug)]
    pub(crate) struct MockCameraHandle {
        info: CameraInfo,
        gain_supported: bool,
        offset_supported: bool,
        temperature_available: bool,
        open: AtomicBool,
        gain: Mutex<u16>,
        offset: Mutex<i32>,
        target_temp_tenths: Mutex<i16>,
        cooler_on: AtomicBool,
        abort: AtomicBool,
        /// C2 injection: make `open` fail.
        pub fail_open: AtomicBool,
        /// E9 injection: make the next capture fail at the SDK.
        pub fail_capture: AtomicBool,
        /// Optional artificial integration time (for in-flight tests).
        capture_delay: Mutex<Duration>,
    }

    impl Default for MockCameraHandle {
        fn default() -> Self {
            Self {
                info: default_info(),
                gain_supported: true,
                offset_supported: true,
                temperature_available: true,
                open: AtomicBool::new(false),
                gain: Mutex::new(100),
                offset: Mutex::new(0),
                target_temp_tenths: Mutex::new(0),
                cooler_on: AtomicBool::new(false),
                abort: AtomicBool::new(false),
                fail_open: AtomicBool::new(false),
                fail_capture: AtomicBool::new(false),
                capture_delay: Mutex::new(Duration::ZERO),
            }
        }
    }

    impl MockCameraHandle {
        /// Present a model with no gain control (GO1's `NOT_IMPLEMENTED` branch).
        pub fn without_gain(mut self) -> Self {
            self.gain_supported = false;
            self
        }

        /// Present a model with no offset control (GO1's `NOT_IMPLEMENTED` branch).
        pub fn without_offset(mut self) -> Self {
            self.offset_supported = false;
            self
        }

        /// Present a model with no ST4 port (PG2's `NOT_IMPLEMENTED` branch).
        pub fn without_st4(mut self) -> Self {
            self.info.flag &= !u64::from(touptek_rs::sys::TOUPCAM_FLAG_ST4);
            self
        }

        /// Present a non-cooled model (K1's `NOT_IMPLEMENTED` branch).
        pub fn without_cooler(mut self) -> Self {
            self.info.flag &= !u64::from(touptek_rs::sys::TOUPCAM_FLAG_TEC);
            self
        }

        /// Present a model with no sensor-temperature read (K2's
        /// `NOT_IMPLEMENTED` branch).
        pub fn without_temperature(mut self) -> Self {
            self.temperature_available = false;
            self
        }

        /// Present a monochrome model (ST1: `SensorType` Monochrome +
        /// `BayerOffsetX/Y` NOT_IMPLEMENTED). The default mock is the colour
        /// ATR533C, so this flips it back to mono for the mono-path assertions.
        pub fn monochrome(mut self) -> Self {
            self.info.is_color = false;
            self.info.flag |= u64::from(touptek_rs::sys::TOUPCAM_FLAG_MONO);
            self
        }

        pub fn set_capture_delay(&self, delay: Duration) {
            *self.capture_delay.lock() = delay;
        }
    }

    impl CameraHandle for MockCameraHandle {
        fn unique_id(&self) -> String {
            "TOUPTEK:Simulated-ToupTek-Camera:sim-0".to_string()
        }

        fn info(&self) -> CameraInfo {
            self.info.clone()
        }

        fn is_open(&self) -> bool {
            self.open.load(Ordering::SeqCst)
        }

        fn open(&self) -> BackendResult<()> {
            if self.fail_open.load(Ordering::SeqCst) {
                return Err(BackendError("simulated open failure".to_string()));
            }
            self.open.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn close(&self) -> BackendResult<()> {
            self.open.store(false, Ordering::SeqCst);
            Ok(())
        }

        fn capabilities(&self) -> BackendResult<Capabilities> {
            Ok(Capabilities {
                exposure_range_us: (100, 3_600_000_000),
                gain_range: self.gain_supported.then_some((100, 1000)),
                offset_supported: self.offset_supported,
                temperature_available: self.temperature_available,
            })
        }

        fn gain(&self) -> BackendResult<u16> {
            Ok(*self.gain.lock())
        }

        fn set_gain(&self, percent: u16) -> BackendResult<()> {
            *self.gain.lock() = percent;
            Ok(())
        }

        fn offset(&self) -> BackendResult<i32> {
            Ok(*self.offset.lock())
        }

        fn set_offset(&self, value: i32) -> BackendResult<()> {
            *self.offset.lock() = value;
            Ok(())
        }

        fn temperature_celsius(&self) -> BackendResult<f64> {
            if !self.temperature_available {
                return Err(BackendError("no temperature sensor".to_string()));
            }
            Ok(if self.cooler_on.load(Ordering::SeqCst) {
                f64::from(*self.target_temp_tenths.lock()) / 10.0
            } else {
                20.0
            })
        }

        fn set_cooler(&self, on: bool) -> BackendResult<()> {
            self.cooler_on.store(on, Ordering::SeqCst);
            Ok(())
        }

        fn cooler_on(&self) -> BackendResult<bool> {
            Ok(self.cooler_on.load(Ordering::SeqCst))
        }

        fn cooler_power_percent(&self) -> BackendResult<u32> {
            Ok(if self.cooler_on.load(Ordering::SeqCst) {
                60
            } else {
                0
            })
        }

        fn set_target_temperature_tenths(&self, tenths: i16) -> BackendResult<()> {
            *self.target_temp_tenths.lock() = tenths;
            Ok(())
        }

        fn target_temperature_tenths(&self) -> BackendResult<i16> {
            Ok(*self.target_temp_tenths.lock())
        }

        fn capture(&self, request: CaptureRequest) -> BackendResult<Option<Vec<u8>>> {
            self.abort.store(false, Ordering::SeqCst);
            let delay = *self.capture_delay.lock();
            // Sleep against a real-clock DEADLINE (not accumulated naps), checking
            // the abort signal, mirroring the production handle.
            let deadline = Instant::now() + delay;
            let step = Duration::from_millis(10);
            loop {
                if self.abort.load(Ordering::SeqCst) {
                    return Ok(None);
                }
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                std::thread::sleep(step.min(deadline - now));
            }
            if self.abort.load(Ordering::SeqCst) {
                return Ok(None);
            }
            if self.fail_capture.load(Ordering::SeqCst) {
                return Err(BackendError("simulated capture failure".to_string()));
            }
            Ok(Some(vec![
                0u8;
                request.width as usize
                    * request.height as usize
                    * 2
            ]))
        }

        fn request_abort(&self) {
            self.abort.store(true, Ordering::SeqCst);
        }

        fn pulse_guide(&self, _direction: GuideDirection, _duration_ms: u32) -> BackendResult<()> {
            Ok(())
        }
    }
}
