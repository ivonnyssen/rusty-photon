//! SDK trait abstractions for QHYCCD cameras and filter wheels
//!
//! This module provides trait abstractions for the QHYCCD SDK operations.
//! These traits enable mockall-based testing without requiring actual hardware.

use async_trait::async_trait;

use crate::error::Result;

// --- Local types mirroring qhyccd-rs ---

/// CCD chip information (mirrors `qhyccd_rs::CCDChipInfo`)
#[derive(Debug, Clone, Copy)]
pub struct CcdChipInfo {
    pub image_width: u32,
    pub image_height: u32,
    pub pixel_width: f64,
    pub pixel_height: f64,
    pub bits_per_pixel: u32,
}

/// CCD chip area / ROI (mirrors `qhyccd_rs::CCDChipArea`)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CcdChipArea {
    pub start_x: u32,
    pub start_y: u32,
    pub width: u32,
    pub height: u32,
}

/// Raw image data from the camera (mirrors `qhyccd_rs::ImageData`)
#[derive(Debug, Clone)]
pub struct ImageData {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub bits_per_pixel: u32,
    pub channels: u32,
}

/// Camera control identifiers (mirrors `qhyccd_rs::Control`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Control {
    Gain,
    Offset,
    Exposure,
    Speed,
    TransferBit,
    Cooler,
    CurTemp,
    CurPWM,
    ManualPWM,
    CamBin1x1mode,
    CamBin2x2mode,
    CamBin3x3mode,
    CamBin4x4mode,
    CamBin6x6mode,
    CamBin8x8mode,
    CamIsColor,
    CamColor,
    CamMechanicalShutter,
    CamSingleFrameMode,
    OutputDataActualBits,
}

/// Stream mode (mirrors `qhyccd_rs::StreamMode`)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamMode {
    SingleFrameMode,
}

/// Bayer pattern mode (mirrors `qhyccd_rs::BayerMode`)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BayerMode {
    GBRG,
    GRBG,
    BGGR,
    RGGB,
}

impl TryFrom<f64> for BayerMode {
    type Error = String;

    fn try_from(value: f64) -> std::result::Result<Self, Self::Error> {
        match value as u32 {
            1 => Ok(BayerMode::GBRG),
            2 => Ok(BayerMode::GRBG),
            3 => Ok(BayerMode::BGGR),
            4 => Ok(BayerMode::RGGB),
            other => Err(format!("invalid bayer mode: {}", other)),
        }
    }
}

// --- SDK traits ---

/// Trait for enumerating QHYCCD devices
#[async_trait]
#[cfg_attr(test, mockall::automock)]
pub trait SdkProvider: Send + Sync {
    /// List available camera IDs
    fn camera_ids(&self) -> Result<Vec<String>>;

    /// List available filter wheel IDs
    fn filter_wheel_ids(&self) -> Result<Vec<String>>;

    /// Create a camera handle for the given ID
    fn open_camera(&self, id: &str) -> Result<Box<dyn CameraHandle>>;

    /// Create a filter wheel handle for the given ID
    fn open_filter_wheel(&self, id: &str) -> Result<Box<dyn FilterWheelHandle>>;
}

/// Trait abstracting QHYCCD Camera SDK operations
#[cfg_attr(test, mockall::automock)]
pub trait CameraHandle: Send + Sync {
    /// Open the camera device
    fn open(&self) -> Result<()>;

    /// Close the camera device
    fn close(&self) -> Result<()>;

    /// Check if the camera is open
    fn is_open(&self) -> Result<bool>;

    /// Initialize the camera after setting stream/readout mode
    fn init(&self) -> Result<()>;

    /// Get CCD chip information
    fn get_ccd_info(&self) -> Result<CcdChipInfo>;

    /// Set stream mode
    fn set_stream_mode(&self, mode: StreamMode) -> Result<()>;

    /// Set readout mode by index
    fn set_readout_mode(&self, mode: u32) -> Result<()>;

    /// Get current readout mode index
    fn get_readout_mode(&self) -> Result<u32>;

    /// Get number of available readout modes
    fn get_number_of_readout_modes(&self) -> Result<u32>;

    /// Get the name of a readout mode by index
    fn get_readout_mode_name(&self, index: u32) -> Result<String>;

    /// Get the resolution for a readout mode by index
    fn get_readout_mode_resolution(&self, index: u32) -> Result<(u32, u32)>;

    /// Set binning mode (width, height)
    fn set_bin_mode(&self, bin_x: u32, bin_y: u32) -> Result<()>;

    /// Set the region of interest
    fn set_roi(&self, roi: CcdChipArea) -> Result<()>;

    /// Get the effective (usable) imaging area
    fn get_effective_area(&self) -> Result<CcdChipArea>;

    /// Check if a control is available. Returns `Some(value)` if available.
    fn is_control_available(&self, control: Control) -> Option<f64>;

    /// Get a control parameter value
    fn get_parameter(&self, control: Control) -> Result<f64>;

    /// Set a control parameter value
    fn set_parameter(&self, control: Control, value: f64) -> Result<()>;

    /// Set a control parameter if it is available (no error if not)
    fn set_if_available(&self, control: Control, value: f64) -> Result<()>;

    /// Get min, max, step for a control parameter
    fn get_parameter_min_max_step(&self, control: Control) -> Result<(f64, f64, f64)>;

    /// Start a single-frame exposure (blocking)
    fn start_single_frame_exposure(&self) -> Result<()>;

    /// Get the required buffer size for the image
    fn get_image_size(&self) -> Result<usize>;

    /// Get the captured single frame (blocking)
    fn get_single_frame(&self, buffer_size: usize) -> Result<ImageData>;

    /// Abort the current exposure and readout
    fn abort_exposure_and_readout(&self) -> Result<()>;

    /// Get remaining exposure time in microseconds
    fn get_remaining_exposure_us(&self) -> Result<u32>;

    /// Get the camera ID string
    fn id(&self) -> &str;

    /// Clone the handle (for passing to spawned tasks)
    fn clone_handle(&self) -> Box<dyn CameraHandle>;
}

/// Trait abstracting QHYCCD FilterWheel SDK operations
#[cfg_attr(test, mockall::automock)]
pub trait FilterWheelHandle: Send + Sync {
    /// Open the filter wheel device
    fn open(&self) -> Result<()>;

    /// Close the filter wheel device
    fn close(&self) -> Result<()>;

    /// Check if the filter wheel is open
    fn is_open(&self) -> Result<bool>;

    /// Get the number of filter positions
    fn get_number_of_filters(&self) -> Result<u32>;

    /// Get current filter wheel position
    fn get_fw_position(&self) -> Result<u32>;

    /// Set filter wheel position
    fn set_fw_position(&self, position: u32) -> Result<()>;

    /// Get the filter wheel ID string
    fn id(&self) -> &str;
}
