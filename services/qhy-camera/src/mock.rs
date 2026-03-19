//! Mock SDK implementation for testing
//!
//! This module provides mock implementations of the SDK traits that simulate
//! QHYCCD camera and filter wheel behavior, allowing the driver to be tested
//! without real hardware.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::debug;

use crate::error::{QhyCameraError, Result};
use crate::io::{
    CameraHandle, CcdChipArea, CcdChipInfo, Control, FilterWheelHandle, ImageData, SdkProvider,
    StreamMode,
};

/// Simulated camera device state
#[derive(Debug, Clone)]
struct MockCameraState {
    is_open: bool,
    initialized: bool,
    binning: u8,
    roi: Option<CcdChipArea>,
    gain: f64,
    offset: f64,
    speed: f64,
    exposure_us: f64,
    cooler_on: bool,
    cooler_pwm: f64,
    target_temp: f64,
    current_temp: f64,
    readout_mode: u32,
}

impl Default for MockCameraState {
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
            cooler_on: false,
            cooler_pwm: 0.0,
            target_temp: 0.0,
            current_temp: 20.0,
            readout_mode: 0,
        }
    }
}

/// Mock camera handle for testing
pub struct MockCameraHandle {
    id: String,
    state: Arc<Mutex<MockCameraState>>,
}

impl MockCameraHandle {
    fn new(id: String) -> Self {
        Self {
            id,
            state: Arc::new(Mutex::new(MockCameraState::default())),
        }
    }
}

impl CameraHandle for MockCameraHandle {
    fn open(&self) -> Result<()> {
        let mut state = self.state.blocking_lock();
        state.is_open = true;
        debug!("Mock camera opened: {}", self.id);
        Ok(())
    }

    fn close(&self) -> Result<()> {
        let mut state = self.state.blocking_lock();
        state.is_open = false;
        state.initialized = false;
        debug!("Mock camera closed: {}", self.id);
        Ok(())
    }

    fn is_open(&self) -> Result<bool> {
        Ok(self.state.blocking_lock().is_open)
    }

    fn init(&self) -> Result<()> {
        let mut state = self.state.blocking_lock();
        if !state.is_open {
            return Err(QhyCameraError::NotConnected);
        }
        state.initialized = true;
        state.roi = Some(CcdChipArea {
            start_x: 0,
            start_y: 0,
            width: 4656,
            height: 3520,
        });
        debug!("Mock camera initialized");
        Ok(())
    }

    fn get_ccd_info(&self) -> Result<CcdChipInfo> {
        Ok(CcdChipInfo {
            image_width: 4656,
            image_height: 3520,
            pixel_width: 3.76,
            pixel_height: 3.76,
            bits_per_pixel: 16,
        })
    }

    fn set_stream_mode(&self, _mode: StreamMode) -> Result<()> {
        Ok(())
    }

    fn set_readout_mode(&self, mode: u32) -> Result<()> {
        self.state.blocking_lock().readout_mode = mode;
        Ok(())
    }

    fn get_readout_mode(&self) -> Result<u32> {
        Ok(self.state.blocking_lock().readout_mode)
    }

    fn get_number_of_readout_modes(&self) -> Result<u32> {
        Ok(2)
    }

    fn get_readout_mode_name(&self, index: u32) -> Result<String> {
        match index {
            0 => Ok("Standard".to_string()),
            1 => Ok("High Gain".to_string()),
            _ => Err(QhyCameraError::InvalidValue(format!(
                "readout mode {} out of range",
                index
            ))),
        }
    }

    fn get_readout_mode_resolution(&self, _index: u32) -> Result<(u32, u32)> {
        Ok((4656, 3520))
    }

    fn set_bin_mode(&self, bin_x: u32, _bin_y: u32) -> Result<()> {
        self.state.blocking_lock().binning = bin_x as u8;
        Ok(())
    }

    fn set_roi(&self, roi: CcdChipArea) -> Result<()> {
        self.state.blocking_lock().roi = Some(roi);
        Ok(())
    }

