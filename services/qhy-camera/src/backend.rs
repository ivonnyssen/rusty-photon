//! The SDK seam: thin traits over the blocking `qhyccd-rs` `Camera` / `FilterWheel`
//! surface the ASCOM devices use, plus production wrappers and a test mock.
//!
//! Why a seam: `qhyccd-rs` returns its own typed `qhyccd_rs::QHYError` and (for the
//! real backend) talks FFI. Funneling every SDK call through [`CameraHandle`] /
//! [`FilterWheelHandle`] (1) collapses that error into a typed [`BackendError`] at
//! one boundary, (2) lets the
//! ASCOM device hold an `Arc<dyn CameraHandle>` so unit tests can substitute a mock
//! with neither hardware nor the simulation runtime, and (3) keeps the `Control`
//! vocabulary in one place. Production impls wrap the real `qhyccd-rs` handles
//! (which are `Clone` over an internal `Arc`, so cloning shares the open camera).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use qhyccd_rs::{CCDChipArea, CCDChipInfo, Control, ImageData, StreamMode};

/// A QHYCCD SDK call failed. Carries the underlying error message; the ASCOM
/// device decides the `ASCOMError` per call site (the SDK error kind does not
/// map 1:1 to an ASCOM code).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct BackendError(pub String);

impl BackendError {
    /// Collapse any `Display` error (a `qhyccd-rs` `QHYError`) into the typed seam
    /// error; the `{:#}` alternate format lets a chained error print full context.
    fn from_err(err: impl std::fmt::Display) -> Self {
        Self(format!("{err:#}"))
    }
}

type BackendResult<T> = std::result::Result<T, BackendError>;

/// The blocking camera operations the ASCOM `Camera` device drives. Every method
/// is synchronous (the SDK is blocking C FFI); the device offloads the long
/// exposure calls onto `spawn_blocking`.
pub trait CameraHandle: std::fmt::Debug + Send + Sync {
    /// SDK camera id (e.g. `"SIM-QHY178M"` / `"QHY600M-<serial>"`).
    fn id(&self) -> String;

    fn open(&self) -> BackendResult<()>;
    fn close(&self) -> BackendResult<()>;
    fn is_open(&self) -> BackendResult<bool>;
    fn init(&self) -> BackendResult<()>;

    /// Force single-frame (long-exposure) stream mode.
    fn set_stream_mode_single(&self) -> BackendResult<()>;
    fn set_readout_mode(&self, mode: u32) -> BackendResult<()>;
    fn get_readout_mode(&self) -> BackendResult<u32>;
    fn get_number_of_readout_modes(&self) -> BackendResult<u32>;
    fn get_readout_mode_name(&self, index: u32) -> BackendResult<String>;
    fn get_readout_mode_resolution(&self, index: u32) -> BackendResult<(u32, u32)>;

    /// Force 16-bit USB transfer if the control is available.
    fn set_transfer_bit_16(&self) -> BackendResult<()>;

    fn get_model(&self) -> BackendResult<String>;
    fn get_ccd_info(&self) -> BackendResult<CCDChipInfo>;
    fn get_effective_area(&self) -> BackendResult<CCDChipArea>;

    /// `Some(value)` when the control exists. For `Control::CamColor` the value is
    /// the Bayer-pattern discriminant; for other controls it is a presence flag.
    fn is_control_available(&self, control: Control) -> Option<u32>;
    fn get_parameter(&self, control: Control) -> BackendResult<f64>;
    fn get_parameter_min_max_step(&self, control: Control) -> BackendResult<(f64, f64, f64)>;
    fn set_parameter(&self, control: Control, value: f64) -> BackendResult<()>;

    fn set_bin_mode(&self, bin_x: u32, bin_y: u32) -> BackendResult<()>;
    fn set_roi(&self, area: CCDChipArea) -> BackendResult<()>;

    fn start_single_frame_exposure(&self) -> BackendResult<()>;
    fn get_image_size(&self) -> BackendResult<usize>;
    fn get_single_frame(&self, buffer_size: usize) -> BackendResult<ImageData>;
    fn get_remaining_exposure_us(&self) -> BackendResult<u32>;
    fn abort_exposure_and_readout(&self) -> BackendResult<()>;
}

