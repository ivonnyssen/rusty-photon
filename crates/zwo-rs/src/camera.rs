//! ASI camera enumeration and device handles.
//!
//! [`Sdk::cameras`] lists every connected camera's [`CameraInfo`] without
//! opening it. [`Sdk::open_camera`] opens and initialises a camera, returning a
//! [`Camera`] RAII handle that closes the device on drop. The handle covers the
//! imaging path: ROI/binning ([`Camera::set_roi_format`]), controls
//! ([`Camera::control_value`] / [`Camera::set_control_value`]), single exposures
//! ([`Camera::start_exposure`] / [`Camera::download_exposure`]), and ST4 guiding
//! ([`Camera::pulse_guide_on`]). With the `simulation` feature a single
//! fabricated `ASI2600MM-Pro-Simulated` camera is presented and the SDK is never
//! called.

#[cfg(not(feature = "simulation"))]
use crate::ffi_util::{c_string_field, hex8};
#[cfg(not(feature = "simulation"))]
use crate::{asi_check, sys};
#[cfg(not(feature = "simulation"))]
use std::os::raw::{c_int, c_long, c_uint};

use crate::{AsiError, Error, Result, Sdk};

/// Bayer colour-filter pattern (`ASI_BAYER_PATTERN`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BayerPattern {
    /// `ASI_BAYER_RG`.
    Rg,
    /// `ASI_BAYER_BG`.
    Bg,
    /// `ASI_BAYER_GR`.
    Gr,
    /// `ASI_BAYER_GB`.
    Gb,
}

impl BayerPattern {
    #[cfg(not(feature = "simulation"))]
    #[must_use]
    fn from_raw(v: u32) -> Self {
        match v {
            0 => Self::Rg,
            1 => Self::Bg,
            2 => Self::Gr,
            _ => Self::Gb,
        }
    }
}

/// Safe view of `ASI_CAMERA_INFO`. Readable without opening the camera
/// (via [`Sdk::cameras`]); also cached on an open [`Camera`].
#[derive(Debug, Clone, PartialEq)]
pub struct CameraInfo {
    /// `CameraID` — the handle used by all per-camera SDK calls.
    pub id: i32,
    /// Model name, e.g. `"ZWO ASI2600MM Pro"`.
    pub name: String,
    /// Full sensor width in pixels.
    pub max_width: u32,
    /// Full sensor height in pixels.
    pub max_height: u32,
    /// Colour (Bayer) sensor when `true`, monochrome when `false`.
    pub is_color: bool,
    /// Bayer pattern (meaningful only when [`is_color`](Self::is_color)).
    pub bayer_pattern: BayerPattern,
    /// Supported symmetric binning factors (e.g. `[1, 2, 3, 4]`).
    pub supported_bins: Vec<u32>,
    /// Pixel size in micrometres.
    pub pixel_size_um: f64,
    /// Whether the camera has a mechanical shutter.
    pub has_mechanical_shutter: bool,
    /// Whether the camera exposes an ST4 guide port.
    pub has_st4_port: bool,
    /// Whether the camera is a cooled model.
    pub is_cooler_cam: bool,
    /// Whether the camera is a USB 3.0 device.
    pub is_usb3: bool,
    /// Native electrons-per-ADU (`ElecPerADU`).
    pub e_per_adu: f32,
    /// ADC bit depth (e.g. `16`, `14`, `12`).
    pub bit_depth: u32,
    /// Whether the camera supports the trigger (industrial) modes.
    pub is_trigger_cam: bool,
}

/// ASI control type (`ASI_CONTROL_TYPE`) — the imaging-relevant subset.
///
/// Unrecognised control types are preserved as [`ControlType::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlType {
    /// `ASI_GAIN`.
    Gain,
    /// `ASI_EXPOSURE` (microseconds).
    Exposure,
    /// `ASI_OFFSET` (a.k.a. brightness).
    Offset,
    /// `ASI_TEMPERATURE` (sensor temperature, in 0.1 °C units).
    Temperature,
    /// `ASI_HIGH_SPEED_MODE`.
    HighSpeedMode,
    /// `ASI_COOLER_POWER_PERC`.
    CoolerPowerPerc,
    /// `ASI_TARGET_TEMP` (cooler set-point, whole °C).
    TargetTemp,
    /// `ASI_COOLER_ON`.
    CoolerOn,
    /// A control type outside the subset mapped here; carries the raw value.
    Other(i32),
}

impl ControlType {
    #[cfg(not(feature = "simulation"))]
    #[must_use]
    fn from_raw(v: i32) -> Self {
        match v {
            0 => Self::Gain,
            1 => Self::Exposure,
            5 => Self::Offset,
            8 => Self::Temperature,
            14 => Self::HighSpeedMode,
            15 => Self::CoolerPowerPerc,
            16 => Self::TargetTemp,
            17 => Self::CoolerOn,
            other => Self::Other(other),
        }
    }

    #[cfg(not(feature = "simulation"))]
    fn to_raw(self) -> c_uint {
        match self {
            Self::Gain => 0,
            Self::Exposure => 1,
            Self::Offset => 5,
            Self::Temperature => 8,
            Self::HighSpeedMode => 14,
            Self::CoolerPowerPerc => 15,
            Self::TargetTemp => 16,
            Self::CoolerOn => 17,
            Self::Other(v) => v as c_uint,
        }
    }
}

/// Safe view of `ASI_CONTROL_CAPS` — one tunable control's range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlCaps {
    /// Control name, e.g. `"Gain"`.
    pub name: String,
    /// Which control this describes.
    pub control_type: ControlType,
    /// Minimum value.
    pub min: i64,
    /// Maximum value.
    pub max: i64,
    /// Default value.
    pub default: i64,
    /// Whether the control can be written (some, e.g. temperature, are read-only).
    pub is_writable: bool,
    /// Whether the control supports the SDK's auto mode.
    pub is_auto_supported: bool,
}

