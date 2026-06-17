//! The SDK seam: thin traits over the blocking `qhyccd-rs` `Camera` / `FilterWheel`
//! surface the ASCOM devices use, plus production wrappers and a test mock.
//!
//! Why a seam: `qhyccd-rs` returns `eyre::Result` and (for the real backend) talks
//! FFI. Funneling every SDK call through [`CameraHandle`] / [`FilterWheelHandle`]
//! (1) collapses `eyre` into a typed [`BackendError`] at one boundary, (2) lets the
//! ASCOM device hold an `Arc<dyn CameraHandle>` so unit tests can substitute a mock
//! with neither hardware nor the simulation runtime, and (3) keeps the `Control`
//! vocabulary in one place. Production impls wrap the real `qhyccd-rs` handles
//! (which are `Clone` over an internal `Arc`, so cloning shares the open camera).

use qhyccd_rs::{CCDChipArea, CCDChipInfo, Control, ImageData, StreamMode};

/// A QHYCCD SDK call failed. Carries the underlying (eyre) message; the ASCOM
/// device decides the `ASCOMError` per call site (the SDK error kind does not
/// map 1:1 to an ASCOM code).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct BackendError(pub String);

impl BackendError {
    /// Collapse any `Display` error (the `qhyccd-rs` `eyre::Report`) into the
    /// typed seam error, using the alternate format so eyre prints the full chain.
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

/// Production [`CameraHandle`] over a real (or `qhyccd-rs`-simulated) `Camera`.
#[derive(Debug)]
pub struct QhyCameraHandle(qhyccd_rs::Camera);

impl QhyCameraHandle {
    pub fn new(camera: qhyccd_rs::Camera) -> Self {
        Self(camera)
    }
}

impl CameraHandle for QhyCameraHandle {
    fn id(&self) -> String {
        self.0.id().to_string()
    }
    fn open(&self) -> BackendResult<()> {
        self.0.open().map_err(BackendError::from_err)
    }
    fn close(&self) -> BackendResult<()> {
        self.0.close().map_err(BackendError::from_err)
    }
    fn is_open(&self) -> BackendResult<bool> {
        self.0.is_open().map_err(BackendError::from_err)
    }
    fn init(&self) -> BackendResult<()> {
        self.0.init().map_err(BackendError::from_err)
    }
    fn set_stream_mode_single(&self) -> BackendResult<()> {
        self.0
            .set_stream_mode(StreamMode::SingleFrameMode)
            .map_err(BackendError::from_err)
    }
    fn set_readout_mode(&self, mode: u32) -> BackendResult<()> {
        self.0
            .set_readout_mode(mode)
            .map_err(BackendError::from_err)
    }
    fn get_readout_mode(&self) -> BackendResult<u32> {
        self.0.get_readout_mode().map_err(BackendError::from_err)
    }
    fn get_number_of_readout_modes(&self) -> BackendResult<u32> {
        self.0
            .get_number_of_readout_modes()
            .map_err(BackendError::from_err)
    }
    fn get_readout_mode_name(&self, index: u32) -> BackendResult<String> {
        self.0
            .get_readout_mode_name(index)
            .map_err(BackendError::from_err)
    }
    fn get_readout_mode_resolution(&self, index: u32) -> BackendResult<(u32, u32)> {
        self.0
            .get_readout_mode_resolution(index)
            .map_err(BackendError::from_err)
    }
    fn set_transfer_bit_16(&self) -> BackendResult<()> {
        self.0
            .set_if_available(Control::TransferBit, 16.0)
            .map_err(BackendError::from_err)
    }
    fn get_model(&self) -> BackendResult<String> {
        self.0.get_model().map_err(BackendError::from_err)
    }
    fn get_ccd_info(&self) -> BackendResult<CCDChipInfo> {
        self.0.get_ccd_info().map_err(BackendError::from_err)
    }
    fn get_effective_area(&self) -> BackendResult<CCDChipArea> {
        self.0.get_effective_area().map_err(BackendError::from_err)
    }
    fn is_control_available(&self, control: Control) -> Option<u32> {
        self.0.is_control_available(control)
    }
    fn get_parameter(&self, control: Control) -> BackendResult<f64> {
        self.0
            .get_parameter(control)
            .map_err(BackendError::from_err)
    }
    fn get_parameter_min_max_step(&self, control: Control) -> BackendResult<(f64, f64, f64)> {
        self.0
            .get_parameter_min_max_step(control)
            .map_err(BackendError::from_err)
    }
    fn set_parameter(&self, control: Control, value: f64) -> BackendResult<()> {
        self.0
            .set_parameter(control, value)
            .map_err(BackendError::from_err)
    }
    fn set_bin_mode(&self, bin_x: u32, bin_y: u32) -> BackendResult<()> {
        self.0
            .set_bin_mode(bin_x, bin_y)
            .map_err(BackendError::from_err)
    }
    fn set_roi(&self, area: CCDChipArea) -> BackendResult<()> {
        self.0.set_roi(area).map_err(BackendError::from_err)
    }
    fn start_single_frame_exposure(&self) -> BackendResult<()> {
        self.0
            .start_single_frame_exposure()
            .map_err(BackendError::from_err)
    }
    fn get_image_size(&self) -> BackendResult<usize> {
        self.0.get_image_size().map_err(BackendError::from_err)
    }
    fn get_single_frame(&self, buffer_size: usize) -> BackendResult<ImageData> {
        self.0
            .get_single_frame(buffer_size)
            .map_err(BackendError::from_err)
    }
    fn get_remaining_exposure_us(&self) -> BackendResult<u32> {
        self.0
            .get_remaining_exposure_us()
            .map_err(BackendError::from_err)
    }
    fn abort_exposure_and_readout(&self) -> BackendResult<()> {
        self.0
            .abort_exposure_and_readout()
            .map_err(BackendError::from_err)
    }
}

/// Production [`FilterWheelHandle`] over a real (or simulated) `FilterWheel`.
#[derive(Debug)]
pub struct QhyFilterWheelHandle(qhyccd_rs::FilterWheel);

impl QhyFilterWheelHandle {
    pub fn new(filter_wheel: qhyccd_rs::FilterWheel) -> Self {
        Self(filter_wheel)
    }
}

impl FilterWheelHandle for QhyFilterWheelHandle {
    fn id(&self) -> String {
        self.0.id().to_string()
    }
    fn open(&self) -> BackendResult<()> {
        self.0.open().map_err(BackendError::from_err)
    }
    fn close(&self) -> BackendResult<()> {
        self.0.close().map_err(BackendError::from_err)
    }
    fn is_open(&self) -> BackendResult<bool> {
        self.0.is_open().map_err(BackendError::from_err)
    }
    fn get_number_of_filters(&self) -> BackendResult<u32> {
        self.0
            .get_number_of_filters()
            .map_err(BackendError::from_err)
    }
    fn get_position(&self) -> BackendResult<u32> {
        self.0.get_fw_position().map_err(BackendError::from_err)
    }
    fn set_position(&self, position: u32) -> BackendResult<()> {
        self.0
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
                    chip_width: 7.4,
                    chip_height: 5.0,
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