/// The CFW operations the ASCOM `FilterWheel` device drives.
pub trait FilterWheelHandle: std::fmt::Debug + Send + Sync {
    fn id(&self) -> String;
    fn open(&self) -> BackendResult<()>;
    fn close(&self) -> BackendResult<()>;
    fn is_open(&self) -> BackendResult<bool>;
    /// Number of slots (via `Control::CfwSlotsNum`).
    fn get_number_of_filters(&self) -> BackendResult<u32>;
    /// Current 0-indexed slot.
    fn get_position(&self) -> BackendResult<u32>;
    /// Command a move to a 0-indexed slot.
    fn set_position(&self, position: u32) -> BackendResult<()>;
}

// --- production wrappers over qhyccd-rs ----------------------------------------

/// One physical QHYCCD connection shared by the Camera and (when present) the
/// FilterWheel ASCOM devices that map to the same SDK id. A QHY CFW is wired to
/// the camera's USB and driven through the camera handle (`Control::CfwPort`), so
/// both ASCOM devices must talk to ONE physical `OpenQHYCCD` handle. We refcount
/// logical connects and only `CloseQHYCCD` on the LAST disconnect: opening the
/// same camera id as two handles and closing either one tears down the shared
/// physical device and breaks the other (confirmed on real hardware 2026-06-18 —
/// see `docs/services/qhy-camera.md` "Camera + CFW share one physical handle").
#[derive(Debug)]
pub struct SharedCameraConnection {
    camera: qhyccd_rs::Camera,
    /// Count of logically-connected ASCOM devices (camera + CFW) holding the
    /// physical handle open. Opens once on 0 → 1 and closes on 1 → 0.
    refs: Mutex<u32>,
}

impl SharedCameraConnection {
    pub fn new(camera: qhyccd_rs::Camera) -> Arc<Self> {
        Arc::new(Self {
            camera,
            refs: Mutex::new(0),
        })
    }

    /// The shared `qhyccd-rs` camera both ASCOM devices operate through. The CFW
    /// device clones it (the clone shares the same internal handle `Arc`) so a
    /// single `OpenQHYCCD` serves both imaging and filter-wheel control.
    pub fn camera(&self) -> &qhyccd_rs::Camera {
        &self.camera
    }