/// Output image format (`ASI_IMG_TYPE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageType {
    /// `ASI_IMG_RAW8` — 8-bit raw (1 byte/pixel).
    Raw8,
    /// `ASI_IMG_RGB24` — 8-bit BGR (3 bytes/pixel).
    Rgb24,
    /// `ASI_IMG_RAW16` — 16-bit raw (2 bytes/pixel).
    Raw16,
    /// `ASI_IMG_Y8` — 8-bit luminance (1 byte/pixel).
    Y8,
}

impl ImageType {
    /// Bytes per pixel for this format.
    #[must_use]
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Raw8 | Self::Y8 => 1,
            Self::Raw16 => 2,
            Self::Rgb24 => 3,
        }
    }

    #[cfg(not(feature = "simulation"))]
    fn to_raw(self) -> c_int {
        match self {
            Self::Raw8 => 0,
            Self::Rgb24 => 1,
            Self::Raw16 => 2,
            Self::Y8 => 3,
        }
    }

    #[cfg(not(feature = "simulation"))]
    fn from_raw(v: c_int) -> Option<Self> {
        match v {
            0 => Some(Self::Raw8),
            1 => Some(Self::Rgb24),
            2 => Some(Self::Raw16),
            3 => Some(Self::Y8),
            _ => None,
        }
    }
}

/// The region-of-interest format: frame size, binning, and pixel format.
///
/// `width`/`height` are **post-binning** pixel counts (as the SDK expects): a
/// full-frame 6248×4176 sensor at bin 2 is `width = 3124`, `height = 2088`. The
/// SDK requires `width % 8 == 0` and `height % 2 == 0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoiFormat {
    /// Post-binning frame width in pixels (`width % 8 == 0`).
    pub width: u32,
    /// Post-binning frame height in pixels (`height % 2 == 0`).
    pub height: u32,
    /// Symmetric binning factor (1 = no binning).
    pub bin: u32,
    /// Pixel/output format.
    pub image_type: ImageType,
}

impl RoiFormat {
    /// Byte length of one full frame in this format
    /// (`width × height × bytes/pixel`).
    #[must_use]
    pub fn buffer_len(&self) -> usize {
        self.width as usize * self.height as usize * self.image_type.bytes_per_pixel()
    }
}

/// Exposure state machine (`ASI_EXPOSURE_STATUS`), reported by
/// [`Camera::exposure_status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExposureStatus {
    /// `ASI_EXP_IDLE` — no exposure in progress; ready to start one.
    Idle,
    /// `ASI_EXP_WORKING` — exposing.
    Working,
    /// `ASI_EXP_SUCCESS` — finished; the frame is ready to download.
    Success,
    /// `ASI_EXP_FAILED` — the exposure failed; start it again.
    Failed,
}

impl ExposureStatus {
    #[cfg(not(feature = "simulation"))]
    #[must_use]
    fn from_raw(v: c_uint) -> Self {
        match v {
            0 => Self::Idle,
            1 => Self::Working,
            2 => Self::Success,
            _ => Self::Failed,
        }
    }
}

/// ST4 guide-pulse direction (`ASI_GUIDE_DIRECTION`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuideDirection {
    /// `ASI_GUIDE_NORTH` (+Dec).
    North,
    /// `ASI_GUIDE_SOUTH` (−Dec).
    South,
    /// `ASI_GUIDE_EAST` (+RA).
    East,
    /// `ASI_GUIDE_WEST` (−RA).
    West,
}

impl GuideDirection {
    #[cfg(not(feature = "simulation"))]
    fn to_raw(self) -> c_uint {
        match self {
            Self::North => 0,
            Self::South => 1,
            Self::East => 2,
            Self::West => 3,
        }
    }
}

/// A control's current value and whether it is in SDK auto mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlValue {
    /// The raw control value (e.g. gain, offset; temperature is in 0.1 °C units).
    pub value: i64,
    /// Whether the control is currently in the SDK's auto mode.
    pub is_auto: bool,
}

/// An open ASI camera. Closes the device on drop.
///
/// The ZWO SDK is not safe for concurrent calls on a single camera handle, so
/// `Camera` is `Send` but **not** `Sync`: move it between threads freely, but to
/// share it across threads put it behind a `Mutex` so the SDK calls serialise.
/// Without this, a second thread could resize the ROI (`set_roi_format`) during
/// an in-flight [`Camera::download_exposure`], making the SDK write past the
/// caller's buffer.
#[derive(Debug)]
pub struct Camera {
    info: CameraInfo,
    #[cfg(feature = "simulation")]
    state: std::sync::Mutex<SimState>,
    /// Makes `Camera` `!Sync` (see the type docs) while leaving it `Send`.
    _not_sync: std::marker::PhantomData<std::cell::Cell<()>>,
}

impl Sdk {
    /// Enumerate every connected camera's [`CameraInfo`] without opening it.
    ///
    /// # Errors
    /// Returns [`Error::Asi`] if the SDK fails to read a camera's properties.
    pub fn cameras(&self) -> Result<Vec<CameraInfo>> {
        #[cfg(feature = "simulation")]
        let infos = (0..crate::SIM_CAMERA_COUNT)
            .map(|_| sim_camera_info())
            .collect();
        #[cfg(not(feature = "simulation"))]
        let infos = {
            let n = self.camera_count()?;
            (0..n)
                .map(|index| {
                    let idx =
                        i32::try_from(index).map_err(|_| Error::Asi(AsiError::InvalidIndex))?;
                    read_camera_property(idx)
                })
                .collect::<Result<Vec<_>>>()?
        };
        Ok(infos)
    }

