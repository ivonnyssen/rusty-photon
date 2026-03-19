//! QHY Camera device implementation
//!
//! Implements the ASCOM Alpaca Device and Camera traits for QHYCCD cameras.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ascom_alpaca::api::camera::{CameraState, ImageArray, SensorType};
use ascom_alpaca::api::{Camera, Device};
use ascom_alpaca::{ASCOMError, ASCOMResult};
use async_trait::async_trait;
use educe::Educe;
use ndarray::Array3;
use tokio::sync::{oneshot, watch, RwLock};
use tokio::task;
use tracing::{debug, error};

use crate::config::CameraConfig;
use crate::io::{BayerMode, CameraHandle, CcdChipArea, Control, ImageData, StreamMode};

/// Signal to stop an in-progress exposure
#[derive(Debug)]
struct StopExposure {
    _want_image: bool,
}

/// Camera exposure state machine
#[derive(Educe)]
#[educe(Debug, PartialEq)]
enum State {
    Idle,
    Exposing {
        start: SystemTime,
        expected_duration_us: u32,
        #[educe(PartialEq(ignore))]
        stop_tx: Option<oneshot::Sender<StopExposure>>,
        #[educe(PartialEq(ignore))]
        done_rx: watch::Receiver<bool>,
    },
}

/// Guard macro that returns NOT_CONNECTED if the device is not connected.
macro_rules! ensure_connected {
    ($self:ident) => {
        if !$self.connected().await.is_ok_and(|connected| connected) {
            debug!("Camera device not connected");
            return Err(ASCOMError::NOT_CONNECTED);
        }
    };
}

/// QHY Camera device for ASCOM Alpaca
pub struct QhyccdCamera {
    config: CameraConfig,
    device: Box<dyn CameraHandle>,
    binning: RwLock<u8>,
    valid_bins: RwLock<Option<Vec<u8>>>,
    target_temperature: RwLock<Option<f64>>,
    ccd_info: RwLock<Option<crate::io::CcdChipInfo>>,
    intended_roi: RwLock<Option<CcdChipArea>>,
    readout_speed_min_max_step: RwLock<Option<(f64, f64, f64)>>,
    exposure_min_max_step: RwLock<Option<(f64, f64, f64)>>,
    last_exposure_start_time: RwLock<Option<SystemTime>>,
    last_exposure_duration_us: RwLock<Option<u32>>,
    last_image: Arc<RwLock<Option<ImageArray>>>,
    state: Arc<RwLock<State>>,
    gain_min_max: RwLock<Option<(f64, f64)>>,
    offset_min_max: RwLock<Option<(f64, f64)>>,
}

impl std::fmt::Debug for QhyccdCamera {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QhyccdCamera")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl QhyccdCamera {
    /// Create a new QHY Camera device
    pub fn new(config: CameraConfig, device: Box<dyn CameraHandle>) -> Self {
        Self {
            config,
            device,
            binning: RwLock::new(1_u8),
            valid_bins: RwLock::new(None),
            target_temperature: RwLock::new(None),
            ccd_info: RwLock::new(None),
            intended_roi: RwLock::new(None),
            readout_speed_min_max_step: RwLock::new(None),
            exposure_min_max_step: RwLock::new(None),
            last_exposure_start_time: RwLock::new(None),
            last_exposure_duration_us: RwLock::new(None),
            last_image: Arc::new(RwLock::new(None)),
            state: Arc::new(RwLock::new(State::Idle)),
            gain_min_max: RwLock::new(None),
            offset_min_max: RwLock::new(None),
        }
    }

    fn get_valid_binning_modes(&self) -> Vec<u8> {
        let mut modes = Vec::with_capacity(6);
        let checks = [
            (Control::CamBin1x1mode, 1_u8),
            (Control::CamBin2x2mode, 2_u8),
            (Control::CamBin3x3mode, 3_u8),
            (Control::CamBin4x4mode, 4_u8),
            (Control::CamBin6x6mode, 6_u8),
            (Control::CamBin8x8mode, 8_u8),
        ];
        for (control, bin) in checks {
            if self.device.is_control_available(control).is_some() {
                modes.push(bin);
            }
        }
        modes
    }

