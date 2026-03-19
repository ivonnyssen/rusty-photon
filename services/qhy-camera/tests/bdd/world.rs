use std::sync::Arc;

use cucumber::World;

use qhy_camera::io::{
    CameraHandle, CcdChipArea, CcdChipInfo, Control, FilterWheelHandle, ImageData, StreamMode,
};
use qhy_camera::{CameraConfig, FilterWheelConfig, QhyccdCamera, QhyccdFilterWheel};

/// Test world for QHY Camera BDD tests
#[derive(Debug, Default, World)]
pub struct QhyCameraWorld {
    pub camera: Option<Arc<QhyccdCamera>>,
    pub filter_wheel: Option<Arc<QhyccdFilterWheel>>,
    pub camera_config: Option<CameraConfig>,
    pub filter_wheel_config: Option<FilterWheelConfig>,
    pub last_error: Option<String>,
    pub last_error_code: Option<String>,
}

impl QhyCameraWorld {
    /// Build a camera with the default mock handle
    pub fn build_camera_with_mock(&mut self) {
        let config = self
            .camera_config
            .clone()
            .unwrap_or_else(default_camera_config);
        let handle = Box::new(TestCameraHandle::default());
        let camera = QhyccdCamera::new(config, handle);
        self.camera = Some(Arc::new(camera));
    }

    /// Build a camera with a custom handle
    pub fn build_camera_with_handle(&mut self, handle: Box<dyn CameraHandle>) {
        let config = self
            .camera_config
            .clone()
            .unwrap_or_else(default_camera_config);
        let camera = QhyccdCamera::new(config, handle);
        self.camera = Some(Arc::new(camera));
    }

    /// Build a filter wheel with the default mock handle
    pub fn build_filter_wheel_with_mock(&mut self) {
        let config = self
            .filter_wheel_config
            .clone()
            .unwrap_or_else(default_filter_wheel_config);
        let handle = Box::new(TestFilterWheelHandle::default());
        let fw = QhyccdFilterWheel::new(config, handle);
        self.filter_wheel = Some(Arc::new(fw));
    }

    /// Build a filter wheel with a custom handle
    pub fn build_filter_wheel_with_handle(&mut self, handle: Box<dyn FilterWheelHandle>) {
        let config = self
            .filter_wheel_config
            .clone()
            .unwrap_or_else(default_filter_wheel_config);
        let fw = QhyccdFilterWheel::new(config, handle);
        self.filter_wheel = Some(Arc::new(fw));
    }
}

pub fn default_camera_config() -> CameraConfig {
    CameraConfig {
        unique_id: "QHY600M-test001".to_string(),
        name: "Test QHY Camera".to_string(),
        description: "Test QHYCCD camera".to_string(),
        device_number: 0,
        enabled: true,
    }
}

pub fn default_filter_wheel_config() -> FilterWheelConfig {
    FilterWheelConfig {
        unique_id: "CFW=QHY600M-test001".to_string(),
        name: "Test Filter Wheel".to_string(),
        description: "Test QHYCCD filter wheel".to_string(),
        device_number: 0,
        enabled: true,
        filter_names: vec![],
    }
}

// --- Test camera handle ---

use std::sync::Mutex;

/// In-test camera handle with controllable behavior
pub struct TestCameraHandle {
    state: Mutex<TestCameraState>,
}

#[derive(Debug)]
struct TestCameraState {
    is_open: bool,
    initialized: bool,
    binning: u8,
    roi: Option<CcdChipArea>,
    gain: f64,
    offset: f64,
    speed: f64,
    exposure_us: f64,
    cooler_pwm: f64,
    target_temp: f64,
    current_temp: f64,
    readout_mode: u32,
    fail_open: bool,
}

impl Default for TestCameraHandle {
    fn default() -> Self {
        Self {
            state: Mutex::new(TestCameraState {
                is_open: false,
                initialized: false,
                binning: 1,
                roi: None,
                gain: 10.0,
                offset: 50.0,
                speed: 0.0,
                exposure_us: 1_000_000.0,
                cooler_pwm: 0.0,
                target_temp: 0.0,
                current_temp: 20.0,
                readout_mode: 0,
                fail_open: false,
            }),
        }
    }
}

impl TestCameraHandle {
    pub fn failing() -> Self {
        Self {
            state: Mutex::new(TestCameraState {
                fail_open: true,
                ..TestCameraState::default()
            }),
        }
    }
}

impl Default for TestCameraState {
    fn default() -> Self {
        Self {
            is_open: false,
            initialized: false,
            binning: 1,
            roi: None,
            gain: 10.0,
            offset: 50.0,
            speed: 0.0,
            exposure_us: 1_000_000.0,
            cooler_pwm: 0.0,
            target_temp: 0.0,
            current_temp: 20.0,
            readout_mode: 0,
            fail_open: false,
        }
    }
}