    /// Register a logical connect for `connected` (the calling device's own flag).
    /// The flag read/set AND the refcount mutation happen in ONE critical section
    /// (under `refs`), so they can never desync under concurrent connect/disconnect
    /// of the same device — otherwise a `connect`'s flag-set and a racing
    /// `disconnect`'s ref-drop could interleave, leaking the physical handle and
    /// corrupting the refcount. Physically `open` on the 0 → 1 transition; a no-op
    /// if this device was already connected.
    fn connect(&self, connected: &AtomicBool) -> BackendResult<()> {
        let mut refs = self.refs.lock();
        if connected.load(Ordering::SeqCst) {
            return Ok(());
        }
        if *refs == 0 {
            self.camera.open().map_err(BackendError::from_err)?;
        }
        *refs += 1;
        connected.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Register a logical disconnect for `connected`. Symmetric to
    /// [`Self::connect`]: physically `close` on the 1 → 0 transition; a no-op if
    /// this device was not connected. The flag is cleared and the ref dropped even
    /// if the physical `close` errors, so logical state never desyncs.
    fn disconnect(&self, connected: &AtomicBool) -> BackendResult<()> {
        let mut refs = self.refs.lock();
        if !connected.load(Ordering::SeqCst) {
            return Ok(());
        }
        connected.store(false, Ordering::SeqCst);
        *refs -= 1; // > 0 here: this device held a ref while `connected` was true
        if *refs == 0 {
            self.camera.close().map_err(BackendError::from_err)?;
        }
        Ok(())
    }
}

/// Production [`CameraHandle`] over a [`SharedCameraConnection`]. Holds its own
/// logical-`connected` flag so `is_open()` reflects THIS device's ASCOM
/// connection state, independent of the shared physical handle (and the CFW).
#[derive(Debug)]
pub struct QhyCameraHandle {
    conn: Arc<SharedCameraConnection>,
    connected: AtomicBool,
}

impl QhyCameraHandle {
    pub fn new(conn: Arc<SharedCameraConnection>) -> Self {
        Self {
            conn,
            connected: AtomicBool::new(false),
        }
    }
}

impl CameraHandle for QhyCameraHandle {
    fn id(&self) -> String {
        self.conn.camera().id().to_string()
    }
    fn open(&self) -> BackendResult<()> {
        self.conn.connect(&self.connected)
    }
    fn close(&self) -> BackendResult<()> {
        self.conn.disconnect(&self.connected)
    }
    fn is_open(&self) -> BackendResult<bool> {
        Ok(self.connected.load(Ordering::SeqCst))
    }
    fn init(&self) -> BackendResult<()> {
        self.conn.camera().init().map_err(BackendError::from_err)
    }
    fn set_stream_mode_single(&self) -> BackendResult<()> {
        self.conn
            .camera()
            .set_stream_mode(StreamMode::SingleFrameMode)
            .map_err(BackendError::from_err)
    }
    fn set_readout_mode(&self, mode: u32) -> BackendResult<()> {
        self.conn
            .camera()
            .set_readout_mode(mode)
            .map_err(BackendError::from_err)
    }
    fn get_readout_mode(&self) -> BackendResult<u32> {
        self.conn
            .camera()
            .get_readout_mode()
            .map_err(BackendError::from_err)
    }
    fn get_number_of_readout_modes(&self) -> BackendResult<u32> {
        self.conn
            .camera()
            .get_number_of_readout_modes()
            .map_err(BackendError::from_err)
    }
    fn get_readout_mode_name(&self, index: u32) -> BackendResult<String> {
        self.conn
            .camera()
            .get_readout_mode_name(index)
            .map_err(BackendError::from_err)
    }
    fn get_readout_mode_resolution(&self, index: u32) -> BackendResult<(u32, u32)> {
        self.conn
            .camera()
            .get_readout_mode_resolution(index)
            .map_err(BackendError::from_err)
    }
    fn set_transfer_bit_16(&self) -> BackendResult<()> {
        self.conn
            .camera()
            .set_if_available(Control::TransferBit, 16.0)
            .map_err(BackendError::from_err)
    }
    fn get_model(&self) -> BackendResult<String> {
        self.conn
            .camera()
            .get_model()
            .map_err(BackendError::from_err)
    }
    fn get_ccd_info(&self) -> BackendResult<CCDChipInfo> {
        self.conn
            .camera()
            .get_ccd_info()
            .map_err(BackendError::from_err)
    }
    fn get_effective_area(&self) -> BackendResult<CCDChipArea> {
        self.conn
            .camera()
            .get_effective_area()
            .map_err(BackendError::from_err)
    }
    fn is_control_available(&self, control: Control) -> Option<u32> {
        self.conn.camera().is_control_available(control)
    }
    fn get_parameter(&self, control: Control) -> BackendResult<f64> {
        self.conn
            .camera()
            .get_parameter(control)
            .map_err(BackendError::from_err)
    }
    fn get_parameter_min_max_step(&self, control: Control) -> BackendResult<(f64, f64, f64)> {
        self.conn
            .camera()
            .get_parameter_min_max_step(control)
            .map_err(BackendError::from_err)
    }
    fn set_parameter(&self, control: Control, value: f64) -> BackendResult<()> {
        self.conn
            .camera()
            .set_parameter(control, value)
            .map_err(BackendError::from_err)
    }
    fn set_bin_mode(&self, bin_x: u32, bin_y: u32) -> BackendResult<()> {
        self.conn
            .camera()
            .set_bin_mode(bin_x, bin_y)
            .map_err(BackendError::from_err)
    }
    fn set_roi(&self, area: CCDChipArea) -> BackendResult<()> {
        self.conn
            .camera()
            .set_roi(area)
            .map_err(BackendError::from_err)
    }
    fn start_single_frame_exposure(&self) -> BackendResult<()> {
        self.conn
            .camera()
            .start_single_frame_exposure()
            .map_err(BackendError::from_err)
    }
    fn get_image_size(&self) -> BackendResult<usize> {
        self.conn
            .camera()
            .get_image_size()
            .map_err(BackendError::from_err)
    }
    fn get_single_frame(&self, buffer_size: usize) -> BackendResult<ImageData> {
        self.conn
            .camera()
            .get_single_frame(buffer_size)
            .map_err(BackendError::from_err)
    }
    fn get_remaining_exposure_us(&self) -> BackendResult<u32> {
        self.conn
            .camera()
            .get_remaining_exposure_us()
            .map_err(BackendError::from_err)
    }
    fn abort_exposure_and_readout(&self) -> BackendResult<()> {
        self.conn
            .camera()
            .abort_exposure_and_readout()
            .map_err(BackendError::from_err)
    }
}

/// Production [`FilterWheelHandle`] over a [`SharedCameraConnection`] (a QHY CFW
/// is driven through the camera handle). Shares the physical connection with the
/// Camera device via the refcount, and keeps its own logical-`connected` flag.
#[derive(Debug)]
pub struct QhyFilterWheelHandle {
    conn: Arc<SharedCameraConnection>,
    /// CFW view over a clone of the shared camera (the clone shares the same
    /// internal handle `Arc`); used only for the filter data calls below.
    wheel: qhyccd_rs::FilterWheel,
    connected: AtomicBool,
}

impl QhyFilterWheelHandle {
    pub fn new(conn: Arc<SharedCameraConnection>) -> Self {
        let wheel = qhyccd_rs::FilterWheel::new(conn.camera().clone());
        Self {
            conn,
            wheel,
            connected: AtomicBool::new(false),
        }
    }
}

impl FilterWheelHandle for QhyFilterWheelHandle {
    fn id(&self) -> String {
        self.conn.camera().id().to_string()
    }
    fn open(&self) -> BackendResult<()> {
        self.conn.connect(&self.connected)
    }
    fn close(&self) -> BackendResult<()> {
        self.conn.disconnect(&self.connected)
    }
    fn is_open(&self) -> BackendResult<bool> {
        Ok(self.connected.load(Ordering::SeqCst))
    }
    fn get_number_of_filters(&self) -> BackendResult<u32> {
        self.wheel
            .get_number_of_filters()
            .map_err(BackendError::from_err)
    }
    fn get_position(&self) -> BackendResult<u32> {
        self.wheel.get_fw_position().map_err(BackendError::from_err)
    }
    fn set_position(&self, position: u32) -> BackendResult<()> {
        self.wheel
            .set_fw_position(position)
            .map_err(BackendError::from_err)
    }
}

// --- test mock -----------------------------------------------------------------

/// A configurable in-memory [`CameraHandle`] / [`FilterWheelHandle`] used by the
/// crate's unit tests, so the device logic — including paths the `qhyccd-rs`
/// simulation cannot easily force, like a mid-exposure SDK error (contract E9) —
/// is exercised without hardware and without any *real* SDK calls. (The static
/// `qhyccd` lib is still linked into the test binary — it is an unconditional
/// link dependency of `libqhyccd-sys`; the mock only replaces the runtime seam.)
#[cfg(test)]
pub(crate) mod mock {
    // `#[cfg(test)]`-gated test-helper infrastructure that never links into a
    // production binary. Excluded from coverage so the figure reflects only
    // production-shipped code — counting these never-shipped mock lines would
    // produce false numbers (matches the six service `mock` modules excluded in
    // PR #370 and the `#[cfg(test)] mod tests` blocks).
    #![cfg_attr(coverage_nightly, coverage(off))]