    /// Transform raw SDK image data into an ASCOM ImageArray
    pub fn transform_image_static(image: ImageData) -> crate::error::Result<ImageArray> {
        match image.channels {
            1 => match image.bits_per_pixel {
                8 => {
                    let expected = image.width as usize * image.height as usize;
                    if expected > image.data.len() {
                        return Err(crate::error::QhyCameraError::ImageTransform(format!(
                            "data length ({}) < width ({}) * height ({})",
                            image.data.len(),
                            image.width,
                            image.height
                        )));
                    }
                    let data: Vec<u8> = image.data[..expected].to_vec();
                    let array = Array3::from_shape_vec(
                        (image.height as usize, image.width as usize, 1),
                        data,
                    )
                    .map_err(|e| crate::error::QhyCameraError::ImageTransform(e.to_string()))?;
                    let mut swapped = array;
                    swapped.swap_axes(0, 1);
                    Ok(swapped.into())
                }
                16 => {
                    let expected = image.width as usize * image.height as usize * 2;
                    if expected > image.data.len() {
                        return Err(crate::error::QhyCameraError::ImageTransform(format!(
                            "data length ({}) < width ({}) * height ({}) * 2",
                            image.data.len(),
                            image.width,
                            image.height
                        )));
                    }
                    let data: Vec<u16> = image.data[..expected]
                        .chunks_exact(2)
                        .map(|a| u16::from_ne_bytes([a[0], a[1]]))
                        .collect();
                    let array = Array3::from_shape_vec(
                        (image.height as usize, image.width as usize, 1),
                        data,
                    )
                    .map_err(|e| crate::error::QhyCameraError::ImageTransform(e.to_string()))?;
                    let mut swapped = array;
                    swapped.swap_axes(0, 1);
                    Ok(swapped.into())
                }
                other => Err(crate::error::QhyCameraError::ImageTransform(format!(
                    "unsupported bits_per_pixel: {}",
                    other
                ))),
            },
            other => Err(crate::error::QhyCameraError::ImageTransform(format!(
                "unsupported number of channels: {}",
                other
            ))),
        }
    }

    async fn connect_camera(&self) -> ASCOMResult<()> {
        self.device.open().map_err(|e| {
            error!("open failed: {}", e);
            ASCOMError::NOT_CONNECTED
        })?;
        self.device
            .is_control_available(Control::CamSingleFrameMode)
            .ok_or_else(|| {
                error!("SingleFrameMode is not available");
                ASCOMError::NOT_CONNECTED
            })?;
        self.device
            .set_stream_mode(StreamMode::SingleFrameMode)
            .map_err(|e| {
                error!("setting StreamMode to SingleFrameMode failed: {}", e);
                ASCOMError::NOT_CONNECTED
            })?;
        self.device.set_readout_mode(0).map_err(|e| {
            error!("setting readout mode to 0 failed: {}", e);
            ASCOMError::NOT_CONNECTED
        })?;
        self.device.init().map_err(|e| {
            error!("camera init failed: {}", e);
            ASCOMError::NOT_CONNECTED
        })?;
        self.device
            .set_if_available(Control::TransferBit, 16_f64)
            .map_err(|e| {
                error!("setting transfer bits failed: {}", e);
                ASCOMError::NOT_CONNECTED
            })?;
        debug!("transfer bit set to 16");

        // Cache CCD info
        let info = self.device.get_ccd_info().map_err(|e| {
            error!("get_ccd_info failed: {}", e);
            ASCOMError::NOT_CONNECTED
        })?;
        *self.ccd_info.write().await = Some(info);

        // Cache effective area as initial ROI
        let area = self.device.get_effective_area().map_err(|e| {
            error!("get_effective_area failed: {}", e);
            ASCOMError::NOT_CONNECTED
        })?;
        *self.intended_roi.write().await = Some(area);

        // Cache valid binning modes
        *self.valid_bins.write().await = Some(self.get_valid_binning_modes());

        // Cache readout speed range if available
        if self.device.is_control_available(Control::Speed).is_some() {
            match self.device.get_parameter_min_max_step(Control::Speed) {
                Ok(mms) => *self.readout_speed_min_max_step.write().await = Some(mms),
                Err(e) => {
                    error!("get_readout_speed_min_max_step failed: {}", e);
                    return Err(ASCOMError::NOT_CONNECTED);
                }
            }
        } else {
            debug!("readout_speed control not available");
        }

        // Cache exposure range
        let exposure_mms = self
            .device
            .get_parameter_min_max_step(Control::Exposure)
            .map_err(|e| {
                error!("get_exposure_min_max_step failed: {}", e);
                ASCOMError::NOT_CONNECTED
            })?;
        *self.exposure_min_max_step.write().await = Some(exposure_mms);

        // Cache gain range if available
        if self.device.is_control_available(Control::Gain).is_some() {
            match self.device.get_parameter_min_max_step(Control::Gain) {
                Ok((min, max, _step)) => *self.gain_min_max.write().await = Some((min, max)),
                Err(e) => {
                    error!("get_gain_min_max failed: {}", e);
                    return Err(ASCOMError::NOT_CONNECTED);
                }
            }
        } else {
            debug!("gain control not available");
        }

        // Cache offset range if available
        if self.device.is_control_available(Control::Offset).is_some() {
            match self.device.get_parameter_min_max_step(Control::Offset) {
                Ok((min, max, _step)) => *self.offset_min_max.write().await = Some((min, max)),
                Err(e) => {
                    error!("get_offset_min_max failed: {}", e);
                    return Err(ASCOMError::NOT_CONNECTED);
                }
            }
        } else {
            debug!("offset control not available");
        }

        Ok(())
    }
}