impl CameraHandle for TestCameraHandle {
    fn open(&self) -> qhy_camera::Result<()> {
        let mut state = self.state.lock().unwrap();
        if state.fail_open {
            return Err(qhy_camera::QhyCameraError::SdkError(
                "mock open failure".to_string(),
            ));
        }
        state.is_open = true;
        Ok(())
    }

    fn close(&self) -> qhy_camera::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.is_open = false;
        state.initialized = false;
        Ok(())
    }

    fn is_open(&self) -> qhy_camera::Result<bool> {
        Ok(self.state.lock().unwrap().is_open)
    }

    fn init(&self) -> qhy_camera::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.initialized = true;
        state.roi = Some(CcdChipArea {
            start_x: 0,
            start_y: 0,
            width: 4656,
            height: 3520,
        });
        Ok(())
    }

    fn get_ccd_info(&self) -> qhy_camera::Result<CcdChipInfo> {
        Ok(CcdChipInfo {
            image_width: 4656,
            image_height: 3520,
            pixel_width: 3.76,
            pixel_height: 3.76,
            bits_per_pixel: 16,
        })
    }

    fn set_stream_mode(&self, _mode: StreamMode) -> qhy_camera::Result<()> {
        Ok(())
    }

    fn set_readout_mode(&self, mode: u32) -> qhy_camera::Result<()> {
        self.state.lock().unwrap().readout_mode = mode;
        Ok(())
    }

    fn get_readout_mode(&self) -> qhy_camera::Result<u32> {
        Ok(self.state.lock().unwrap().readout_mode)
    }

    fn get_number_of_readout_modes(&self) -> qhy_camera::Result<u32> {
        Ok(2)
    }

    fn get_readout_mode_name(&self, index: u32) -> qhy_camera::Result<String> {
        match index {
            0 => Ok("Standard".to_string()),
            1 => Ok("High Gain".to_string()),
            _ => Err(qhy_camera::QhyCameraError::InvalidValue(format!(
                "mode {} out of range",
                index
            ))),
        }
    }

    fn get_readout_mode_resolution(&self, _index: u32) -> qhy_camera::Result<(u32, u32)> {
        Ok((4656, 3520))
    }

    fn set_bin_mode(&self, bin_x: u32, _bin_y: u32) -> qhy_camera::Result<()> {
        self.state.lock().unwrap().binning = bin_x as u8;
        Ok(())
    }

    fn set_roi(&self, roi: CcdChipArea) -> qhy_camera::Result<()> {
        self.state.lock().unwrap().roi = Some(roi);
        Ok(())
    }

    fn get_effective_area(&self) -> qhy_camera::Result<CcdChipArea> {
        Ok(CcdChipArea {
            start_x: 0,
            start_y: 0,
            width: 4656,
            height: 3520,
        })
    }

    fn is_control_available(&self, control: Control) -> Option<f64> {
        match control {
            Control::CamBin1x1mode => Some(1.0),
            Control::CamBin2x2mode => Some(1.0),
            Control::CamBin3x3mode => Some(1.0),
            Control::CamBin4x4mode => Some(1.0),
            Control::CamSingleFrameMode => Some(1.0),
            Control::Gain => Some(1.0),
            Control::Offset => Some(1.0),
            Control::Speed => Some(1.0),
            Control::Cooler => Some(1.0),
            Control::OutputDataActualBits => Some(16.0),
            Control::CamIsColor => None,
            Control::CamColor => None,
            Control::CamMechanicalShutter => None,
            _ => None,
        }
    }

    fn get_parameter(&self, control: Control) -> qhy_camera::Result<f64> {
        let state = self.state.lock().unwrap();
        match control {
            Control::Gain => Ok(state.gain),
            Control::Offset => Ok(state.offset),
            Control::Speed => Ok(state.speed),
            Control::Exposure => Ok(state.exposure_us),
            Control::CurTemp => Ok(state.current_temp),
            Control::CurPWM => Ok(state.cooler_pwm),
            Control::OutputDataActualBits => Ok(16.0),
            _ => Err(qhy_camera::QhyCameraError::ControlNotAvailable(format!(
                "{:?}",
                control
            ))),
        }
    }

    fn set_parameter(&self, control: Control, value: f64) -> qhy_camera::Result<()> {
        let mut state = self.state.lock().unwrap();
        match control {
            Control::Gain => state.gain = value,
            Control::Offset => state.offset = value,
            Control::Speed => state.speed = value,
            Control::Exposure => state.exposure_us = value,
            Control::Cooler => state.target_temp = value,
            Control::ManualPWM => state.cooler_pwm = value,
            Control::TransferBit => {}
            _ => {
                return Err(qhy_camera::QhyCameraError::ControlNotAvailable(format!(
                    "{:?}",
                    control
                )));
            }
        }
        Ok(())
    }

    fn set_if_available(&self, control: Control, value: f64) -> qhy_camera::Result<()> {
        if self.is_control_available(control).is_some() {
            self.set_parameter(control, value)
        } else {
            Ok(())
        }
    }

    fn get_parameter_min_max_step(&self, control: Control) -> qhy_camera::Result<(f64, f64, f64)> {
        match control {
            Control::Gain => Ok((0.0, 100.0, 1.0)),
            Control::Offset => Ok((0.0, 200.0, 1.0)),
            Control::Speed => Ok((0.0, 2.0, 1.0)),
            Control::Exposure => Ok((100.0, 3_600_000_000.0, 1.0)),
            _ => Err(qhy_camera::QhyCameraError::ControlNotAvailable(format!(
                "{:?}",
                control
            ))),
        }
    }

    fn start_single_frame_exposure(&self) -> qhy_camera::Result<()> {
        Ok(())
    }

    fn get_image_size(&self) -> qhy_camera::Result<usize> {
        let state = self.state.lock().unwrap();
        let roi = state
            .roi
            .ok_or_else(|| qhy_camera::QhyCameraError::SdkError("no ROI set".to_string()))?;
        Ok(roi.width as usize * roi.height as usize * 2)
    }

    fn get_single_frame(&self, buffer_size: usize) -> qhy_camera::Result<ImageData> {
        let state = self.state.lock().unwrap();
        let roi = state
            .roi
            .ok_or_else(|| qhy_camera::QhyCameraError::SdkError("no ROI set".to_string()))?;
        Ok(ImageData {
            data: vec![128_u8; buffer_size],
            width: roi.width,
            height: roi.height,
            bits_per_pixel: 16,
            channels: 1,
        })
    }

    fn abort_exposure_and_readout(&self) -> qhy_camera::Result<()> {
        Ok(())
    }

    fn get_remaining_exposure_us(&self) -> qhy_camera::Result<u32> {
        Ok(0)
    }

    fn id(&self) -> &str {
        "QHY600M-test001"
    }

    fn clone_handle(&self) -> Box<dyn CameraHandle> {
        let state = self.state.lock().unwrap();
        let new_state = TestCameraState {
            is_open: state.is_open,
            initialized: state.initialized,
            binning: state.binning,
            roi: state.roi,
            gain: state.gain,
            offset: state.offset,
            speed: state.speed,
            exposure_us: state.exposure_us,
            cooler_pwm: state.cooler_pwm,
            target_temp: state.target_temp,
            current_temp: state.current_temp,
            readout_mode: state.readout_mode,
            fail_open: false,
        };
        Box::new(TestCameraHandle {
            state: Mutex::new(new_state),
        })
    }
}