    use super::*;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    #[derive(Debug)]
    pub(crate) struct MockCameraHandle {
        pub id: String,
        pub model: String,
        open: AtomicBool,
        /// Controls reported available; for `CamColor`, the stored value is the
        /// Bayer discriminant returned by `is_control_available`.
        controls: Mutex<HashMap<Control, u32>>,
        params: Mutex<HashMap<Control, f64>>,
        ranges: Mutex<HashMap<Control, (f64, f64, f64)>>,
        ccd_info: CCDChipInfo,
        effective_area: CCDChipArea,
        readout_modes: Vec<(String, (u32, u32))>,
        roi: Mutex<CCDChipArea>,
        bin: Mutex<(u32, u32)>,
        /// E9 injection: make the next single-frame exposure fail.
        pub fail_single_frame: AtomicBool,
        /// C2 injection: make the post-open handshake (`get_ccd_info`) fail.
        pub fail_handshake: AtomicBool,
        /// Make control *writes* (`set_bin_mode` / `set_readout_mode`) fail, to
        /// exercise the setters' SDK-failure → `INVALID_OPERATION` mapping.
        pub fail_set_controls: AtomicBool,
        /// Optional artificial blocking of `get_single_frame` (for in-flight tests).
        single_frame_delay: Mutex<Duration>,
        aborted: AtomicBool,
    }

