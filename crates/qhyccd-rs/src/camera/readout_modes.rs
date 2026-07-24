#[cfg(not(feature = "simulation"))]
use std::ffi::{c_char, CStr};

use crate::Result;

use crate::QHYError;

#[cfg(not(feature = "simulation"))]
use crate::backend::read_lock;
#[cfg(not(feature = "simulation"))]
use crate::sys::{
    GetQHYCCDNumberOfReadModes, GetQHYCCDReadMode, GetQHYCCDReadModeName,
    GetQHYCCDReadModeResolution, QHYCCD_ERROR, QHYCCD_SUCCESS,
};

use super::Camera;

impl Camera {
    /// Returns the number of readout modes of the camera
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,Camera};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// camera.open().expect("open failed");
    /// let number_of_readout_modes = camera.get_number_of_readout_modes().expect("get_number_of_readout_modes failed");
    /// println!("Number of readout modes: {}", number_of_readout_modes);
    /// ```
    pub fn get_number_of_readout_modes(&self) -> Result<u32> {
        #[cfg(not(feature = "simulation"))]
        {
            let handle = read_lock!(self.handle)?;

            let mut num: u32 = 0;
            match unsafe { GetQHYCCDNumberOfReadModes(handle, &mut num as *mut u32) } {
                QHYCCD_ERROR => {
                    let error = QHYError::Sdk {
                        op: "get_number_of_readout_modes",
                    };
                    tracing::error!(error = ?error);
                    Err(error)
                }
                _ => Ok(num),
            }
        }
        #[cfg(feature = "simulation")]
        {
            let state = self.state.read();
            if !state.is_open {
                return Err(QHYError::CameraNotOpen);
            }
            Ok(state.config.readout_modes.len() as u32)
        }
    }

    /// Returns the readout mode name with the given index. Make sure to check the number of readout modes.
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,Camera};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// camera.open().expect("open failed");
    /// let readout_mode_name = camera.get_readout_mode_name(0).expect("get_readout_mode_name failed");
    /// println!("Readout mode name: {}", readout_mode_name);
    /// ```
    pub fn get_readout_mode_name(&self, index: u32) -> Result<String> {
        #[cfg(not(feature = "simulation"))]
        {
            let handle = read_lock!(self.handle)?;
            let mut name: [c_char; 80] = [0; 80];
            match unsafe { GetQHYCCDReadModeName(handle, index, name.as_mut_ptr()) } {
                QHYCCD_ERROR => {
                    let error = QHYError::Sdk {
                        op: "get_readout_mode_name",
                    };
                    tracing::error!(error = ?error);
                    Err(error)
                }
                _ => {
                    let name = unsafe { CStr::from_ptr(name.as_ptr()) }
                        .to_str()
                        .inspect_err(|error| tracing::error!(error = ?error))?;
                    Ok(name.to_string())
                }
            }
        }
        #[cfg(feature = "simulation")]
        {
            let state = self.state.read();
            if !state.is_open {
                return Err(QHYError::CameraNotOpen);
            }
            state
                .config
                .readout_modes
                .get(index as usize)
                .map(|(name, _)| name.clone())
                .ok_or(QHYError::Sdk {
                    op: "get_readout_mode_name",
                })
        }
    }

    /// Returns the readout mode resolution with the given index. Make sure to check the number of readout modes.
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,Camera};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// camera.open().expect("open failed");
    /// let (width, height) = camera.get_readout_mode_resolution(0).expect("get_readout_mode_resolution failed");
    /// println!("Readout mode resolution: {}x{}", width, height);
    /// ```
    pub fn get_readout_mode_resolution(&self, index: u32) -> Result<(u32, u32)> {
        #[cfg(not(feature = "simulation"))]
        {
            let handle = read_lock!(self.handle)?;

            let mut width: u32 = 0;
            let mut height: u32 = 0;
            match unsafe {
                GetQHYCCDReadModeResolution(
                    handle,
                    index,
                    &mut width as *mut u32,
                    &mut height as *mut u32,
                )
            } {
                QHYCCD_SUCCESS => Ok((width, height)),
                _ => {
                    let error = QHYError::Sdk {
                        op: "get_readout_mode_resolution",
                    };
                    tracing::error!(error = ?error);
                    Err(error)
                }
            }
        }
        #[cfg(feature = "simulation")]
        {
            let state = self.state.read();
            if !state.is_open {
                return Err(QHYError::CameraNotOpen);
            }
            state
                .config
                .readout_modes
                .get(index as usize)
                .map(|(_, res)| *res)
                .ok_or(QHYError::Sdk {
                    op: "get_readout_mode_resolution",
                })
        }
    }

    /// Returns the current readout mode
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,Camera};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// camera.open().expect("open failed");
    /// let readout_mode = camera.get_readout_mode().expect("get_readout_mode failed");
    /// println!("Readout mode: {}", readout_mode);
    /// ```
    pub fn get_readout_mode(&self) -> Result<u32> {
        #[cfg(not(feature = "simulation"))]
        {
            let handle = read_lock!(self.handle)?;
            let mut mode: u32 = 0;
            match unsafe { GetQHYCCDReadMode(handle, &mut mode as *mut u32) } {
                QHYCCD_SUCCESS => Ok(mode),
                _ => {
                    let error = QHYError::Sdk {
                        op: "get_readout_mode",
                    };
                    tracing::error!(error = ?error);
                    Err(error)
                }
            }
        }
        #[cfg(feature = "simulation")]
        {
            let state = self.state.read();
            if !state.is_open {
                return Err(QHYError::CameraNotOpen);
            }
            Ok(state.readout_mode)
        }
    }
}
