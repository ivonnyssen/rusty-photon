use crate::Result;

use crate::backend::{read_lock, CameraBackend};
use crate::{ControlType, QHYError::*};

use crate::ffi::{
    GetQHYCCDParam, GetQHYCCDParamMinMaxStep, IsQHYCCDCFWPlugged, IsQHYCCDControlAvailable,
    SetQHYCCDParam, QHYCCD_ERROR, QHYCCD_ERROR_F64, QHYCCD_SUCCESS,
};

use super::Camera;

impl Camera {
    /// Returns information about the control given to the function
    /// # Returns
    /// `Err` if the control is not available
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,Camera,ControlType};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// camera.open().expect("open failed");
    /// let control = camera.is_control_available(ControlType::Exposure).expect("is_control_available failed");
    /// println!("ControlType: {:?}", control);
    /// ```
    pub fn is_control_available(&self, control: ControlType) -> Option<u32> {
        match &self.backend {
            CameraBackend::Real { handle } => {
                let handle = match read_lock!(handle) {
                    Ok(handle) => handle,
                    Err(_) => return None,
                };
                match unsafe { IsQHYCCDControlAvailable(handle, control.to_raw()) } {
                    QHYCCD_ERROR => {
                        let error = IsControlAvailableError { control };
                        tracing::debug!(control = ?error);
                        None
                    }
                    is_supported => Some(is_supported),
                }
            }
            #[cfg(feature = "simulation")]
            CameraBackend::Simulated { state } => {
                let state = state.read();
                if !state.is_open {
                    return None;
                }
                // Check if control is in supported_controls
                if state.config.supported_controls.contains_key(&control) {
                    // For CamColor, return the bayer mode value
                    if control == ControlType::CamColor {
                        return state.config.bayer_mode.map(|m| m as u32);
                    }
                    Some(1) // ControlType is available
                } else {
                    None
                }
            }
        }
    }

    /// Returns the value for a given control
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,Camera,ControlType};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// camera.open().expect("open failed");
    /// let exposure = camera.get_parameter(ControlType::Exposure).expect("get_parameter failed");
    /// println!("Exposure: {}", exposure);
    /// ```
    pub fn get_parameter(&self, control: ControlType) -> Result<f64> {
        match &self.backend {
            CameraBackend::Real { handle } => {
                let handle = read_lock!(handle)?;
                let res = unsafe { GetQHYCCDParam(handle, control.to_raw()) };
                if (res - QHYCCD_ERROR_F64).abs() < f64::EPSILON {
                    let error = GetParameterError { control };
                    tracing::error!(error = ?error);
                    Err(error)
                } else {
                    Ok(res)
                }
            }
            #[cfg(feature = "simulation")]
            CameraBackend::Simulated { state } => {
                let state = state.read();
                if !state.is_open {
                    return Err(CameraNotOpenError);
                }
                // Handle special controls
                match control {
                    ControlType::CfwPort => {
                        // Return position as ASCII value (48 = '0')
                        Ok((state.filter_wheel_position + 48) as f64)
                    }
                    ControlType::CfwSlotsNum => Ok(state.config.filter_wheel_slots as f64),
                    ControlType::CurTemp => Ok(state.current_temperature),
                    ControlType::CurPWM => Ok(state.cooler_pwm),
                    ControlType::Cooler => {
                        if state
                            .config
                            .supported_controls
                            .contains_key(&ControlType::Cooler)
                        {
                            Ok(state.target_temperature)
                        } else {
                            let error = GetParameterError { control };
                            tracing::error!(error = ?error);
                            Err(error)
                        }
                    }
                    ControlType::OutputDataActualBits => Ok(state.bit_depth as f64),
                    _ => state.parameters.get(&control).copied().ok_or_else(|| {
                        let error = GetParameterError { control };
                        tracing::error!(error = ?error);
                        error
                    }),
                }
            }
        }
    }

    /// Returns the min, max and step value for a given control
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,Camera,ControlType};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// camera.open().expect("open failed");
    /// let (min_exposure, max_exposure, exposure_resolution) = camera.get_parameter_min_max_step(ControlType::Exposure).expect("getting min,max,step failed");
    /// ```
    pub fn get_parameter_min_max_step(&self, control: ControlType) -> Result<(f64, f64, f64)> {
        match &self.backend {
            CameraBackend::Real { handle } => {
                let handle = read_lock!(handle)?;
                let mut min: f64 = 0.0;
                let mut max: f64 = 0.0;
                let mut step: f64 = 0.0;
                match unsafe {
                    GetQHYCCDParamMinMaxStep(
                        handle,
                        control.to_raw(),
                        &mut min as *mut f64,
                        &mut max as *mut f64,
                        &mut step as *mut f64,
                    )
                } {
                    QHYCCD_SUCCESS => Ok((min, max, step)),
                    _ => {
                        let error = GetMinMaxStepError { control };
                        tracing::error!(error = ?error);
                        Err(error)
                    }
                }
            }
            #[cfg(feature = "simulation")]
            CameraBackend::Simulated { state } => {
                let state = state.read();
                if !state.is_open {
                    return Err(CameraNotOpenError);
                }
                state
                    .config
                    .supported_controls
                    .get(&control)
                    .copied()
                    .ok_or_else(|| {
                        let error = GetMinMaxStepError { control };
                        tracing::error!(error = ?error);
                        error
                    })
            }
        }
    }

    /// Sets the value for a given control
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,Camera,ControlType};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// camera.open().expect("open failed");
    /// camera.set_parameter(ControlType::Exposure, 2000000.0).expect("set_parameter failed");
    /// ```
    pub fn set_parameter(&self, control: ControlType, value: f64) -> Result<()> {
        match &self.backend {
            CameraBackend::Real { handle } => {
                let handle = read_lock!(handle)?;
                match unsafe { SetQHYCCDParam(handle, control.to_raw(), value) } {
                    QHYCCD_SUCCESS => Ok(()),
                    error_code => {
                        let error = SetParameterError { error_code };
                        tracing::error!(error = ?error);
                        Err(error)
                    }
                }
            }
            #[cfg(feature = "simulation")]
            CameraBackend::Simulated { state } => {
                let mut state = state.write();
                if !state.is_open {
                    return Err(CameraNotOpenError);
                }
                // Handle special controls
                match control {
                    ControlType::CfwPort => {
                        // Value is ASCII position, convert to 0-indexed
                        state.filter_wheel_position = (value as u32).saturating_sub(48);
                    }
                    ControlType::Cooler => {
                        state.target_temperature = value;
                    }
                    ControlType::ManualPWM => {
                        state.cooler_pwm = value;
                    }
                    ControlType::Exposure => {
                        state.exposure_duration_us = value as u64;
                        state.parameters.insert(control, value);
                    }
                    _ => {
                        state.parameters.insert(control, value);
                    }
                }
                Ok(())
            }
        }
    }

    /// Convinience function that sets the value for a given control if it is available
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,Camera,ControlType};
    ///
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// camera.open().expect("open failed");
    /// camera.set_if_available(ControlType::TransferBit, 16.0).expect("failed to set usb transfer mode");
    /// ```
    pub fn set_if_available(&self, control: ControlType, value: f64) -> Result<()> {
        match self.is_control_available(control) {
            Some(_) => self.set_parameter(control, value),
            None => Err(IsControlAvailableError { control }),
        }
    }

    /// Returns `true` if a filter wheel is plugged into the given camera
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,Camera,ControlType};
    ///
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// camera.open().expect("open failed");
    /// let is_cfw_plugged_in = camera.is_cfw_plugged_in().expect("is_cfw_plugged_in failed");
    /// println!("Is filter wheel plugged in: {}", is_cfw_plugged_in);
    /// ```
    pub fn is_cfw_plugged_in(&self) -> Result<bool> {
        match &self.backend {
            CameraBackend::Real { handle } => {
                let handle = read_lock!(handle)?;
                match unsafe { IsQHYCCDCFWPlugged(handle) } {
                    QHYCCD_SUCCESS => Ok(true),
                    QHYCCD_ERROR => Ok(false),
                    _ => {
                        let error = IsCfwPluggedInError;
                        tracing::error!(error = ?error);
                        Err(error)
                    }
                }
            }
            #[cfg(feature = "simulation")]
            CameraBackend::Simulated { state } => {
                let state = state.read();
                if !state.is_open {
                    return Err(CameraNotOpenError);
                }
                Ok(state.config.filter_wheel_slots > 0)
            }
        }
    }

    // --- typed accessors ------------------------------------------------------
    //
    // Thin, unit-labelled wrappers over the generic `get_parameter` /
    // `set_parameter` / `get_parameter_min_max_step` surface, mirroring
    // `svbony camera.rs`'s `gain()` / `exposure_us()` / … accessors. They are
    // the routine way to read/write the well-known controls; the generic
    // `*_parameter(ControlType, )` methods remain for `Other` and any control
    // without a dedicated accessor. QHY temperatures are already whole °C, so —
    // unlike svbony's 0.1 °C `SVB_*_TEMPERATURE` — no decode is applied.

    /// Current sensor gain (`ControlType::Gain`).
    pub fn gain(&self) -> Result<f64> {
        self.get_parameter(ControlType::Gain)
    }

    /// Set the sensor gain (`ControlType::Gain`).
    pub fn set_gain(&self, gain: f64) -> Result<()> {
        self.set_parameter(ControlType::Gain, gain)
    }

    /// `(min, max, step)` for gain (`ControlType::Gain`).
    pub fn gain_range(&self) -> Result<(f64, f64, f64)> {
        self.get_parameter_min_max_step(ControlType::Gain)
    }

    /// Current sensor offset / black level (`ControlType::Offset`).
    pub fn offset(&self) -> Result<f64> {
        self.get_parameter(ControlType::Offset)
    }

    /// Set the sensor offset / black level (`ControlType::Offset`).
    pub fn set_offset(&self, offset: f64) -> Result<()> {
        self.set_parameter(ControlType::Offset, offset)
    }

    /// `(min, max, step)` for offset (`ControlType::Offset`).
    pub fn offset_range(&self) -> Result<(f64, f64, f64)> {
        self.get_parameter_min_max_step(ControlType::Offset)
    }

    /// Current exposure time in microseconds (`ControlType::Exposure`).
    pub fn exposure_us(&self) -> Result<f64> {
        self.get_parameter(ControlType::Exposure)
    }

    /// Set the exposure time in microseconds (`ControlType::Exposure`).
    pub fn set_exposure_us(&self, exposure_us: f64) -> Result<()> {
        self.set_parameter(ControlType::Exposure, exposure_us)
    }

    /// `(min, max, step)` for exposure in microseconds (`ControlType::Exposure`).
    pub fn exposure_range_us(&self) -> Result<(f64, f64, f64)> {
        self.get_parameter_min_max_step(ControlType::Exposure)
    }

    /// Current sensor temperature in °C (`ControlType::CurTemp`). Reported
    /// independently of whether cooling is on.
    pub fn current_temperature_celsius(&self) -> Result<f64> {
        self.get_parameter(ControlType::CurTemp)
    }

    /// Engage the cooler's auto-regulation to `celsius` (`ControlType::Cooler`).
    ///
    /// Actuates hardware — callers must respect the workspace's *no actuation on
    /// connect* tenet (never call from a connect/reconnect path; see
    /// `docs/workspace.md`).
    pub fn set_target_temperature_celsius(&self, celsius: f64) -> Result<()> {
        self.set_parameter(ControlType::Cooler, celsius)
    }

    /// Set a fixed manual cooler duty cycle, 0–255 (`ControlType::ManualPWM`);
    /// writing `0.0` stops the cooler.
    ///
    /// Actuates hardware — see [`Camera::set_target_temperature_celsius`]'s
    /// tenet note.
    pub fn set_manual_cooler_pwm(&self, pwm: f64) -> Result<()> {
        self.set_parameter(ControlType::ManualPWM, pwm)
    }

    /// Current cooler power as the raw SDK PWM, 0–255 (`ControlType::CurPWM`).
    pub fn cooler_power_raw(&self) -> Result<f64> {
        self.get_parameter(ControlType::CurPWM)
    }

    /// Number of filter-wheel slots (`ControlType::CfwSlotsNum`).
    pub fn cfw_slot_count(&self) -> Result<u32> {
        Ok(self.get_parameter(ControlType::CfwSlotsNum)? as u32)
    }

    /// Current 0-indexed filter-wheel position, decoding the SDK's ASCII-offset
    /// `ControlType::CfwPort` value (`'0'` == slot 0).
    pub fn cfw_position(&self) -> Result<u32> {
        Ok((self.get_parameter(ControlType::CfwPort)? - 48_f64) as u32)
    }

    /// Command the filter wheel to a 0-indexed slot, encoding the SDK's ASCII
    /// offset onto `ControlType::CfwPort`.
    ///
    /// Actuates hardware — see [`Camera::set_target_temperature_celsius`]'s
    /// tenet note.
    pub fn set_cfw_position(&self, position: u32) -> Result<()> {
        self.set_parameter(ControlType::CfwPort, f64::from(position + 48))
    }
}