#[async_trait]
impl Device for QhyccdCamera {
    fn static_name(&self) -> &str {
        &self.config.name
    }

    fn unique_id(&self) -> &str {
        &self.config.unique_id
    }

    async fn connected(&self) -> ASCOMResult<bool> {
        self.device.is_open().map_err(|e| {
            error!("is_open failed: {}", e);
            ASCOMError::NOT_CONNECTED
        })
    }

    async fn set_connected(&self, connected: bool) -> ASCOMResult<()> {
        if self.connected().await? == connected {
            return Ok(());
        }
        match connected {
            true => self.connect_camera().await,
            false => self.device.close().map_err(|e| {
                error!("close failed: {}", e);
                ASCOMError::NOT_CONNECTED
            }),
        }
    }

    async fn description(&self) -> ASCOMResult<String> {
        Ok(self.config.description.clone())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("QHY Camera Driver - ASCOM Alpaca interface for QHYCCD cameras".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }
}

#[async_trait]
impl Camera for QhyccdCamera {
    async fn bayer_offset_x(&self) -> ASCOMResult<u8> {
        ensure_connected!(self);
        self.device
            .is_control_available(Control::CamIsColor)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)?;
        let bayer_id = self
            .device
            .is_control_available(Control::CamColor)
            .ok_or(ASCOMError::INVALID_VALUE)?;
        match BayerMode::try_from(bayer_id) {
            Ok(BayerMode::GBRG | BayerMode::RGGB) => Ok(0),
            Ok(BayerMode::GRBG | BayerMode::BGGR) => Ok(1),
            Err(_) => Err(ASCOMError::INVALID_VALUE),
        }
    }