    /// Open and initialise the camera at enumeration `index`.
    ///
    /// On the real path this calls `ASIOpenCamera` + `ASIInitCamera`; the
    /// returned [`Camera`] closes the device on drop.
    ///
    /// # Errors
    /// Returns [`Error::Asi`] if the index is out of range or the SDK fails to
    /// open/initialise the camera.
    pub fn open_camera(&self, index: usize) -> Result<Camera> {
        #[cfg(feature = "simulation")]
        let camera = {
            if index >= crate::SIM_CAMERA_COUNT {
                return Err(Error::Asi(AsiError::InvalidIndex));
            }
            let info = sim_camera_info();
            let state = std::sync::Mutex::new(SimState::new(&info));
            Camera {
                info,
                state,
                _not_sync: std::marker::PhantomData,
            }
        };
        #[cfg(not(feature = "simulation"))]
        let camera = {
            let idx = i32::try_from(index).map_err(|_| Error::Asi(AsiError::InvalidIndex))?;
            let info = read_camera_property(idx)?;
            // SAFETY: `info.id` is a valid CameraID from enumeration; open it.
            asi_check(unsafe { sys::ASIOpenCamera(info.id) } as i32)?;
            // SAFETY: the camera was just opened; initialise it. On failure,
            // close it again so the open handle is not leaked.
            if let Err(e) = asi_check(unsafe { sys::ASIInitCamera(info.id) } as i32) {
                unsafe {
                    let _ = sys::ASICloseCamera(info.id);
                }
                return Err(e);
            }
            Camera {
                info,
                _not_sync: std::marker::PhantomData,
            }
        };
        Ok(camera)
    }
}

impl Camera {
    /// The camera's cached [`CameraInfo`].
    #[must_use]
    pub fn info(&self) -> &CameraInfo {
        &self.info
    }

    /// The camera's `CameraID`.
    #[must_use]
    pub fn id(&self) -> i32 {
        self.info.id
    }

    /// The camera's stable serial number as a 16-character hex string.
    ///
    /// Reads `ASIGetSerialNumber` (the 8-byte hardware serial); if the model
    /// reports none, falls back to the writable flash id (`ASIGetID`, USB3
    /// only). Requires the camera to be open, which it always is here.
    ///
    /// # Errors
    /// Returns [`Error::Asi`] if neither a serial nor a flash id is available.
    pub fn serial(&self) -> Result<String> {
        #[cfg(feature = "simulation")]
        let serial = SIM_SERIAL.to_owned();
        #[cfg(not(feature = "simulation"))]
        let serial = {
            // SAFETY: `ASI_SN` is a POD `[u8; 8]`; the SDK fills it on success.
            let mut sn: sys::ASI_SN = unsafe { std::mem::zeroed() };
            if asi_check(unsafe { sys::ASIGetSerialNumber(self.info.id, &mut sn) } as i32).is_ok() {
                hex8(&sn.id)
            } else {
                // SAFETY: as above; `ASIGetID` fills the 8-byte flash id.
                let mut fid: sys::ASI_ID = unsafe { std::mem::zeroed() };
                asi_check(unsafe { sys::ASIGetID(self.info.id, &mut fid) } as i32)?;
                hex8(&fid.id)
            }
        };
        Ok(serial)
    }

    /// Enumerate this camera's tunable controls and their ranges.
    ///
    /// # Errors
    /// Returns [`Error::Asi`] if the SDK fails to read the control list.
    pub fn control_caps(&self) -> Result<Vec<ControlCaps>> {
        #[cfg(feature = "simulation")]
        let caps = sim_control_caps();
        #[cfg(not(feature = "simulation"))]
        let caps = {
            let mut n: std::os::raw::c_int = 0;
            // SAFETY: `self.info.id` is an open camera; the SDK writes the count.
            asi_check(unsafe { sys::ASIGetNumOfControls(self.info.id, &mut n) } as i32)?;
            let count = usize::try_from(n).unwrap_or(0);
            (0..count)
                .map(|i| {
                    let idx = i32::try_from(i).map_err(|_| Error::Asi(AsiError::InvalidIndex))?;
                    // SAFETY: POD struct filled by the SDK for a valid index.
                    let mut raw: sys::ASI_CONTROL_CAPS = unsafe { std::mem::zeroed() };
                    asi_check(
                        unsafe { sys::ASIGetControlCaps(self.info.id, idx, &mut raw) } as i32,
                    )?;
                    Ok(control_caps_from_raw(&raw))
                })
                .collect::<Result<Vec<_>>>()?
        };
        Ok(caps)
    }

    /// Set the ROI format: frame size (post-binning), binning, and pixel format.
    ///
    /// Capture must be stopped first. `width` must be a multiple of 8 and
    /// `height` a multiple of 2 (SDK requirements).
    ///
    /// # Errors
    /// Returns [`Error::Asi`] if the size, binning, or format is invalid for
    /// this camera.
    pub fn set_roi_format(
        &self,
        width: u32,
        height: u32,
        bin: u32,
        image_type: ImageType,
    ) -> Result<()> {
        #[cfg(feature = "simulation")]
        self.sim_set_roi_format(width, height, bin, image_type)?;
        #[cfg(not(feature = "simulation"))]
        {
            let w = c_int::try_from(width).map_err(|_| Error::Asi(AsiError::InvalidSize))?;
            let h = c_int::try_from(height).map_err(|_| Error::Asi(AsiError::InvalidSize))?;
            let b = c_int::try_from(bin).map_err(|_| Error::Asi(AsiError::InvalidSize))?;
            // SAFETY: open camera id; the SDK validates size/binning/format.
            asi_check(
                unsafe { sys::ASISetROIFormat(self.info.id, w, h, b, image_type.to_raw()) } as i32,
            )?;
        }
        Ok(())
    }