    impl Default for MockCameraHandle {
        fn default() -> Self {
            // Mirrors the `qhyccd-rs` QHY178M-Simulated: 3072x2048, 16-bit mono,
            // cooler, gain/offset, single readout mode, bins 1 & 2, shutterless.
            let mut controls = HashMap::new();
            for c in [
                Control::Gain,
                Control::Offset,
                Control::Exposure,
                Control::CamSingleFrameMode,
                Control::CamBin1x1mode,
                Control::CamBin2x2mode,
                Control::Cooler,
                Control::CurTemp,
                Control::CurPWM,
                Control::ManualPWM,
                Control::TransferBit,
            ] {
                controls.insert(c, 1);
            }
            let mut ranges = HashMap::new();
            ranges.insert(Control::Gain, (0.0, 100.0, 1.0));
            ranges.insert(Control::Offset, (0.0, 255.0, 1.0));
            ranges.insert(Control::Exposure, (1.0, 3_600_000_000.0, 1.0));
            let mut params = HashMap::new();
            params.insert(Control::Gain, 10.0);
            params.insert(Control::Offset, 10.0);
            params.insert(Control::CurTemp, 20.0);
            params.insert(Control::CurPWM, 0.0);
            params.insert(Control::OutputDataActualBits, 16.0);
            let area = CCDChipArea {
                start_x: 0,
                start_y: 0,
                width: 3072,
                height: 2048,
            };
            Self {
                id: "SIM-QHY178M".to_string(),
                model: "QHY178M-Simulated".to_string(),
                open: AtomicBool::new(false),
                controls: Mutex::new(controls),
                params: Mutex::new(params),
                ranges: Mutex::new(ranges),
                ccd_info: CCDChipInfo {
                    // um, matching the real SDK (chip dims ≈ image_dim × pixel_size).
                    chip_width: 7372.8,  // um (3072 × 2.4)
                    chip_height: 4915.2, // um (2048 × 2.4)
                    image_width: 3072,
                    image_height: 2048,
                    pixel_width: 2.4,
                    pixel_height: 2.4,
                    bits_per_pixel: 16,
                },
                effective_area: area,
                readout_modes: vec![("Standard".to_string(), (3072, 2048))],
                roi: Mutex::new(area),
                bin: Mutex::new((1, 1)),
                fail_single_frame: AtomicBool::new(false),
                fail_handshake: AtomicBool::new(false),
                fail_set_controls: AtomicBool::new(false),
                single_frame_delay: Mutex::new(Duration::ZERO),
                aborted: AtomicBool::new(false),
            }
        }
    }

    impl MockCameraHandle {
        /// Drop a control so it reports unavailable (e.g. remove `Gain` to test
        /// the `NOT_IMPLEMENTED` gate).
        pub fn without_control(self, control: Control) -> Self {
            self.controls.lock().remove(&control);
            self
        }
        /// Add a control with a presence/value (e.g. `CamColor` → Bayer code, or
        /// `CamMechanicalShutter` to report a shutter).
        pub fn with_control(self, control: Control, value: u32) -> Self {
            self.controls.lock().insert(control, value);
            self
        }
        /// Override a numeric parameter returned by `get_parameter` (e.g. set
        /// `OutputDataActualBits` to 0 to mimic the QHY5III715C's SDK quirk).
        pub fn with_param(self, control: Control, value: f64) -> Self {
            self.params.lock().insert(control, value);
            self
        }
        /// Read back a parameter value as last written via `set_parameter`
        /// (test introspection, e.g. to assert which cooler control a call
        /// wrote to).
        pub fn param(&self, control: Control) -> Option<f64> {
            self.params.lock().get(&control).copied()
        }
        pub fn set_single_frame_delay(&self, delay: Duration) {
            *self.single_frame_delay.lock() = delay;
        }
    }