    fn get_effective_area(&self) -> Result<CcdChipArea> {
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
            Control::CamIsColor => None, // monochrome by default
            Control::CamColor => None,
            Control::CamMechanicalShutter => None,
            _ => None,
        }
    }

    fn get_parameter(&self, control: Control) -> Result<f64> {
        let state = self.state.blocking_lock();
        match control {
            Control::Gain => Ok(state.gain),
            Control::Offset => Ok(state.offset),
            Control::Speed => Ok(state.speed),
            Control::Exposure => Ok(state.exposure_us),
            Control::CurTemp => Ok(state.current_temp),
            Control::CurPWM => Ok(state.cooler_pwm),
            Control::OutputDataActualBits => Ok(16.0),
            _ => Err(QhyCameraError::ControlNotAvailable(format!(
                "{:?}",
                control
            ))),
        }
    }

    fn set_parameter(&self, control: Control, value: f64) -> Result<()> {
        let mut state = self.state.blocking_lock();
        match control {
            Control::Gain => state.gain = value,
            Control::Offset => state.offset = value,
            Control::Speed => state.speed = value,
            Control::Exposure => state.exposure_us = value,
            Control::Cooler => state.target_temp = value,
            Control::ManualPWM => {
                state.cooler_pwm = value;
                state.cooler_on = value > 0.0;
            }
            Control::TransferBit => {}
            _ => {
                return Err(QhyCameraError::ControlNotAvailable(format!(
                    "{:?}",
                    control
                )));
            }
        }
        Ok(())
    }

    fn set_if_available(&self, control: Control, value: f64) -> Result<()> {
        if self.is_control_available(control).is_some() {
            self.set_parameter(control, value)
        } else {
            Ok(())
        }
    }

    fn get_parameter_min_max_step(&self, control: Control) -> Result<(f64, f64, f64)> {
        match control {
            Control::Gain => Ok((0.0, 100.0, 1.0)),
            Control::Offset => Ok((0.0, 200.0, 1.0)),
            Control::Speed => Ok((0.0, 2.0, 1.0)),
            Control::Exposure => Ok((100.0, 3_600_000_000.0, 1.0)), // 100us to 3600s
            _ => Err(QhyCameraError::ControlNotAvailable(format!(
                "{:?}",
                control
            ))),
        }
    }

    fn start_single_frame_exposure(&self) -> Result<()> {
        debug!("Mock: starting single frame exposure");
        Ok(())
    }

    fn get_image_size(&self) -> Result<usize> {
        let state = self.state.blocking_lock();
        let roi = state
            .roi
            .ok_or_else(|| QhyCameraError::SdkError("no ROI set".to_string()))?;
        // 16-bit pixels
        Ok(roi.width as usize * roi.height as usize * 2)
    }

    fn get_single_frame(&self, buffer_size: usize) -> Result<ImageData> {
        let state = self.state.blocking_lock();
        let roi = state
            .roi
            .ok_or_else(|| QhyCameraError::SdkError("no ROI set".to_string()))?;
        debug!(
            "Mock: returning {}x{} 16-bit image ({} bytes)",
            roi.width, roi.height, buffer_size
        );
        Ok(ImageData {
            data: vec![128_u8; buffer_size],
            width: roi.width,
            height: roi.height,
            bits_per_pixel: 16,
            channels: 1,
        })
    }

    fn abort_exposure_and_readout(&self) -> Result<()> {
        debug!("Mock: aborting exposure");
        Ok(())
    }

    fn get_remaining_exposure_us(&self) -> Result<u32> {
        Ok(0)
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn clone_handle(&self) -> Box<dyn CameraHandle> {
        Box::new(MockCameraHandle {
            id: self.id.clone(),
            state: Arc::clone(&self.state),
        })
    }
}

/// Simulated filter wheel device state
#[derive(Debug, Clone)]
struct MockFilterWheelState {
    is_open: bool,
    position: u32,
    number_of_filters: u32,
}

impl Default for MockFilterWheelState {
    fn default() -> Self {
        Self {
            is_open: false,
            position: 0,
            number_of_filters: 7,
        }
    }
}

/// Mock filter wheel handle for testing
pub struct MockFilterWheelHandle {
    id: String,
    state: Arc<Mutex<MockFilterWheelState>>,
}