    /// Read the current ROI format.
    ///
    /// # Errors
    /// Returns [`Error::Asi`] if the SDK call fails or the camera reports an
    /// unknown image type.
    pub fn roi_format(&self) -> Result<RoiFormat> {
        #[cfg(feature = "simulation")]
        let roi = self.sim_roi_format()?;
        #[cfg(not(feature = "simulation"))]
        let roi = {
            let mut w: c_int = 0;
            let mut h: c_int = 0;
            let mut b: c_int = 0;
            let mut img: sys::ASI_IMG_TYPE = 0;
            // SAFETY: open camera id; the SDK writes the four out-params.
            asi_check(unsafe {
                sys::ASIGetROIFormat(self.info.id, &mut w, &mut h, &mut b, &mut img)
            } as i32)?;
            let image_type =
                ImageType::from_raw(img).ok_or(Error::Asi(AsiError::InvalidImgType))?;
            RoiFormat {
                width: u32::try_from(w).unwrap_or(0),
                height: u32::try_from(h).unwrap_or(0),
                bin: u32::try_from(b).unwrap_or(0),
                image_type,
            }
        };
        Ok(roi)
    }

    /// Set the ROI start position (top-left), in post-binning coordinates.
    ///
    /// # Errors
    /// Returns [`Error::Asi`] if the position puts the frame out of bounds.
    pub fn set_start_pos(&self, x: u32, y: u32) -> Result<()> {
        #[cfg(feature = "simulation")]
        self.sim_set_start_pos(x, y)?;
        #[cfg(not(feature = "simulation"))]
        {
            let sx = c_int::try_from(x).map_err(|_| Error::Asi(AsiError::OutOfBoundary))?;
            let sy = c_int::try_from(y).map_err(|_| Error::Asi(AsiError::OutOfBoundary))?;
            // SAFETY: open camera id; the SDK validates the position.
            asi_check(unsafe { sys::ASISetStartPos(self.info.id, sx, sy) } as i32)?;
        }
        Ok(())
    }

    /// Read the current ROI start position (top-left), in post-binning
    /// coordinates.
    ///
    /// # Errors
    /// Returns [`Error::Asi`] if the SDK call fails.
    pub fn start_pos(&self) -> Result<(u32, u32)> {
        #[cfg(feature = "simulation")]
        let pos = self.sim_start_pos()?;
        #[cfg(not(feature = "simulation"))]
        let pos = {
            let mut x: c_int = 0;
            let mut y: c_int = 0;
            // SAFETY: open camera id; the SDK writes both out-params.
            asi_check(unsafe { sys::ASIGetStartPos(self.info.id, &mut x, &mut y) } as i32)?;
            (u32::try_from(x).unwrap_or(0), u32::try_from(y).unwrap_or(0))
        };
        Ok(pos)
    }

    /// Read a control's current value and auto flag.
    ///
    /// Temperature is reported in 0.1 °C units (see
    /// [`Camera::temperature_celsius`]).
    ///
    /// # Errors
    /// Returns [`Error::Asi`] if the control type is invalid for this camera.
    pub fn control_value(&self, control: ControlType) -> Result<ControlValue> {
        #[cfg(feature = "simulation")]
        let value = self.sim_control_value(control)?;
        #[cfg(not(feature = "simulation"))]
        let value = {
            let mut v: c_long = 0;
            let mut auto: sys::ASI_BOOL = 0;
            // SAFETY: open camera id; the SDK writes the value and auto flag.
            asi_check(unsafe {
                sys::ASIGetControlValue(self.info.id, control.to_raw(), &mut v, &mut auto)
            } as i32)?;
            ControlValue {
                value: v,
                is_auto: auto != 0,
            }
        };
        Ok(value)
    }

    /// Set a control's value (and auto mode).
    ///
    /// The SDK clamps out-of-range values to the control's min/max rather than
    /// failing.
    ///
    /// # Errors
    /// Returns [`Error::Asi`] if the control type is invalid or the value is
    /// rejected by the camera.
    pub fn set_control_value(&self, control: ControlType, value: i64, auto: bool) -> Result<()> {
        #[cfg(feature = "simulation")]
        self.sim_set_control_value(control, value, auto)?;
        #[cfg(not(feature = "simulation"))]
        {
            let auto_flag: sys::ASI_BOOL = if auto { 1 } else { 0 };
            // SAFETY: open camera id; the SDK validates control/value.
            asi_check(unsafe {
                sys::ASISetControlValue(self.info.id, control.to_raw(), value, auto_flag)
            } as i32)?;
        }
        Ok(())
    }

    /// Sensor temperature in °C (decodes the 0.1 °C `ASI_TEMPERATURE` units).
    ///
    /// # Errors
    /// Returns [`Error::Asi`] if the temperature control cannot be read.
    pub fn temperature_celsius(&self) -> Result<f64> {
        let raw = self.control_value(ControlType::Temperature)?;
        Ok(raw.value as f64 / 10.0)
    }

    /// Start a single exposure. `is_dark` requests a dark frame on cameras with
    /// a mechanical shutter (ignored otherwise).
    ///
    /// # Errors
    /// Returns [`Error::Asi`] if the camera is in video mode or the call fails.
    pub fn start_exposure(&self, is_dark: bool) -> Result<()> {
        #[cfg(feature = "simulation")]
        self.sim_start_exposure(is_dark)?;
        #[cfg(not(feature = "simulation"))]
        {
            let dark: sys::ASI_BOOL = if is_dark { 1 } else { 0 };
            // SAFETY: open camera id; starts a single exposure.
            asi_check(unsafe { sys::ASIStartExposure(self.info.id, dark) } as i32)?;
        }
        Ok(())
    }