    async fn bayer_offset_y(&self) -> ASCOMResult<u8> {
        ensure_connected!(self);
        self.device
            .is_control_available(Control::CamIsColor)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)?;
        let bayer_id = self
            .device
            .is_control_available(Control::CamColor)
            .ok_or(ASCOMError::INVALID_VALUE)?;
        match BayerMode::try_from(bayer_id) {
            Ok(BayerMode::GBRG | BayerMode::BGGR) => Ok(1),
            Ok(BayerMode::GRBG | BayerMode::RGGB) => Ok(0),
            Err(_) => Err(ASCOMError::INVALID_VALUE),
        }
    }

    async fn sensor_name(&self) -> ASCOMResult<String> {
        ensure_connected!(self);
        match self.unique_id().split('-').next() {
            Some(model) => Ok(model.to_string()),
            None => Err(ASCOMError::INVALID_OPERATION),
        }
    }

    async fn bin_x(&self) -> ASCOMResult<u8> {
        ensure_connected!(self);
        Ok(*self.binning.read().await)
    }

    async fn set_bin_x(&self, bin_x: u8) -> ASCOMResult<()> {
        ensure_connected!(self);
        let valid_bins = self.valid_bins.read().await.clone().ok_or_else(|| {
            error!("valid_bins not set");
            ASCOMError::NOT_CONNECTED
        })?;
        valid_bins
            .iter()
            .find(|bin| **bin == bin_x)
            .ok_or_else(|| ASCOMError::invalid_value("bin value must be one of the valid bins"))?;
        let mut lock = self.binning.write().await;
        if *lock == bin_x {
            return Ok(());
        }
        self.device
            .set_bin_mode(bin_x as u32, bin_x as u32)
            .map_err(|e| {
                error!("set_bin_mode failed: {}", e);
                ASCOMError::VALUE_NOT_SET
            })?;
        let old = *lock;
        *lock = bin_x;
        let mut roi_lock = self.intended_roi.write().await;
        *roi_lock = roi_lock.map(|roi| CcdChipArea {
            start_x: (roi.start_x as f32 * old as f32 / bin_x as f32) as u32,
            start_y: (roi.start_y as f32 * old as f32 / bin_x as f32) as u32,
            width: (roi.width as f32 * old as f32 / bin_x as f32) as u32,
            height: (roi.height as f32 * old as f32 / bin_x as f32) as u32,
        });
        Ok(())
    }

    async fn bin_y(&self) -> ASCOMResult<u8> {
        self.bin_x().await
    }

    async fn set_bin_y(&self, bin_y: u8) -> ASCOMResult<()> {
        self.set_bin_x(bin_y).await
    }

    async fn max_bin_x(&self) -> ASCOMResult<u8> {
        ensure_connected!(self);
        self.get_valid_binning_modes()
            .iter()
            .max()
            .copied()
            .ok_or(ASCOMError::INVALID_OPERATION)
    }

    async fn max_bin_y(&self) -> ASCOMResult<u8> {
        self.max_bin_x().await
    }

    async fn camera_state(&self) -> ASCOMResult<CameraState> {
        ensure_connected!(self);
        match *self.state.read().await {
            State::Idle => Ok(CameraState::Idle),
            State::Exposing { .. } => Ok(CameraState::Exposing),
        }
    }

    async fn exposure_max(&self) -> ASCOMResult<Duration> {
        ensure_connected!(self);
        match *self.exposure_min_max_step.read().await {
            Some((_min, max, _step)) => Ok(Duration::from_micros(max as u64)),
            None => Err(ASCOMError::INVALID_VALUE),
        }
    }

    async fn exposure_min(&self) -> ASCOMResult<Duration> {
        ensure_connected!(self);
        match *self.exposure_min_max_step.read().await {
            Some((min, _max, _step)) => Ok(Duration::from_micros(min as u64)),
            None => Err(ASCOMError::INVALID_VALUE),
        }
    }

    async fn exposure_resolution(&self) -> ASCOMResult<Duration> {
        ensure_connected!(self);
        match *self.exposure_min_max_step.read().await {
            Some((_min, _max, step)) => Ok(Duration::from_micros(step as u64)),
            None => Err(ASCOMError::INVALID_VALUE),
        }
    }

    async fn has_shutter(&self) -> ASCOMResult<bool> {
        ensure_connected!(self);
        Ok(self
            .device
            .is_control_available(Control::CamMechanicalShutter)
            .is_some())
    }

    async fn image_array(&self) -> ASCOMResult<ImageArray> {
        ensure_connected!(self);
        self.last_image
            .read()
            .await
            .clone()
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn image_ready(&self) -> ASCOMResult<bool> {
        ensure_connected!(self);
        match *self.state.read().await {
            State::Idle => Ok(self.last_image.read().await.is_some()),
            State::Exposing { .. } => Ok(false),
        }
    }

    async fn last_exposure_start_time(&self) -> ASCOMResult<SystemTime> {
        ensure_connected!(self);
        self.last_exposure_start_time
            .read()
            .await
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn last_exposure_duration(&self) -> ASCOMResult<Duration> {
        ensure_connected!(self);
        self.last_exposure_duration_us
            .read()
            .await
            .map(|us| Duration::from_micros(us.into()))
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn max_adu(&self) -> ASCOMResult<u32> {
        ensure_connected!(self);
        self.device
            .get_parameter(Control::OutputDataActualBits)
            .map(|bits| 2_u32.pow(bits as u32))
            .map_err(|e| {
                error!("could not get OutputDataActualBits: {}", e);
                ASCOMError::VALUE_NOT_SET
            })
    }

    async fn camera_x_size(&self) -> ASCOMResult<u32> {
        ensure_connected!(self);
        self.ccd_info
            .read()
            .await
            .map(|info| info.image_width)
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn camera_y_size(&self) -> ASCOMResult<u32> {
        ensure_connected!(self);
        self.ccd_info
            .read()
            .await
            .map(|info| info.image_height)
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn start_x(&self) -> ASCOMResult<u32> {
        ensure_connected!(self);
        self.intended_roi
            .read()
            .await
            .map(|roi| roi.start_x)
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn set_start_x(&self, start_x: u32) -> ASCOMResult<()> {
        ensure_connected!(self);
        let mut lock = self.intended_roi.write().await;
        *lock = match *lock {
            Some(roi) => Some(CcdChipArea { start_x, ..roi }),
            None => return Err(ASCOMError::INVALID_VALUE),
        };
        Ok(())
    }

    async fn start_y(&self) -> ASCOMResult<u32> {
        ensure_connected!(self);
        self.intended_roi
            .read()
            .await
            .map(|roi| roi.start_y)
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn set_start_y(&self, start_y: u32) -> ASCOMResult<()> {
        ensure_connected!(self);
        let mut lock = self.intended_roi.write().await;
        *lock = match *lock {
            Some(roi) => Some(CcdChipArea { start_y, ..roi }),
            None => return Err(ASCOMError::INVALID_VALUE),
        };
        Ok(())
    }

    async fn num_x(&self) -> ASCOMResult<u32> {
        ensure_connected!(self);
        self.intended_roi
            .read()
            .await
            .map(|roi| roi.width)
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn set_num_x(&self, num_x: u32) -> ASCOMResult<()> {
        ensure_connected!(self);
        let mut lock = self.intended_roi.write().await;
        *lock = match *lock {
            Some(roi) => Some(CcdChipArea {
                width: num_x,
                ..roi
            }),
            None => return Err(ASCOMError::INVALID_VALUE),
        };
        Ok(())
    }

    async fn num_y(&self) -> ASCOMResult<u32> {
        ensure_connected!(self);
        self.intended_roi
            .read()
            .await
            .map(|roi| roi.height)
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn set_num_y(&self, num_y: u32) -> ASCOMResult<()> {
        ensure_connected!(self);
        let mut lock = self.intended_roi.write().await;
        *lock = match *lock {
            Some(roi) => Some(CcdChipArea {
                height: num_y,
                ..roi
            }),
            None => return Err(ASCOMError::INVALID_VALUE),
        };
        Ok(())
    }

    async fn percent_completed(&self) -> ASCOMResult<u8> {
        ensure_connected!(self);
        match *self.state.read().await {
            State::Idle => Ok(100),
            State::Exposing {
                expected_duration_us,
                ..
            } => {
                let remaining = self
                    .device
                    .get_remaining_exposure_us()
                    .map_err(|_| ASCOMError::INVALID_OPERATION)?;
                let pct = (100_f64 * remaining as f64 / expected_duration_us as f64) as u8;
                Ok(pct.min(100))
            }
        }
    }

    async fn readout_mode(&self) -> ASCOMResult<usize> {
        ensure_connected!(self);
        self.device
            .get_readout_mode()
            .map(|m| m as usize)
            .map_err(|e| {
                error!("get_readout_mode failed: {}", e);
                ASCOMError::INVALID_OPERATION
            })
    }

    async fn set_readout_mode(&self, readout_mode: usize) -> ASCOMResult<()> {
        let readout_mode_u32 = readout_mode as u32;
        ensure_connected!(self);
        let number = self.device.get_number_of_readout_modes().map_err(|e| {
            error!("get_number_of_readout_modes failed: {}", e);
            ASCOMError::INVALID_VALUE
        })?;
        if !(0..number).contains(&readout_mode_u32) {
            return Err(ASCOMError::INVALID_VALUE);
        }
        let (width, height) = self
            .device
            .get_readout_mode_resolution(readout_mode_u32)
            .map_err(|e| {
                error!("get_readout_mode_resolution failed: {}", e);
                ASCOMError::INVALID_VALUE
            })?;
        self.device
            .set_readout_mode(readout_mode_u32)
            .map_err(|e| {
                error!("set_readout_mode failed: {}", e);
                ASCOMError::VALUE_NOT_SET
            })?;
        let mut lock = self.ccd_info.write().await;
        *lock = lock.map(|ccd_info| crate::io::CcdChipInfo {
            image_width: width,
            image_height: height,
            ..ccd_info
        });
        Ok(())
    }

    async fn readout_modes(&self) -> ASCOMResult<Vec<String>> {
        ensure_connected!(self);
        let number = self.device.get_number_of_readout_modes().map_err(|e| {
            error!("get_number_of_readout_modes failed: {}", e);
            ASCOMError::INVALID_OPERATION
        })?;
        let mut modes = Vec::with_capacity(number as usize);
        for i in 0..number {
            let name = self.device.get_readout_mode_name(i).map_err(|e| {
                error!("get_readout_mode_name failed: {}", e);
                ASCOMError::INVALID_OPERATION
            })?;
            modes.push(name);
        }
        Ok(modes)
    }

    async fn sensor_type(&self) -> ASCOMResult<SensorType> {
        ensure_connected!(self);
        if self
            .device
            .is_control_available(Control::CamIsColor)
            .is_none()
        {
            return Ok(SensorType::Monochrome);
        }
        self.device
            .is_control_available(Control::CamColor)
            .map(|_| SensorType::RGGB)
            .ok_or(ASCOMError::INVALID_VALUE)
    }

    async fn start_exposure(&self, duration: Duration, light: bool) -> ASCOMResult<()> {
        if !light {
            return Err(ASCOMError::invalid_operation("dark frames not supported"));
        }
        ensure_connected!(self);

        // Validate ROI
        if self.start_x().await? > self.num_x().await? {
            return Err(ASCOMError::invalid_value("StartX > NumX"));
        }
        if self.start_y().await? > self.num_y().await? {
            return Err(ASCOMError::invalid_value("StartY > NumY"));
        }
        if self.num_x().await?
            > (self.camera_x_size().await? as f32 / self.bin_x().await? as f32) as u32
        {
            return Err(ASCOMError::invalid_value("NumX > CameraXSize"));
        }
        if self.num_y().await?
            > (self.camera_y_size().await? as f32 / self.bin_y().await? as f32) as u32
        {
            return Err(ASCOMError::invalid_value("NumY > CameraYSize"));
        }

        let Some(roi) = *self.intended_roi.read().await else {
            return Err(ASCOMError::invalid_value("no ROI defined for camera"));
        };
        self.device.set_roi(roi).map_err(|e| {
            debug!("failed to set ROI: {}", e);
            ASCOMError::invalid_value("failed to set ROI")
        })?;

        let exposure_us = (duration.as_secs_f64() * 1_000_000_f64) as u32;
        let (stop_tx, mut stop_rx) = oneshot::channel::<StopExposure>();
        let (done_tx, done_rx) = watch::channel(false);

        let mut lock = self.state.write().await;
        *lock = match *lock {
            State::Idle => State::Exposing {
                start: SystemTime::now(),
                expected_duration_us: exposure_us,
                stop_tx: Some(stop_tx),
                done_rx,
            },
            State::Exposing { .. } => {
                return Err(ASCOMError::INVALID_OPERATION);
            }
        };
        drop(lock);

        *self.last_exposure_start_time.write().await = Some(SystemTime::now());
        *self.last_exposure_duration_us.write().await = Some(exposure_us);

        self.device
            .set_parameter(Control::Exposure, exposure_us as f64)
            .map_err(|e| {
                error!("failed to set exposure time: {}", e);
                ASCOMError::INVALID_OPERATION
            })?;

        let device = self.device.clone_handle();
        let device_for_abort = self.device.clone_handle();
        let state = self.state.clone();
        let last_image = self.last_image.clone();

        tokio::spawn(async move {
            debug!("exposure task started");

            // Helper to handle abort and data exchange
            let handle_abort = || async {
                debug!("handling abort");
                if device_for_abort.abort_exposure_and_readout().is_ok() {
                    debug!("abort succeeded, completing data exchange");
                    if let Ok(buffer_size) = device_for_abort.get_image_size() {
                        if let Ok(image) = device_for_abort.get_single_frame(buffer_size) {
                            if let Ok(transformed) = QhyccdCamera::transform_image_static(image) {
                                *last_image.write().await = Some(transformed);
                                debug!("aborted exposure data stored");
                            }
                        }
                    }
                }
            };

            // Start exposure (blocking SDK call)
            let start_result = task::spawn_blocking({
                let device = device.clone_handle();
                move || device.start_single_frame_exposure()
            })
            .await;

            match start_result {
                Ok(Ok(())) => {}
                _ => {
                    error!("start exposure failed");
                    *state.write().await = State::Idle;
                    return;
                }
            }

            // Check for abort
            if stop_rx.try_recv().is_ok() {
                handle_abort().await;
                *state.write().await = State::Idle;
                return;
            }

            // Get image size (blocking)
            let size_result = task::spawn_blocking({
                let device = device.clone_handle();
                move || device.get_image_size()
            })
            .await;

            let buffer_size = match size_result {
                Ok(Ok(size)) => size,
                _ => {
                    error!("get_image_size failed");
                    *state.write().await = State::Idle;
                    return;
                }
            };

            // Check for abort
            if stop_rx.try_recv().is_ok() {
                handle_abort().await;
                *state.write().await = State::Idle;
                return;
            }

            // Get single frame (blocking)
            let image_result = task::spawn_blocking({
                let device = device.clone_handle();
                move || device.get_single_frame(buffer_size)
            })
            .await;

            let image = match image_result {
                Ok(Ok(img)) => img,
                _ => {
                    error!("get_single_frame failed");
                    *state.write().await = State::Idle;
                    return;
                }
            };

            // Transform and store
            match QhyccdCamera::transform_image_static(image) {
                Ok(transformed) => {
                    *last_image.write().await = Some(transformed);
                    let _ = done_tx.send(true);
                    debug!("exposure completed successfully");
                }
                Err(e) => error!("failed to transform image: {}", e),
            }

            *state.write().await = State::Idle;
        });

        Ok(())
    }

    async fn can_stop_exposure(&self) -> ASCOMResult<bool> {
        Ok(false)
    }

    async fn can_abort_exposure(&self) -> ASCOMResult<bool> {
        Ok(true)
    }

    async fn abort_exposure(&self) -> ASCOMResult<()> {
        ensure_connected!(self);
        let mut state_lock = self.state.write().await;
        match &mut *state_lock {
            State::Exposing { stop_tx, .. } => {
                if let Some(tx) = stop_tx.take() {
                    let _ = tx.send(StopExposure { _want_image: false });
                    Ok(())
                } else {
                    Err(ASCOMError::INVALID_OPERATION)
                }
            }
            State::Idle => Ok(()),
        }
    }

    async fn pixel_size_x(&self) -> ASCOMResult<f64> {
        ensure_connected!(self);
        self.ccd_info
            .read()
            .await
            .map(|info| info.pixel_width)
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn pixel_size_y(&self) -> ASCOMResult<f64> {
        ensure_connected!(self);
        self.ccd_info
            .read()
            .await
            .map(|info| info.pixel_height)
            .ok_or(ASCOMError::VALUE_NOT_SET)
    }

    async fn can_get_cooler_power(&self) -> ASCOMResult<bool> {
        self.can_set_ccd_temperature().await
    }

    async fn can_set_ccd_temperature(&self) -> ASCOMResult<bool> {
        ensure_connected!(self);
        Ok(self.device.is_control_available(Control::Cooler).is_some())
    }

    async fn ccd_temperature(&self) -> ASCOMResult<f64> {
        ensure_connected!(self);
        self.device
            .is_control_available(Control::Cooler)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)?;
        self.device.get_parameter(Control::CurTemp).map_err(|e| {
            error!("could not get current temperature: {}", e);
            ASCOMError::INVALID_VALUE
        })
    }

    async fn set_ccd_temperature(&self) -> ASCOMResult<f64> {
        ensure_connected!(self);
        self.device
            .is_control_available(Control::Cooler)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)?;
        match *self.target_temperature.read().await {
            Some(temp) => Ok(temp),
            None => self.ccd_temperature().await,
        }
    }

    async fn set_set_ccd_temperature(&self, set_ccd_temperature: f64) -> ASCOMResult<()> {
        if !(-273.15..=80_f64).contains(&set_ccd_temperature) {
            return Err(ASCOMError::INVALID_VALUE);
        }
        ensure_connected!(self);
        self.device
            .is_control_available(Control::Cooler)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)?;
        self.device
            .set_parameter(Control::Cooler, set_ccd_temperature)
            .map_err(|e| {
                error!("could not set target temperature: {}", e);
                ASCOMError::INVALID_OPERATION
            })?;
        *self.target_temperature.write().await = Some(set_ccd_temperature);
        Ok(())
    }

    async fn cooler_on(&self) -> ASCOMResult<bool> {
        ensure_connected!(self);
        self.device
            .is_control_available(Control::Cooler)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)?;
        let power = self.device.get_parameter(Control::CurPWM).map_err(|e| {
            error!("could not get current power: {}", e);
            ASCOMError::INVALID_VALUE
        })?;
        Ok(power > 0_f64)
    }

    async fn set_cooler_on(&self, cooler_on: bool) -> ASCOMResult<()> {
        if self.cooler_on().await? == cooler_on {
            return Ok(());
        }
        let value = if cooler_on {
            1_f64 / 100_f64 * 255_f64
        } else {
            0_f64
        };
        self.device
            .set_parameter(Control::ManualPWM, value)
            .map_err(|e| {
                error!("error setting cooler power: {}", e);
                ASCOMError::INVALID_OPERATION
            })
    }

    async fn cooler_power(&self) -> ASCOMResult<f64> {
        ensure_connected!(self);
        self.device
            .is_control_available(Control::Cooler)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)?;
        self.device
            .get_parameter(Control::CurPWM)
            .map(|pwm| pwm / 255_f64 * 100_f64)
            .map_err(|e| {
                error!("could not get cooler power: {}", e);
                ASCOMError::INVALID_VALUE
            })
    }

    async fn gain(&self) -> ASCOMResult<i32> {
        ensure_connected!(self);
        self.device
            .is_control_available(Control::Gain)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)?;
        self.device
            .get_parameter(Control::Gain)
            .map(|g| g as i32)
            .map_err(|e| {
                error!("failed to get gain: {}", e);
                ASCOMError::INVALID_OPERATION
            })
    }

    async fn set_gain(&self, gain: i32) -> ASCOMResult<()> {
        ensure_connected!(self);
        self.device
            .is_control_available(Control::Gain)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)?;
        let (min, max) = self
            .gain_min_max
            .read()
            .await
            .ok_or(ASCOMError::INVALID_OPERATION)?;
        if !(min as i32..=max as i32).contains(&gain) {
            return Err(ASCOMError::INVALID_VALUE);
        }
        self.device
            .set_parameter(Control::Gain, gain as f64)
            .map_err(|e| {
                error!("failed to set gain: {}", e);
                ASCOMError::INVALID_OPERATION
            })
    }

    async fn gain_max(&self) -> ASCOMResult<i32> {
        ensure_connected!(self);
        self.gain_min_max
            .read()
            .await
            .map(|(_, max)| max as i32)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn gain_min(&self) -> ASCOMResult<i32> {
        ensure_connected!(self);
        self.gain_min_max
            .read()
            .await
            .map(|(min, _)| min as i32)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn offset(&self) -> ASCOMResult<i32> {
        ensure_connected!(self);
        self.device
            .is_control_available(Control::Offset)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)?;
        self.device
            .get_parameter(Control::Offset)
            .map(|o| o as i32)
            .map_err(|e| {
                error!("failed to get offset: {}", e);
                ASCOMError::INVALID_OPERATION
            })
    }

    async fn set_offset(&self, offset: i32) -> ASCOMResult<()> {
        ensure_connected!(self);
        self.device
            .is_control_available(Control::Offset)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)?;
        let (min, max) = self
            .offset_min_max
            .read()
            .await
            .ok_or(ASCOMError::INVALID_OPERATION)?;
        if !(min as i32..=max as i32).contains(&offset) {
            return Err(ASCOMError::INVALID_VALUE);
        }
        self.device
            .set_parameter(Control::Offset, offset as f64)
            .map_err(|e| {
                error!("failed to set offset: {}", e);
                ASCOMError::INVALID_OPERATION
            })
    }

    async fn offset_max(&self) -> ASCOMResult<i32> {
        ensure_connected!(self);
        self.offset_min_max
            .read()
            .await
            .map(|(_, max)| max as i32)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn offset_min(&self) -> ASCOMResult<i32> {
        ensure_connected!(self);
        self.offset_min_max
            .read()
            .await
            .map(|(min, _)| min as i32)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn can_fast_readout(&self) -> ASCOMResult<bool> {
        ensure_connected!(self);
        Ok(self.device.is_control_available(Control::Speed).is_some()
            && self.readout_speed_min_max_step.read().await.is_some())
    }

    async fn fast_readout(&self) -> ASCOMResult<bool> {
        ensure_connected!(self);
        self.device
            .is_control_available(Control::Speed)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)?;
        let speed = self.device.get_parameter(Control::Speed).map_err(|e| {
            error!("failed to get speed: {}", e);
            ASCOMError::INVALID_OPERATION
        })?;
        let (_min, max, _step) = self
            .readout_speed_min_max_step
            .read()
            .await
            .ok_or(ASCOMError::INVALID_OPERATION)?;
        Ok((speed - max).abs() < f64::EPSILON)
    }

    async fn set_fast_readout(&self, fast_readout: bool) -> ASCOMResult<()> {
        ensure_connected!(self);
        self.device
            .is_control_available(Control::Speed)
            .ok_or(ASCOMError::NOT_IMPLEMENTED)?;
        let (min, max, _step) = self
            .readout_speed_min_max_step
            .read()
            .await
            .ok_or(ASCOMError::INVALID_OPERATION)?;
        let speed = if fast_readout { max } else { min };
        self.device
            .set_parameter(Control::Speed, speed)
            .map_err(|e| {
                error!("failed to set speed: {}", e);
                ASCOMError::INVALID_OPERATION
            })
    }
}