    impl CameraHandle for MockCameraHandle {
        fn id(&self) -> String {
            self.id.clone()
        }
        fn open(&self) -> BackendResult<()> {
            self.open.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn close(&self) -> BackendResult<()> {
            self.open.store(false, Ordering::SeqCst);
            Ok(())
        }
        fn is_open(&self) -> BackendResult<bool> {
            Ok(self.open.load(Ordering::SeqCst))
        }
        fn init(&self) -> BackendResult<()> {
            Ok(())
        }
        fn set_stream_mode_single(&self) -> BackendResult<()> {
            Ok(())
        }
        fn set_readout_mode(&self, mode: u32) -> BackendResult<()> {
            if self.fail_set_controls.load(Ordering::SeqCst) {
                return Err(BackendError(
                    "simulated set_readout_mode failure".to_string(),
                ));
            }
            if (mode as usize) < self.readout_modes.len() {
                Ok(())
            } else {
                Err(BackendError("readout mode out of range".to_string()))
            }
        }
        fn get_readout_mode(&self) -> BackendResult<u32> {
            Ok(0)
        }
        fn get_number_of_readout_modes(&self) -> BackendResult<u32> {
            Ok(self.readout_modes.len() as u32)
        }
        fn get_readout_mode_name(&self, index: u32) -> BackendResult<String> {
            self.readout_modes
                .get(index as usize)
                .map(|(name, _)| name.clone())
                .ok_or_else(|| BackendError("readout mode index out of range".to_string()))
        }
        fn get_readout_mode_resolution(&self, index: u32) -> BackendResult<(u32, u32)> {
            self.readout_modes
                .get(index as usize)
                .map(|(_, res)| *res)
                .ok_or_else(|| BackendError("readout mode index out of range".to_string()))
        }
        fn set_transfer_bit_16(&self) -> BackendResult<()> {
            Ok(())
        }
        fn get_model(&self) -> BackendResult<String> {
            Ok(self.model.clone())
        }
        fn get_ccd_info(&self) -> BackendResult<CCDChipInfo> {
            if self.fail_handshake.load(Ordering::SeqCst) {
                return Err(BackendError("simulated handshake failure".to_string()));
            }
            Ok(self.ccd_info)
        }
        fn get_effective_area(&self) -> BackendResult<CCDChipArea> {
            Ok(self.effective_area)
        }
        fn is_control_available(&self, control: Control) -> Option<u32> {
            self.controls.lock().get(&control).copied()
        }
        fn get_parameter(&self, control: Control) -> BackendResult<f64> {
            self.params
                .lock()
                .get(&control)
                .copied()
                .ok_or_else(|| BackendError(format!("no parameter {control:?}")))
        }
        fn get_parameter_min_max_step(&self, control: Control) -> BackendResult<(f64, f64, f64)> {
            self.ranges
                .lock()
                .get(&control)
                .copied()
                .ok_or_else(|| BackendError(format!("no range for {control:?}")))
        }
        fn set_parameter(&self, control: Control, value: f64) -> BackendResult<()> {
            // Mirror the simulation's cooler routing so device-level cooling tests
            // observe the same coupling as the live backend.
            match control {
                Control::ManualPWM => {
                    self.params.lock().insert(Control::CurPWM, value);
                }
                Control::Cooler => {
                    self.params.lock().insert(Control::Cooler, value);
                }
                _ => {
                    self.params.lock().insert(control, value);
                }
            }
            Ok(())
        }
        fn set_bin_mode(&self, bin_x: u32, bin_y: u32) -> BackendResult<()> {
            if self.fail_set_controls.load(Ordering::SeqCst) {
                return Err(BackendError("simulated set_bin_mode failure".to_string()));
            }
            *self.bin.lock() = (bin_x, bin_y);
            Ok(())
        }
        fn set_roi(&self, area: CCDChipArea) -> BackendResult<()> {
            *self.roi.lock() = area;
            Ok(())
        }
        fn start_single_frame_exposure(&self) -> BackendResult<()> {
            self.aborted.store(false, Ordering::SeqCst);
            Ok(())
        }
        fn get_image_size(&self) -> BackendResult<usize> {
            let roi = *self.roi.lock();
            Ok((roi.width * roi.height * 2) as usize)
        }
        fn get_single_frame(&self, _buffer_size: usize) -> BackendResult<ImageData> {
            // Sleep in small chunks so a concurrent abort returns promptly (mirrors
            // the SDK's cancellable readout), rather than blocking the full delay.
            let delay = *self.single_frame_delay.lock();
            let mut slept = Duration::ZERO;
            while slept < delay {
                if self.aborted.load(Ordering::SeqCst) {
                    return Err(BackendError("exposure aborted".to_string()));
                }
                let step = Duration::from_millis(10).min(delay - slept);
                std::thread::sleep(step);
                slept += step;
            }
            if self.aborted.load(Ordering::SeqCst) {
                return Err(BackendError("exposure aborted".to_string()));
            }
            if self.fail_single_frame.load(Ordering::SeqCst) {
                return Err(BackendError("simulated capture failure".to_string()));
            }
            let roi = *self.roi.lock();
            Ok(ImageData {
                data: vec![0u8; (roi.width * roi.height * 2) as usize],
                width: roi.width,
                height: roi.height,
                bits_per_pixel: 16,
                channels: 1,
            })
        }
        fn get_remaining_exposure_us(&self) -> BackendResult<u32> {
            Ok(0)
        }
        fn abort_exposure_and_readout(&self) -> BackendResult<()> {
            self.aborted.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Debug)]
    pub(crate) struct MockFilterWheelHandle {
        pub id: String,
        open: AtomicBool,
        filters: u32,
        position: Mutex<u32>,
        /// Make the post-open handshake (`get_number_of_filters`) fail, so the
        /// `connect` cleanup path (close-on-failed-handshake) can be tested.
        pub fail_handshake: AtomicBool,
    }