    /// Cancel an exposure in progress.
    ///
    /// # Errors
    /// Returns [`Error::Asi`] if the call fails.
    pub fn stop_exposure(&self) -> Result<()> {
        #[cfg(feature = "simulation")]
        self.sim_stop_exposure()?;
        #[cfg(not(feature = "simulation"))]
        // SAFETY: open camera id; cancels any exposure in progress.
        asi_check(unsafe { sys::ASIStopExposure(self.info.id) } as i32)?;
        Ok(())
    }

    /// Read the current [`ExposureStatus`].
    ///
    /// # Errors
    /// Returns [`Error::Asi`] if the call fails.
    pub fn exposure_status(&self) -> Result<ExposureStatus> {
        #[cfg(feature = "simulation")]
        let status = self.sim_exposure_status()?;
        #[cfg(not(feature = "simulation"))]
        let status = {
            let mut s: sys::ASI_EXPOSURE_STATUS = 0;
            // SAFETY: open camera id; the SDK writes the exposure status.
            asi_check(unsafe { sys::ASIGetExpStatus(self.info.id, &mut s) } as i32)?;
            ExposureStatus::from_raw(s)
        };
        Ok(status)
    }

    /// Download the most recent frame into `buf` after
    /// [`ExposureStatus::Success`].
    ///
    /// `buf` must be at least [`RoiFormat::buffer_len`] bytes for the current
    /// ROI; a short buffer is rejected with [`AsiError::BufferTooSmall`]
    /// **before** the SDK is called (`ASIGetDataAfterExp` would otherwise read
    /// out of bounds).
    ///
    /// # Errors
    /// Returns [`Error::Asi`] if `buf` is too small or the download fails.
    pub fn download_exposure(&self, buf: &mut [u8]) -> Result<()> {
        let need = self.roi_format()?.buffer_len();
        if buf.len() < need {
            return Err(Error::Asi(AsiError::BufferTooSmall));
        }
        #[cfg(feature = "simulation")]
        self.sim_download_exposure(buf, need)?;
        #[cfg(not(feature = "simulation"))]
        {
            let len =
                c_long::try_from(buf.len()).map_err(|_| Error::Asi(AsiError::BufferTooSmall))?;
            // SAFETY: `buf` is at least `need` bytes (checked above) and `len`
            // equals its length, so the SDK writes within bounds.
            asi_check(
                unsafe { sys::ASIGetDataAfterExp(self.info.id, buf.as_mut_ptr(), len) } as i32,
            )?;
        }
        Ok(())
    }

    /// Begin an ST4 guide pulse in `direction` (requires an ST4 port).
    ///
    /// # Errors
    /// Returns [`Error::Asi`] if the call fails.
    pub fn pulse_guide_on(&self, direction: GuideDirection) -> Result<()> {
        #[cfg(feature = "simulation")]
        let _ = direction;
        #[cfg(not(feature = "simulation"))]
        // SAFETY: open camera id; starts an ST4 pulse in the given direction.
        asi_check(unsafe { sys::ASIPulseGuideOn(self.info.id, direction.to_raw()) } as i32)?;
        Ok(())
    }

    /// End an ST4 guide pulse in `direction`.
    ///
    /// # Errors
    /// Returns [`Error::Asi`] if the call fails.
    pub fn pulse_guide_off(&self, direction: GuideDirection) -> Result<()> {
        #[cfg(feature = "simulation")]
        let _ = direction;
        #[cfg(not(feature = "simulation"))]
        // SAFETY: open camera id; ends the ST4 pulse in the given direction.
        asi_check(unsafe { sys::ASIPulseGuideOff(self.info.id, direction.to_raw()) } as i32)?;
        Ok(())
    }
}

#[cfg(not(feature = "simulation"))]
impl Drop for Camera {
    fn drop(&mut self) {
        // SAFETY: closing an open camera by id; `ASICloseCamera` is idempotent
        // and returns success even if the camera is already closed.
        unsafe {
            let _ = sys::ASICloseCamera(self.info.id);
        }
    }
}

// ---- real FFI helpers --------------------------------------------------------

#[cfg(not(feature = "simulation"))]
fn read_camera_property(index: i32) -> Result<CameraInfo> {
    // SAFETY: `ASI_CAMERA_INFO` is POD; the SDK fills it for a valid index.
    let mut raw: sys::ASI_CAMERA_INFO = unsafe { std::mem::zeroed() };
    asi_check(unsafe { sys::ASIGetCameraProperty(&mut raw, index) } as i32)?;
    Ok(camera_info_from_raw(&raw))
}

#[cfg(not(feature = "simulation"))]
fn camera_info_from_raw(raw: &sys::ASI_CAMERA_INFO) -> CameraInfo {
    CameraInfo {
        id: raw.CameraID,
        name: c_string_field(&raw.Name),
        max_width: u32::try_from(raw.MaxWidth).unwrap_or(0),
        max_height: u32::try_from(raw.MaxHeight).unwrap_or(0),
        is_color: raw.IsColorCam != 0,
        bayer_pattern: BayerPattern::from_raw(raw.BayerPattern),
        supported_bins: raw
            .SupportedBins
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| u32::try_from(b).unwrap_or(0))
            .collect(),
        pixel_size_um: raw.PixelSize,
        has_mechanical_shutter: raw.MechanicalShutter != 0,
        has_st4_port: raw.ST4Port != 0,
        is_cooler_cam: raw.IsCoolerCam != 0,
        is_usb3: raw.IsUSB3Camera != 0,
        e_per_adu: raw.ElecPerADU,
        bit_depth: u32::try_from(raw.BitDepth).unwrap_or(0),
        is_trigger_cam: raw.IsTriggerCam != 0,
    }
}

