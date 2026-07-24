use crate::Result;

use crate::{CCDChipArea, StreamMode};

#[cfg(not(feature = "simulation"))]
use crate::backend::read_lock;
#[cfg(not(feature = "simulation"))]
use crate::check;
#[cfg(not(feature = "simulation"))]
use crate::sys::{
    SetQHYCCDBinMode, SetQHYCCDBitsMode, SetQHYCCDDebayerOnOff, SetQHYCCDReadMode,
    SetQHYCCDResolution, SetQHYCCDStreamMode,
};

// `QHYError` is only constructed on the simulation arms here (the real arms funnel
// their success/fail return through `check`), so keep it `simulation`-gated to
// avoid an unused import on the real build.
#[cfg(feature = "simulation")]
use crate::QHYError;

use super::Camera;

impl Camera {
    /// Sets the stream mode of the camera
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,Camera,StreamMode};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// camera.open().expect("open failed");
    /// camera.set_stream_mode(StreamMode::LiveMode).expect("set_stream_mode failed");
    /// ```
    pub fn set_stream_mode(&self, mode: StreamMode) -> Result<()> {
        #[cfg(not(feature = "simulation"))]
        {
            let handle = read_lock!(self.handle)?;
            check(
                unsafe { SetQHYCCDStreamMode(handle, mode as u8) },
                "set_stream_mode",
            )
        }
        #[cfg(feature = "simulation")]
        {
            let mut state = self.state.write();
            if !state.is_open {
                return Err(QHYError::CameraNotOpen);
            }
            state.stream_mode = Some(mode);
            Ok(())
        }
    }

    /// Sets the readout mode of the camera with the id of the `ReadoutMode` between 0 and the value
    /// returned by `get_number_of_readout_modes`
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,Camera};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// camera.open().expect("open failed");
    /// camera.set_readout_mode(0).expect("set_readout_mode failed");
    /// ```
    pub fn set_readout_mode(&self, mode: u32) -> Result<()> {
        #[cfg(not(feature = "simulation"))]
        {
            let handle = read_lock!(self.handle)?;
            check(
                unsafe { SetQHYCCDReadMode(handle, mode) },
                "set_readout_mode",
            )
        }
        #[cfg(feature = "simulation")]
        {
            let mut state = self.state.write();
            if !state.is_open {
                return Err(QHYError::CameraNotOpen);
            }
            if mode as usize >= state.config.readout_modes.len() {
                return Err(QHYError::Sdk {
                    op: "set_readout_mode",
                });
            }
            state.readout_mode = mode;
            Ok(())
        }
    }

    /// Sets the binning mode of the camera
    /// Only symmetric binnings are supported
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,Camera};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// camera.open().expect("open failed");
    /// camera.set_bin_mode(1, 1).expect("set_bin_mode failed");
    /// ```
    pub fn set_bin_mode(&self, bin_x: u32, bin_y: u32) -> Result<()> {
        #[cfg(not(feature = "simulation"))]
        {
            let handle = read_lock!(self.handle)?;
            check(
                unsafe { SetQHYCCDBinMode(handle, bin_x, bin_y) },
                "set_bin_mode",
            )
        }
        #[cfg(feature = "simulation")]
        {
            let mut state = self.state.write();
            if !state.is_open {
                return Err(QHYError::CameraNotOpen);
            }
            state.binning = (bin_x, bin_y);
            Ok(())
        }
    }

    /// According to c-cod ethis does not work for all cameras
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,Camera};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// camera.open().expect("open failed");
    /// camera.set_debayer(true).expect("set_debayer failed");
    /// ```
    pub fn set_debayer(&self, on: bool) -> Result<()> {
        #[cfg(not(feature = "simulation"))]
        {
            let handle = read_lock!(self.handle)?;
            check(unsafe { SetQHYCCDDebayerOnOff(handle, on) }, "set_debayer")
        }
        #[cfg(feature = "simulation")]
        {
            let mut state = self.state.write();
            if !state.is_open {
                return Err(QHYError::CameraNotOpen);
            }
            state.debayer_enabled = on;
            Ok(())
        }
    }

    /// Sets the Region of interest of the camera
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,Camera, CCDChipArea};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// camera.open().expect("open failed");
    /// let roi = CCDChipArea {
    ///     start_x: 0,
    ///     start_y: 0,
    ///     width: 640,
    ///     height: 480,
    /// };
    /// camera.set_roi(roi).expect("set_roi failed");
    /// ```
    pub fn set_roi(&self, roi: CCDChipArea) -> Result<()> {
        #[cfg(not(feature = "simulation"))]
        {
            let handle = read_lock!(self.handle)?;
            check(
                unsafe {
                    SetQHYCCDResolution(handle, roi.start_x, roi.start_y, roi.width, roi.height)
                },
                "set_roi",
            )
        }
        #[cfg(feature = "simulation")]
        {
            let mut state = self.state.write();
            if !state.is_open {
                return Err(QHYError::CameraNotOpen);
            }
            state.roi = roi;
            Ok(())
        }
    }

    /// Sets the USB transfer mode to either 8 or 16 bit
    ///
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,Camera};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// camera.open().expect("open failed");
    /// camera.set_bit_mode(16).expect("set_bit_mode failed");
    /// ```
    pub fn set_bit_mode(&self, mode: u32) -> Result<()> {
        #[cfg(not(feature = "simulation"))]
        {
            let handle = read_lock!(self.handle)?;
            check(unsafe { SetQHYCCDBitsMode(handle, mode) }, "set_bit_mode")
        }
        #[cfg(feature = "simulation")]
        {
            let mut state = self.state.write();
            if !state.is_open {
                return Err(QHYError::CameraNotOpen);
            }
            state.bit_depth = mode;
            Ok(())
        }
    }
}