    impl MockFilterWheelHandle {
        pub fn new(id: &str, filters: u32) -> Self {
            Self {
                id: id.to_string(),
                open: AtomicBool::new(false),
                filters,
                position: Mutex::new(0),
                fail_handshake: AtomicBool::new(false),
            }
        }
    }

    impl FilterWheelHandle for MockFilterWheelHandle {
        fn id(&self) -> String {
            self.id.clone()
        }
        fn open(&self) -> BackendResult<()> {
            self.open.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn close(&self) -> BackendResult<()> {
            self.open.store(false, Ordering::SeqCst);
            Ok(())
        }
        fn is_open(&self) -> BackendResult<bool> {
            Ok(self.open.load(Ordering::SeqCst))
        }
        fn get_number_of_filters(&self) -> BackendResult<u32> {
            if self.fail_handshake.load(Ordering::SeqCst) {
                return Err(BackendError("simulated handshake failure".to_string()));
            }
            Ok(self.filters)
        }
        fn get_position(&self) -> BackendResult<u32> {
            Ok(*self.position.lock())
        }
        fn set_position(&self, position: u32) -> BackendResult<()> {
            *self.position.lock() = position;
            Ok(())
        }
    }
}

// --- shared-connection refcount tests ------------------------------------------

/// Tests for [`SharedCameraConnection`] — the refcount that makes the Camera and
/// FilterWheel devices share ONE physical handle so disconnecting one does not
/// tear down the other (the real-hardware bug fixed 2026-06-18). They exercise
/// the refcount against the `qhyccd-rs` simulation backend (`Sdk::new()`
/// fabricates a QHY178M-Simulated camera + CFW), so they need no hardware.
///
/// Gated on `feature = "simulation"`: `cargo --all-features` / `cargo rail` turn
/// it on and run these, but the Bazel `qhy-camera_unit_test` target links the
/// REAL SDK (no `simulation`), where `Sdk::new()` would scan USB and find no
/// camera — so they are correctly compiled out there.
#[cfg(all(test, feature = "simulation"))]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod conn_tests {
    use super::*;