#[cfg(not(feature = "simulation"))]
fn control_caps_from_raw(raw: &sys::ASI_CONTROL_CAPS) -> ControlCaps {
    ControlCaps {
        name: c_string_field(&raw.Name),
        control_type: ControlType::from_raw(raw.ControlType as i32),
        min: raw.MinValue,
        max: raw.MaxValue,
        default: raw.DefaultValue,
        is_writable: raw.IsWritable != 0,
        is_auto_supported: raw.IsAutoSupported != 0,
    }
}

// ---- simulation backend ------------------------------------------------------

#[cfg(feature = "simulation")]
const SIM_SERIAL: &str = "0a1b2c3d4e5f6071";

/// The fabricated simulated camera. Mirrors the rusty-photon `zwo-camera`
/// design-doc device: `ASI2600MM-Pro-Simulated` — 6248×4176 monochrome, 16-bit,
/// cooled, ST4 present.
#[cfg(feature = "simulation")]
fn sim_camera_info() -> CameraInfo {
    CameraInfo {
        id: 0,
        name: "ASI2600MM-Pro-Simulated".to_owned(),
        max_width: 6248,
        max_height: 4176,
        is_color: false,
        bayer_pattern: BayerPattern::Rg,
        supported_bins: vec![1, 2, 3, 4],
        pixel_size_um: 3.76,
        has_mechanical_shutter: false,
        has_st4_port: true,
        is_cooler_cam: true,
        is_usb3: true,
        e_per_adu: 0.25,
        bit_depth: 16,
        is_trigger_cam: false,
    }
}

#[cfg(feature = "simulation")]
fn sim_control_caps() -> Vec<ControlCaps> {
    let cap =
        |name: &str, control_type, min, max, default, is_writable, is_auto_supported| ControlCaps {
            name: name.to_owned(),
            control_type,
            min,
            max,
            default,
            is_writable,
            is_auto_supported,
        };
    vec![
        cap("Gain", ControlType::Gain, 0, 500, 100, true, true),
        cap(
            "Exposure",
            ControlType::Exposure,
            32,
            2_000_000_000,
            10_000,
            true,
            true,
        ),
        cap("Offset", ControlType::Offset, 0, 1000, 50, true, false),
        cap(
            "Temperature",
            ControlType::Temperature,
            -500,
            1000,
            0,
            false,
            false,
        ),
        cap("CoolerOn", ControlType::CoolerOn, 0, 1, 0, true, false),
        cap(
            "TargetTemp",
            ControlType::TargetTemp,
            -40,
            30,
            0,
            true,
            false,
        ),
    ]
}

/// Mutable state for the simulated camera, behind a `Mutex` so the `&self`
/// device methods can update it.
#[cfg(feature = "simulation")]
#[derive(Debug)]
struct SimState {
    roi: RoiFormat,
    start_x: u32,
    start_y: u32,
    exposure_status: ExposureStatus,
    gain: i64,
    offset: i64,
    exposure_us: i64,
    target_temp: i64,
    cooler_on: bool,
}

#[cfg(feature = "simulation")]
impl SimState {
    fn new(info: &CameraInfo) -> Self {
        Self {
            roi: RoiFormat {
                width: info.max_width,
                height: info.max_height,
                bin: 1,
                image_type: ImageType::Raw16,
            },
            start_x: 0,
            start_y: 0,
            exposure_status: ExposureStatus::Idle,
            gain: 100,
            offset: 50,
            // Matches the "Exposure" control cap default (microseconds).
            exposure_us: 10_000,
            target_temp: 0,
            cooler_on: false,
        }
    }
}

#[cfg(feature = "simulation")]
impl Camera {
    fn sim_set_roi_format(
        &self,
        width: u32,
        height: u32,
        bin: u32,
        image_type: ImageType,
    ) -> Result<()> {
        if !self.info.supported_bins.contains(&bin) {
            return Err(Error::Asi(AsiError::InvalidSize));
        }
        if !width.is_multiple_of(8) || !height.is_multiple_of(2) {
            return Err(Error::Asi(AsiError::InvalidSize));
        }
        let max_w = self.info.max_width / bin;
        let max_h = self.info.max_height / bin;
        if width == 0 || height == 0 || width > max_w || height > max_h {
            return Err(Error::Asi(AsiError::InvalidSize));
        }
        let mut st = self.state.lock().unwrap();
        st.roi = RoiFormat {
            width,
            height,
            bin,
            image_type,
        };
        // Re-centre the ROI start, matching the SDK's default behaviour.
        st.start_x = (max_w - width) / 2;
        st.start_y = (max_h - height) / 2;
        Ok(())
    }

    fn sim_roi_format(&self) -> Result<RoiFormat> {
        Ok(self.state.lock().unwrap().roi)
    }