impl MockFilterWheelHandle {
    fn new(id: String) -> Self {
        Self {
            id,
            state: Arc::new(Mutex::new(MockFilterWheelState::default())),
        }
    }
}

impl FilterWheelHandle for MockFilterWheelHandle {
    fn open(&self) -> Result<()> {
        self.state.blocking_lock().is_open = true;
        debug!("Mock filter wheel opened: {}", self.id);
        Ok(())
    }

    fn close(&self) -> Result<()> {
        self.state.blocking_lock().is_open = false;
        debug!("Mock filter wheel closed: {}", self.id);
        Ok(())
    }

    fn is_open(&self) -> Result<bool> {
        Ok(self.state.blocking_lock().is_open)
    }

    fn get_number_of_filters(&self) -> Result<u32> {
        Ok(self.state.blocking_lock().number_of_filters)
    }

    fn get_fw_position(&self) -> Result<u32> {
        Ok(self.state.blocking_lock().position)
    }

    fn set_fw_position(&self, position: u32) -> Result<()> {
        self.state.blocking_lock().position = position;
        debug!("Mock filter wheel moved to position {}", position);
        Ok(())
    }

    fn id(&self) -> &str {
        &self.id
    }
}

/// Mock SDK provider for testing
pub struct MockSdkProvider;

#[async_trait]
impl SdkProvider for MockSdkProvider {
    fn camera_ids(&self) -> Result<Vec<String>> {
        Ok(vec!["QHY600M-mock001".to_string()])
    }

    fn filter_wheel_ids(&self) -> Result<Vec<String>> {
        Ok(vec!["CFW=QHY600M-mock001".to_string()])
    }

    fn open_camera(&self, id: &str) -> Result<Box<dyn CameraHandle>> {
        Ok(Box::new(MockCameraHandle::new(id.to_string())))
    }

    fn open_filter_wheel(&self, id: &str) -> Result<Box<dyn FilterWheelHandle>> {
        Ok(Box::new(MockFilterWheelHandle::new(id.to_string())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_camera_lifecycle() {
        let handle = MockCameraHandle::new("test-cam".to_string());
        assert!(!handle.is_open().unwrap());
        handle.open().unwrap();
        assert!(handle.is_open().unwrap());
        handle.close().unwrap();
        assert!(!handle.is_open().unwrap());
    }

    #[test]
    fn test_mock_camera_ccd_info() {
        let handle = MockCameraHandle::new("test-cam".to_string());
        let info = handle.get_ccd_info().unwrap();
        assert_eq!(info.image_width, 4656);
        assert_eq!(info.image_height, 3520);
    }

    #[test]
    fn test_mock_camera_controls() {
        let handle = MockCameraHandle::new("test-cam".to_string());
        assert!(handle.is_control_available(Control::Gain).is_some());
        assert!(handle.is_control_available(Control::CamIsColor).is_none());
    }

    #[test]
    fn test_mock_camera_gain() {
        let handle = MockCameraHandle::new("test-cam".to_string());
        let (min, max, _step) = handle.get_parameter_min_max_step(Control::Gain).unwrap();
        assert_eq!(min, 0.0);
        assert_eq!(max, 100.0);
        handle.set_parameter(Control::Gain, 50.0).unwrap();
        assert_eq!(handle.get_parameter(Control::Gain).unwrap(), 50.0);
    }

    #[test]
    fn test_mock_filter_wheel_lifecycle() {
        let handle = MockFilterWheelHandle::new("test-fw".to_string());
        assert!(!handle.is_open().unwrap());
        handle.open().unwrap();
        assert!(handle.is_open().unwrap());
        assert_eq!(handle.get_number_of_filters().unwrap(), 7);
        assert_eq!(handle.get_fw_position().unwrap(), 0);
        handle.set_fw_position(3).unwrap();
        assert_eq!(handle.get_fw_position().unwrap(), 3);
        handle.close().unwrap();
        assert!(!handle.is_open().unwrap());
    }

    #[test]
    fn test_mock_sdk_provider() {
        let provider = MockSdkProvider;
        let cameras = provider.camera_ids().unwrap();
        assert_eq!(cameras.len(), 1);
        let fws = provider.filter_wheel_ids().unwrap();
        assert_eq!(fws.len(), 1);
    }
}