    fn sim_camera() -> qhyccd_rs::Camera {
        let sdk = qhyccd_rs::Sdk::new().expect("simulated SDK");
        let camera = sdk.cameras().next().expect("simulated camera").clone();
        camera
    }

    #[test]
    fn cfw_disconnect_keeps_camera_physically_open() {
        let conn = SharedCameraConnection::new(sim_camera());
        let cam = QhyCameraHandle::new(conn.clone());
        let fw = QhyFilterWheelHandle::new(conn.clone());

        // Nothing connected yet → physically closed.
        assert!(!conn.camera().is_open().unwrap());
        cam.open().unwrap();
        fw.open().unwrap();
        assert!(cam.is_open().unwrap() && fw.is_open().unwrap());
        assert!(conn.camera().is_open().unwrap(), "physical handle open");

        // Disconnecting the CFW must NOT close the shared physical handle while
        // the camera is still connected — the bug this fixes.
        fw.close().unwrap();
        assert!(!fw.is_open().unwrap());
        assert!(cam.is_open().unwrap());
        assert!(
            conn.camera().is_open().unwrap(),
            "camera must stay physically open after CFW disconnect"
        );

        // The last disconnect drops the refcount to 0 and closes physically.
        cam.close().unwrap();
        assert!(!cam.is_open().unwrap());
        assert!(
            !conn.camera().is_open().unwrap(),
            "physical handle closed on last disconnect"
        );
    }

    #[test]
    fn camera_disconnect_keeps_cfw_physically_open() {
        let conn = SharedCameraConnection::new(sim_camera());
        let cam = QhyCameraHandle::new(conn.clone());
        let fw = QhyFilterWheelHandle::new(conn.clone());
        cam.open().unwrap();
        fw.open().unwrap();

        // Symmetric direction: disconnecting the camera leaves the CFW usable.
        cam.close().unwrap();
        assert!(!cam.is_open().unwrap());
        assert!(fw.is_open().unwrap());
        assert!(
            conn.camera().is_open().unwrap(),
            "CFW keeps the handle open"
        );
        // CFW data calls still work through the shared handle.
        fw.get_number_of_filters().unwrap();

        fw.close().unwrap();
        assert!(!conn.camera().is_open().unwrap());
    }

    #[test]
    fn redundant_open_does_not_double_count_the_refcount() {
        let conn = SharedCameraConnection::new(sim_camera());
        let cam = QhyCameraHandle::new(conn.clone());
        cam.open().unwrap();
        cam.open().unwrap(); // idempotent: must not push the refcount to 2
        cam.close().unwrap();
        assert!(
            !conn.camera().is_open().unwrap(),
            "a single close after redundant opens must fully close the device"
        );
    }

    #[test]
    fn concurrent_connect_disconnect_keeps_refcount_consistent() {
        // Regression guard for the flag/refcount desync: the per-device flag and
        // the shared refcount must move together under one lock. Hammer open/close
        // on both devices from many threads; since every open is immediately
        // matched by a close, both devices end disconnected and the shared handle
        // MUST be physically closed (refs back to 0). A desync (flag set without
        // the matching ref bump, or vice-versa) would leak it open.
        let conn = SharedCameraConnection::new(sim_camera());
        let cam = QhyCameraHandle::new(conn.clone());
        let fw = QhyFilterWheelHandle::new(conn.clone());
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    for _ in 0..3_000 {
                        let _ = cam.open();
                        let _ = cam.close();
                        let _ = fw.open();
                        let _ = fw.close();
                    }
                });
            }
        });
        assert!(!cam.is_open().unwrap());
        assert!(!fw.is_open().unwrap());
        assert!(
            !conn.camera().is_open().unwrap(),
            "physical handle must be closed once both devices are disconnected — \
             a flag/refcount desync would leak it open"
        );
    }
}