    fn sim_set_start_pos(&self, x: u32, y: u32) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        let max_w = self.info.max_width / st.roi.bin;
        let max_h = self.info.max_height / st.roi.bin;
        if x.saturating_add(st.roi.width) > max_w || y.saturating_add(st.roi.height) > max_h {
            return Err(Error::Asi(AsiError::OutOfBoundary));
        }
        st.start_x = x;
        st.start_y = y;
        Ok(())
    }

    fn sim_start_pos(&self) -> Result<(u32, u32)> {
        let st = self.state.lock().unwrap();
        Ok((st.start_x, st.start_y))
    }

    fn sim_control_value(&self, control: ControlType) -> Result<ControlValue> {
        let st = self.state.lock().unwrap();
        let value = match control {
            ControlType::Gain => st.gain,
            ControlType::Offset => st.offset,
            ControlType::Exposure => st.exposure_us,
            ControlType::TargetTemp => st.target_temp,
            ControlType::CoolerOn => i64::from(st.cooler_on),
            ControlType::CoolerPowerPerc => {
                if st.cooler_on {
                    60
                } else {
                    0
                }
            }
            ControlType::Temperature => {
                // 0.1 °C units: track the target when cooling, else ambient.
                let celsius = if st.cooler_on { st.target_temp } else { 20 };
                celsius * 10
            }
            _ => return Err(Error::Asi(AsiError::InvalidControlType)),
        };
        Ok(ControlValue {
            value,
            is_auto: false,
        })
    }

    fn sim_set_control_value(&self, control: ControlType, value: i64, _auto: bool) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        match control {
            ControlType::Gain => st.gain = value,
            ControlType::Offset => st.offset = value,
            ControlType::Exposure => st.exposure_us = value,
            ControlType::TargetTemp => st.target_temp = value,
            ControlType::CoolerOn => st.cooler_on = value != 0,
            // Read-only (e.g. Temperature) and unknown controls are rejected.
            _ => return Err(Error::Asi(AsiError::InvalidControlType)),
        }
        Ok(())
    }

    fn sim_start_exposure(&self, _is_dark: bool) -> Result<()> {
        self.state.lock().unwrap().exposure_status = ExposureStatus::Working;
        Ok(())
    }

    fn sim_stop_exposure(&self) -> Result<()> {
        self.state.lock().unwrap().exposure_status = ExposureStatus::Idle;
        Ok(())
    }

    fn sim_exposure_status(&self) -> Result<ExposureStatus> {
        let mut st = self.state.lock().unwrap();
        let current = st.exposure_status;
        // A simulated exposure completes one poll after it starts.
        if current == ExposureStatus::Working {
            st.exposure_status = ExposureStatus::Success;
        }
        Ok(current)
    }

    fn sim_download_exposure(&self, buf: &mut [u8], need: usize) -> Result<()> {
        crate::simulation::fill_noise(&mut buf[..need]);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_is_send() {
        // The ZWO SDK isn't concurrency-safe per camera, so `Camera` is `Send`
        // (movable between threads) but deliberately not `Sync`. Lock in `Send`
        // here — the multi-threaded tokio runtime the driver uses requires it.
        fn assert_send<T: Send>() {}
        assert_send::<Camera>();
    }

    #[test]
    fn cameras_enumerates() {
        let sdk = Sdk::new().unwrap();
        let cams = sdk.cameras().unwrap();
        #[cfg(feature = "simulation")]
        {
            assert_eq!(cams.len(), crate::SIM_CAMERA_COUNT);
            let info = &cams[0];
            assert_eq!(info.name, "ASI2600MM-Pro-Simulated");
            assert_eq!(info.max_width, 6248);
            assert_eq!(info.max_height, 4176);
            assert!(!info.is_color);
            assert_eq!(info.bit_depth, 16);
            assert!(info.is_cooler_cam);
            assert!(info.has_st4_port);
            assert_eq!(info.supported_bins, vec![1, 2, 3, 4]);
        }
        // Without the feature this calls the real SDK; with no hardware the list
        // is empty, but the call must still succeed.
        #[cfg(not(feature = "simulation"))]
        {
            let _ = cams;
        }
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn open_camera_exposes_info_serial_and_controls() {
        let sdk = Sdk::new().unwrap();
        let cam = sdk.open_camera(0).unwrap();
        assert_eq!(cam.id(), 0);
        assert_eq!(cam.info().name, "ASI2600MM-Pro-Simulated");

        let serial = cam.serial().unwrap();
        assert_eq!(serial.len(), 16);
        assert!(serial.chars().all(|c| c.is_ascii_hexdigit()));

        let caps = cam.control_caps().unwrap();
        let gain = caps
            .iter()
            .find(|c| c.control_type == ControlType::Gain)
            .unwrap();
        assert_eq!(gain.max, 500);
        let exposure = caps
            .iter()
            .find(|c| c.control_type == ControlType::Exposure)
            .unwrap();
        assert_eq!(exposure.min, 32);
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn open_camera_out_of_range_is_rejected() {
        let sdk = Sdk::new().unwrap();
        assert_eq!(
            sdk.open_camera(99).unwrap_err(),
            Error::Asi(AsiError::InvalidIndex)
        );
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn roi_format_round_trips_and_recenters() {
        let sdk = Sdk::new().unwrap();
        let cam = sdk.open_camera(0).unwrap();
        // Default ROI is full-frame Raw16 bin1.
        let default = cam.roi_format().unwrap();
        assert_eq!(default.width, 6248);
        assert_eq!(default.height, 4176);
        assert_eq!(default.bin, 1);
        assert_eq!(default.image_type, ImageType::Raw16);
        assert_eq!(default.buffer_len(), 6248 * 4176 * 2);

        // A binned, sub-framed ROI.
        cam.set_roi_format(800, 600, 2, ImageType::Raw8).unwrap();
        let roi = cam.roi_format().unwrap();
        assert_eq!(roi.width, 800);
        assert_eq!(roi.height, 600);
        assert_eq!(roi.bin, 2);
        assert_eq!(roi.image_type, ImageType::Raw8);
        assert_eq!(roi.buffer_len(), 800 * 600);
        // Setting the ROI re-centres the start position (binned coordinates).
        let (sx, sy) = cam.start_pos().unwrap();
        assert_eq!(sx, (6248 / 2 - 800) / 2);
        assert_eq!(sy, (4176 / 2 - 600) / 2);
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn set_roi_format_rejects_misaligned_and_unsupported() {
        let sdk = Sdk::new().unwrap();
        let cam = sdk.open_camera(0).unwrap();
        // width not a multiple of 8
        assert_eq!(
            cam.set_roi_format(801, 600, 1, ImageType::Raw16)
                .unwrap_err(),
            Error::Asi(AsiError::InvalidSize)
        );
        // height not a multiple of 2
        assert_eq!(
            cam.set_roi_format(800, 601, 1, ImageType::Raw16)
                .unwrap_err(),
            Error::Asi(AsiError::InvalidSize)
        );
        // unsupported binning
        assert_eq!(
            cam.set_roi_format(800, 600, 5, ImageType::Raw16)
                .unwrap_err(),
            Error::Asi(AsiError::InvalidSize)
        );
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn set_start_pos_rejects_out_of_bounds() {
        let sdk = Sdk::new().unwrap();
        let cam = sdk.open_camera(0).unwrap();
        cam.set_roi_format(800, 600, 1, ImageType::Raw16).unwrap();
        cam.set_start_pos(100, 100).unwrap();
        assert_eq!(cam.start_pos().unwrap(), (100, 100));
        // start + size beyond the full frame
        assert_eq!(
            cam.set_start_pos(6000, 100).unwrap_err(),
            Error::Asi(AsiError::OutOfBoundary)
        );
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn control_values_round_trip() {
        let sdk = Sdk::new().unwrap();
        let cam = sdk.open_camera(0).unwrap();
        cam.set_control_value(ControlType::Gain, 200, false)
            .unwrap();
        assert_eq!(cam.control_value(ControlType::Gain).unwrap().value, 200);
        cam.set_control_value(ControlType::Offset, 30, false)
            .unwrap();
        assert_eq!(cam.control_value(ControlType::Offset).unwrap().value, 30);
        // Cooler + target temperature drive the simulated sensor temperature.
        cam.set_control_value(ControlType::TargetTemp, -10, false)
            .unwrap();
        cam.set_control_value(ControlType::CoolerOn, 1, false)
            .unwrap();
        assert_eq!(cam.control_value(ControlType::CoolerOn).unwrap().value, 1);
        assert!((cam.temperature_celsius().unwrap() - (-10.0)).abs() < f64::EPSILON);
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn exposure_control_round_trips() {
        let sdk = Sdk::new().unwrap();
        let cam = sdk.open_camera(0).unwrap();
        // The "Exposure" control is advertised as writable; a real ASI camera
        // accepts it, so the simulation must too (the driver sets the
        // exposure-time control before every capture).
        assert_eq!(
            cam.control_value(ControlType::Exposure).unwrap().value,
            10_000
        );
        cam.set_control_value(ControlType::Exposure, 1_500_000, false)
            .unwrap();
        assert_eq!(
            cam.control_value(ControlType::Exposure).unwrap().value,
            1_500_000
        );
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn full_frame_download_fills_buffer() {
        // Exercises the fast (parallel) frame fill at full-sensor size: the old
        // byte-at-a-time fill took >10 s here and tripped ConformU's timeout.
        let sdk = Sdk::new().unwrap();
        let cam = sdk.open_camera(0).unwrap();
        let need = cam.roi_format().unwrap().buffer_len();
        assert_eq!(need, 6248 * 4176 * 2);
        cam.start_exposure(false).unwrap();
        assert_eq!(cam.exposure_status().unwrap(), ExposureStatus::Working);
        assert_eq!(cam.exposure_status().unwrap(), ExposureStatus::Success);
        let mut buf = vec![0u8; need];
        cam.download_exposure(&mut buf).unwrap();
        assert_eq!(buf.len(), need);
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn set_unwritable_control_is_rejected() {
        let sdk = Sdk::new().unwrap();
        let cam = sdk.open_camera(0).unwrap();
        assert_eq!(
            cam.set_control_value(ControlType::Temperature, 0, false)
                .unwrap_err(),
            Error::Asi(AsiError::InvalidControlType)
        );
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn exposure_cycle_completes_and_downloads() {
        let sdk = Sdk::new().unwrap();
        let cam = sdk.open_camera(0).unwrap();
        cam.set_roi_format(800, 600, 1, ImageType::Raw16).unwrap();
        assert_eq!(cam.exposure_status().unwrap(), ExposureStatus::Idle);

        cam.start_exposure(false).unwrap();
        // The simulated exposure reports Working once, then Success.
        assert_eq!(cam.exposure_status().unwrap(), ExposureStatus::Working);
        assert_eq!(cam.exposure_status().unwrap(), ExposureStatus::Success);

        let mut buf = vec![0u8; cam.roi_format().unwrap().buffer_len()];
        cam.download_exposure(&mut buf).unwrap();
        assert_eq!(buf.len(), 800 * 600 * 2);
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn download_rejects_short_buffer() {
        let sdk = Sdk::new().unwrap();
        let cam = sdk.open_camera(0).unwrap();
        cam.set_roi_format(800, 600, 1, ImageType::Raw16).unwrap();
        let mut buf = vec![0u8; 10];
        assert_eq!(
            cam.download_exposure(&mut buf).unwrap_err(),
            Error::Asi(AsiError::BufferTooSmall)
        );
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn stop_exposure_returns_to_idle() {
        let sdk = Sdk::new().unwrap();
        let cam = sdk.open_camera(0).unwrap();
        cam.start_exposure(false).unwrap();
        cam.stop_exposure().unwrap();
        assert_eq!(cam.exposure_status().unwrap(), ExposureStatus::Idle);
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn pulse_guide_is_accepted() {
        let sdk = Sdk::new().unwrap();
        let cam = sdk.open_camera(0).unwrap();
        cam.pulse_guide_on(GuideDirection::North).unwrap();
        cam.pulse_guide_off(GuideDirection::North).unwrap();
        cam.pulse_guide_on(GuideDirection::West).unwrap();
        cam.pulse_guide_off(GuideDirection::West).unwrap();
    }
}
