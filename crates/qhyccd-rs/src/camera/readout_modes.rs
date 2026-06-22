use std::ffi::{c_char, CStr};

use crate::Result;

use crate::backend::{read_lock, CameraBackend};
use crate::QHYError::*;

use crate::ffi::{
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
        match &self.backend {
            CameraBackend::Real { handle } => {
                let handle = read_lock!(handle, GetNumberOfReadoutModesError)?;

                let mut num: u32 = 0;
                match unsafe { GetQHYCCDNumberOfReadModes(handle, &mut num as *mut u32) } {
                    QHYCCD_ERROR => {
                        let error = GetNumberOfReadoutModesError;
                        tracing::error!(error = ?error);
                        Err(error)
                    }
                    _ => Ok(num),
                }
            }
            #[cfg(feature = "simulation")]
            CameraBackend::Simulated { state } => {
                let state = state.read().map_err(|err| {
                    tracing::error!(error=?err);
                    LockPoisoned("Could not acquire read lock on simulated camera state")
                })?;
                if !state.is_open {
                    return Err(CameraNotOpenError);
                }
                Ok(state.config.readout_modes.len() as u32)
            }
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
        match &self.backend {
            CameraBackend::Real { handle } => {
                let handle = read_lock!(handle, GetReadoutModeNameError)?;
                let mut name: [c_char; 80] = [0; 80];
                match unsafe { GetQHYCCDReadModeName(handle, index, name.as_mut_ptr()) } {
                    QHYCCD_ERROR => {
                        let error = GetReadoutModeNameError;
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
            CameraBackend::Simulated { state } => {
                let state = state.read().map_err(|err| {
                    tracing::error!(error=?err);
                    LockPoisoned("Could not acquire read lock on simulated camera state")
                })?;
                if !state.is_open {
                    return Err(CameraNotOpenError);
                }
                state
                    .config
                    .readout_modes
                    .get(index as usize)
                    .map(|(name, _)| name.clone())
                    .ok_or(GetReadoutModeNameError)
            }
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
        match &self.backend {
            CameraBackend::Real { handle } => {
                let handle = read_lock!(handle, GetReadoutModeResolutionError)?;

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
                        let error = GetReadoutModeResolutionError;
                        tracing::error!(error = ?error);
                        Err(error)
                    }
                }
            }
            #[cfg(feature = "simulation")]
            CameraBackend::Simulated { state } => {
                let state = state.read().map_err(|err| {
                    tracing::error!(error=?err);
                    LockPoisoned("Could not acquire read lock on simulated camera state")
                })?;
                if !state.is_open {
                    return Err(CameraNotOpenError);
                }
                state
                    .config
                    .readout_modes
                    .get(index as usize)
                    .map(|(_, res)| *res)
                    .ok_or(GetReadoutModeResolutionError)
            }
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
        match &self.backend {
            CameraBackend::Real { handle } => {
                let handle = read_lock!(handle, GetReadoutModeError)?;
                let mut mode: u32 = 0;
                match unsafe { GetQHYCCDReadMode(handle, &mut mode as *mut u32) } {
                    QHYCCD_SUCCESS => Ok(mode),
                    _ => {
                        let error = GetReadoutModeError;
                        tracing::error!(error = ?error);
                        Err(error)
                    }
                }
            }
            #[cfg(feature = "simulation")]
            CameraBackend::Simulated { state } => {
                let state = state.read().map_err(|err| {
                    tracing::error!(error=?err);
                    LockPoisoned("Could not acquire read lock on simulated camera state")
                })?;
                if !state.is_open {
                    return Err(CameraNotOpenError);
                }
                Ok(state.readout_mode)
            }
        }
    }
}