// --- Test filter wheel handle ---

pub struct TestFilterWheelHandle {
    state: Mutex<TestFilterWheelState>,
}

#[derive(Debug)]
struct TestFilterWheelState {
    is_open: bool,
    position: u32,
    number_of_filters: u32,
    fail_open: bool,
}

impl Default for TestFilterWheelHandle {
    fn default() -> Self {
        Self {
            state: Mutex::new(TestFilterWheelState {
                is_open: false,
                position: 0,
                number_of_filters: 7,
                fail_open: false,
            }),
        }
    }
}

impl TestFilterWheelHandle {
    pub fn failing() -> Self {
        Self {
            state: Mutex::new(TestFilterWheelState {
                is_open: false,
                position: 0,
                number_of_filters: 7,
                fail_open: true,
            }),
        }
    }
}

impl FilterWheelHandle for TestFilterWheelHandle {
    fn open(&self) -> qhy_camera::Result<()> {
        let mut state = self.state.lock().unwrap();
        if state.fail_open {
            return Err(qhy_camera::QhyCameraError::SdkError(
                "mock open failure".to_string(),
            ));
        }
        state.is_open = true;
        Ok(())
    }

    fn close(&self) -> qhy_camera::Result<()> {
        self.state.lock().unwrap().is_open = false;
        Ok(())
    }

    fn is_open(&self) -> qhy_camera::Result<bool> {
        Ok(self.state.lock().unwrap().is_open)
    }

    fn get_number_of_filters(&self) -> qhy_camera::Result<u32> {
        Ok(self.state.lock().unwrap().number_of_filters)
    }

    fn get_fw_position(&self) -> qhy_camera::Result<u32> {
        Ok(self.state.lock().unwrap().position)
    }

    fn set_fw_position(&self, position: u32) -> qhy_camera::Result<()> {
        self.state.lock().unwrap().position = position;
        Ok(())
    }

    fn id(&self) -> &str {
        "CFW=QHY600M-test001"
    }
}
