//! Safe RAII camera handle over the ToupTek `Toupcam_*` C API.
//!
//! ## The callback → blocking bridge
//!
//! ToupTek delivers frames through a callback invoked on an **internal SDK
//! thread**, and the header warns that calling `Toupcam_Stop` / `Toupcam_Close`
//! from inside that callback **deadlocks**. So the bridge here keeps the callback
//! trivial: the `extern "C"` [`event_trampoline`] only forwards the event code
//! over an `mpsc` channel and never re-enters the SDK. The owning thread wakes on
//! the channel ([`Camera::wait_for_event`]) and does the real work
//! ([`Camera::pull_image`], [`Camera::stop`], teardown). This is the bridge
//! called out in `docs/plans/touptek-driver.md` (Concurrency).
//!
//! Discrete ASCOM-style exposures use trigger mode
//! ([`Camera::enable_trigger_mode`] + [`Camera::trigger_single`]) rather than the
//! free-running video stream.

use crate::error::{Error, Result, SdkError};
use crate::sys;
use std::time::Duration;

#[cfg(not(feature = "simulation"))]
use crate::error::hr_check;

/// Static identity + capability info for an enumerated camera.
#[derive(Debug, Clone)]
pub struct CameraInfo {
    /// SDK device id (`ToupcamDeviceV2.id`) — the handle to `Toupcam_Open`.
    pub id: String,
    /// Human-readable name (`ToupcamDeviceV2.displayname`).
    pub display_name: String,
    /// Model name (`ToupcamModelV2.name`).
    pub model_name: String,
    /// Capability flag bitmask (`ToupcamModelV2.flag`, e.g. `TOUPCAM_FLAG_*`).
    pub flag: u64,
    /// Pixel size in microns.
    pub pixel_size_x: f32,
    /// Pixel size in microns.
    pub pixel_size_y: f32,
    /// Full-frame sensor width in pixels (`ToupcamModelV2.res[0].width`).
    pub max_width: u32,
    /// Full-frame sensor height in pixels (`ToupcamModelV2.res[0].height`).
    pub max_height: u32,
    /// RAW bit depth the astro path reads (16 for the `PIXELFORMAT_RAW16` path).
    pub bit_depth: u32,
    /// Whether the sensor is colour (Bayer) — the inverse of `TOUPCAM_FLAG_MONO`.
    pub is_color: bool,
    /// Supported symmetric digital-binning factors (`OPTION_BINNING`).
    pub supported_bins: Vec<u32>,
}

impl CameraInfo {
    /// Whether the model reports a thermo-electric cooler (`TOUPCAM_FLAG_TEC`).
    #[must_use]
    pub fn has_tec(&self) -> bool {
        self.flag & u64::from(sys::TOUPCAM_FLAG_TEC) != 0
    }

    /// Whether the model reports an ST4 guide port (`TOUPCAM_FLAG_ST4`).
    #[must_use]
    pub fn has_st4(&self) -> bool {
        self.flag & u64::from(sys::TOUPCAM_FLAG_ST4) != 0
    }
}

/// An ST4 guide-pulse direction. The numeric codes are the ones
/// `Toupcam_ST4PlusGuide` expects (`0 = North`, `1 = South`, `2 = East`,
/// `3 = West`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuideDirection {
    /// +Declination.
    North,
    /// −Declination.
    South,
    /// +RA.
    East,
    /// −RA.
    West,
}

impl GuideDirection {
    /// The `nDirect` code `Toupcam_ST4PlusGuide` expects.
    ///
    /// Only the real FFI path consumes this; the simulation backend ignores the
    /// direction, so it is dead code there.
    #[must_use]
    #[cfg_attr(feature = "simulation", allow(dead_code))]
    fn code(self) -> u32 {
        match self {
            Self::North => 0,
            Self::South => 1,
            Self::East => 2,
            Self::West => 3,
        }
    }
}

/// A pulled sensor frame. `data` is the raw pixel buffer in host (native) byte
/// order; its layout depends on the configured pixel format / bit depth.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Bits per pixel requested from the SDK.
    pub bits: u32,
    /// Raw pixel bytes.
    pub data: Vec<u8>,
}

/// A pull-mode event reported by the SDK callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A streaming/preview frame is ready (`TOUPCAM_EVENT_IMAGE`).
    Image,
    /// A still / triggered frame is ready (`TOUPCAM_EVENT_STILLIMAGE`).
    StillImage,
    /// The SDK reported an error event (`TOUPCAM_EVENT_ERROR`).
    Error,
    /// The camera was disconnected (`TOUPCAM_EVENT_DISCONNECTED`).
    Disconnected,
    /// Any other event code.
    Other(u32),
}

impl Event {
    #[must_use]
    fn from_code(code: u32) -> Self {
        match code {
            c if c == sys::TOUPCAM_EVENT_IMAGE => Self::Image,
            c if c == sys::TOUPCAM_EVENT_STILLIMAGE => Self::StillImage,
            c if c == sys::TOUPCAM_EVENT_ERROR => Self::Error,
            c if c == sys::TOUPCAM_EVENT_DISCONNECTED => Self::Disconnected,
            other => Self::Other(other),
        }
    }
}

// ---------------------------------------------------------------------------
// Real callback bridge (compiled out under `simulation`).
// ---------------------------------------------------------------------------

/// Boxed context handed to the SDK as the callback's `ctxEvent`.
///
/// Its address must stay stable for the whole pull session, so it lives behind a
/// `Box` kept in the [`Camera`]; the trampoline only forwards over `tx` and never
/// re-enters the SDK.
#[cfg(not(feature = "simulation"))]
struct EventBridge {
    tx: std::sync::mpsc::Sender<u32>,
}

/// The `extern "C"` callback the SDK invokes on its internal thread.
///
/// It only forwards the event code over the channel — it never calls back into
/// the SDK, per the header's deadlock warning.
#[cfg(not(feature = "simulation"))]
unsafe extern "C" fn event_trampoline(event: std::os::raw::c_uint, ctx: *mut std::os::raw::c_void) {
    if ctx.is_null() {
        return;
    }
    // SAFETY: `ctx` is the `&EventBridge` pointer we passed to
    // `Toupcam_StartPullModeWithCallback`. It is still alive: the owning
    // `Box<EventBridge>` is dropped only after `Stop`/`Close` in `Camera::drop`,
    // so the SDK thread can never observe a freed bridge. We only `send` here.
    let bridge = unsafe { &*(ctx as *const EventBridge) };
    let _ = bridge.tx.send(event);
}

/// A safe, RAII handle to an open ToupTek camera.
///
/// `Drop` stops any pull session and closes the handle. Every method maps onto a
/// `Toupcam_*` call (real path) or a fabricated equivalent (`simulation`).
pub struct Camera {
    info: CameraInfo,
    #[cfg(not(feature = "simulation"))]
    handle: sys::HToupcam,
    /// Kept alive for the pull session; its heap address is the callback `ctx`.
    #[cfg(not(feature = "simulation"))]
    bridge: Option<Box<EventBridge>>,
    #[cfg(not(feature = "simulation"))]
    events: Option<std::sync::mpsc::Receiver<u32>>,
    #[cfg(feature = "simulation")]
    sim: SimState,
}

// SAFETY: the ToupTek handle may be used from any single thread, just not
// concurrently. rusty-photon funnels every call through one logical owner
// (`spawn_blocking` + a `Mutex`), upholding that. The event bridge is only
// dereferenced by the SDK's internal callback thread, which is torn down
// (`Stop`/`Close`) before the bridge is freed. So moving the handle between owner
// threads is sound.
#[cfg(not(feature = "simulation"))]
unsafe impl Send for Camera {}

impl Camera {
    /// Static identity + capability info for this camera.
    #[must_use]
    pub fn info(&self) -> &CameraInfo {
        &self.info
    }
}

// ---------------------------------------------------------------------------
// Real-FFI implementation.
// ---------------------------------------------------------------------------

#[cfg(not(feature = "simulation"))]
impl Camera {
    /// Open the camera at `index` (`Toupcam_OpenByIndex`).
    pub(crate) fn open_by_index(index: u32, info: CameraInfo) -> Result<Self> {
        // SAFETY: `OpenByIndex` takes an enumeration index and returns a handle
        // or null; no other invariants.
        let handle = unsafe { sys::Toupcam_OpenByIndex(index) };
        if handle.is_null() {
            return Err(Error::DeviceNotFound);
        }
        Ok(Self {
            info,
            handle,
            bridge: None,
            events: None,
        })
    }

    /// Set the exposure time in microseconds (`Toupcam_put_ExpoTime`).
    pub fn set_exposure_time_us(&self, micros: u32) -> Result<()> {
        // SAFETY: `handle` is a valid open handle; the call only sets a value.
        hr_check(unsafe { sys::Toupcam_put_ExpoTime(self.handle, micros) })
    }

    /// Set the analog gain in percent (`Toupcam_put_ExpoAGain`; 100 = 1.0×).
    pub fn set_gain_percent(&self, percent: u16) -> Result<()> {
        // SAFETY: as above.
        hr_check(unsafe { sys::Toupcam_put_ExpoAGain(self.handle, percent) })
    }

    /// Set the region of interest (`Toupcam_put_Roi`). Offsets and sizes must be
    /// even; `(0, 0, 0, 0)` resets to the full frame.
    pub fn set_roi(&self, x: u32, y: u32, width: u32, height: u32) -> Result<()> {
        // SAFETY: as above; the SDK validates the even-number / bounds rules.
        hr_check(unsafe { sys::Toupcam_put_Roi(self.handle, x, y, width, height) })
    }

    /// Set a `TOUPCAM_OPTION_*` option (`Toupcam_put_Option`).
    pub fn set_option(&self, option: u32, value: i32) -> Result<()> {
        // SAFETY: as above.
        hr_check(unsafe { sys::Toupcam_put_Option(self.handle, option, value) })
    }

    /// Read the current sensor temperature in 0.1 °C units
    /// (`Toupcam_get_Temperature`).
    pub fn temperature_tenths_c(&self) -> Result<i16> {
        let mut tenths: std::os::raw::c_short = 0;
        // SAFETY: `handle` is valid; `tenths` is a live `c_short` the SDK writes.
        hr_check(unsafe { sys::Toupcam_get_Temperature(self.handle, &mut tenths) })?;
        Ok(tenths)
    }

    /// Read a `TOUPCAM_OPTION_*` option (`Toupcam_get_Option`).
    pub fn get_option(&self, option: u32) -> Result<i32> {
        let mut value: std::os::raw::c_int = 0;
        // SAFETY: `handle` is valid; `value` is a live `c_int` the SDK writes.
        hr_check(unsafe { sys::Toupcam_get_Option(self.handle, option, &mut value) })?;
        Ok(value)
    }

    /// The current analog gain in percent (`Toupcam_get_ExpoAGain`; 100 = 1.0×).
    pub fn gain_percent(&self) -> Result<u16> {
        let mut gain: std::os::raw::c_ushort = 0;
        // SAFETY: `handle` is valid; `gain` is a live `c_ushort` the SDK writes.
        hr_check(unsafe { sys::Toupcam_get_ExpoAGain(self.handle, &mut gain) })?;
        Ok(gain)
    }

    /// The supported analog-gain range in percent (`Toupcam_get_ExpoAGainRange`).
    pub fn gain_range(&self) -> Result<(u16, u16)> {
        let (mut min, mut max, mut def): (
            std::os::raw::c_ushort,
            std::os::raw::c_ushort,
            std::os::raw::c_ushort,
        ) = (0, 0, 0);
        // SAFETY: `handle` is valid; the three out-params are live `c_ushort`s.
        hr_check(unsafe {
            sys::Toupcam_get_ExpoAGainRange(self.handle, &mut min, &mut max, &mut def)
        })?;
        Ok((min, max))
    }

    /// The supported exposure-time range in microseconds
    /// (`Toupcam_get_ExpTimeRange`).
    pub fn exposure_range_us(&self) -> Result<(u32, u32)> {
        let (mut min, mut max, mut def): (
            std::os::raw::c_uint,
            std::os::raw::c_uint,
            std::os::raw::c_uint,
        ) = (0, 0, 0);
        // SAFETY: `handle` is valid; the three out-params are live `c_uint`s.
        hr_check(unsafe {
            sys::Toupcam_get_ExpTimeRange(self.handle, &mut min, &mut max, &mut def)
        })?;
        Ok((min, max))
    }

    /// The current black level (`OPTION_BLACKLEVEL`) — the ASCOM `Offset`.
    pub fn black_level(&self) -> Result<i32> {
        self.get_option(sys::TOUPCAM_OPTION_BLACKLEVEL)
    }

    /// Set the black level (`OPTION_BLACKLEVEL`) — the ASCOM `Offset`.
    pub fn set_black_level(&self, value: i32) -> Result<()> {
        self.set_option(sys::TOUPCAM_OPTION_BLACKLEVEL, value)
    }

    /// Turn the thermo-electric cooler on/off (`OPTION_TEC`).
    pub fn set_cooler(&self, on: bool) -> Result<()> {
        self.set_option(sys::TOUPCAM_OPTION_TEC, i32::from(on))
    }

    /// Whether the cooler is currently on (`OPTION_TEC`).
    pub fn cooler_on(&self) -> Result<bool> {
        Ok(self.get_option(sys::TOUPCAM_OPTION_TEC)? != 0)
    }

    /// The current cooler power as a 0–100 % of the model's maximum TEC voltage
    /// (`OPTION_TEC_VOLTAGE` over `OPTION_TEC_VOLTAGE_MAX`).
    pub fn cooler_power_percent(&self) -> Result<u32> {
        let voltage = self.get_option(sys::TOUPCAM_OPTION_TEC_VOLTAGE)?;
        let max = self.get_option(sys::TOUPCAM_OPTION_TEC_VOLTAGE_MAX)?;
        if max <= 0 {
            return Ok(0);
        }
        let pct = i64::from(voltage.max(0)) * 100 / i64::from(max);
        Ok(pct.clamp(0, 100) as u32)
    }

    /// Set the cooler target temperature in 0.1 °C units (`OPTION_TECTARGET`).
    pub fn set_target_temperature_tenths(&self, tenths: i16) -> Result<()> {
        self.set_option(sys::TOUPCAM_OPTION_TECTARGET, i32::from(tenths))
    }

    /// The current cooler target temperature in 0.1 °C units (`OPTION_TECTARGET`).
    ///
    /// On real hardware this has a power-on default even before a setpoint is
    /// written, so the ASCOM `SetCCDTemperature` getter can always report a value.
    pub fn target_temperature_tenths(&self) -> Result<i16> {
        Ok(self.get_option(sys::TOUPCAM_OPTION_TECTARGET)? as i16)
    }

    /// Issue an ST4 guide pulse in `direction` for `duration_ms` milliseconds
    /// (`Toupcam_ST4PlusGuide`). Returns immediately; the SDK times the pulse.
    pub fn st4_pulse_guide(&self, direction: GuideDirection, duration_ms: u32) -> Result<()> {
        // SAFETY: `handle` is valid; the call only starts a guide pulse.
        hr_check(unsafe { sys::Toupcam_ST4PlusGuide(self.handle, direction.code(), duration_ms) })
    }

    /// Switch the camera into software-trigger mode (`OPTION_TRIGGER = 1`) so
    /// frames are produced one per [`trigger_single`](Self::trigger_single)
    /// instead of free-running.
    pub fn enable_trigger_mode(&self) -> Result<()> {
        self.set_option(sys::TOUPCAM_OPTION_TRIGGER, 1)
    }

    /// Start pull mode with the callback bridge
    /// (`Toupcam_StartPullModeWithCallback`).
    pub fn start_pull_mode(&mut self) -> Result<()> {
        let (tx, rx) = std::sync::mpsc::channel();
        let bridge = Box::new(EventBridge { tx });
        // The boxed bridge's heap address is the callback context; it is stable
        // across the `self.bridge = Some(bridge)` move below (only the pointer
        // moves, not the heap allocation).
        let ctx = std::ptr::addr_of!(*bridge) as *mut std::os::raw::c_void;
        // SAFETY: `handle` is valid; `event_trampoline` matches the expected ABI;
        // `ctx` points to the bridge we store in `self.bridge`, so it outlives the
        // session and is freed only after `Stop`/`Close`.
        let hr = unsafe {
            sys::Toupcam_StartPullModeWithCallback(self.handle, Some(event_trampoline), ctx)
        };
        hr_check(hr)?;
        self.bridge = Some(bridge);
        self.events = Some(rx);
        Ok(())
    }

    /// Trigger a single frame in trigger mode (`Toupcam_Trigger(h, 1)`).
    pub fn trigger_single(&self) -> Result<()> {
        // SAFETY: `handle` is valid; triggering one frame has no other invariants.
        hr_check(unsafe { sys::Toupcam_Trigger(self.handle, 1) })
    }

    /// Block until the next pull-mode event, or `timeout`.
    ///
    /// # Errors
    /// [`SdkError::Pending`] if pull mode is not running, [`SdkError::Timeout`] on
    /// timeout, or [`SdkError::DeviceFailure`] if the callback channel closed.
    pub fn wait_for_event(&self, timeout: Duration) -> Result<Event> {
        let rx = self.events.as_ref().ok_or(Error::Sdk(SdkError::Pending))?;
        match rx.recv_timeout(timeout) {
            Ok(code) => Ok(Event::from_code(code)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(Error::Sdk(SdkError::Timeout)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err(Error::Sdk(SdkError::DeviceFailure))
            }
        }
    }

    /// Copy the latest frame into an owned buffer (`Toupcam_PullImageV4`).
    ///
    /// `width`/`height`/`bits` describe the configured frame; the buffer is sized
    /// tightly (`width * height * ceil(bits/8)`) and that pitch is passed to the
    /// SDK so there is no row padding.
    pub fn pull_image(&self, width: u32, height: u32, bits: u32) -> Result<Frame> {
        let bytes_per_px = bytes_per_pixel(bits);
        let row_pitch = width as usize * bytes_per_px;
        let mut data = vec![0u8; row_pitch * height as usize];
        // SAFETY: zeroed is a valid bit pattern for the POD `ToupcamFrameInfoV4`.
        let mut info: sys::ToupcamFrameInfoV4 = unsafe { std::mem::zeroed() };
        // SAFETY: `handle` is valid; `data` is sized to `row_pitch * height`; we
        // pass that exact `row_pitch`, so the SDK writes within bounds; `info` is a
        // live POD the SDK fills.
        let hr = unsafe {
            sys::Toupcam_PullImageV4(
                self.handle,
                data.as_mut_ptr().cast(),
                0,
                bits as std::os::raw::c_int,
                row_pitch as std::os::raw::c_int,
                &mut info,
            )
        };
        hr_check(hr)?;
        Ok(Frame {
            width: info.v3.width,
            height: info.v3.height,
            bits,
            data,
        })
    }

    /// Stop the pull session (`Toupcam_Stop`) and release the callback bridge.
    pub fn stop(&mut self) -> Result<()> {
        if self.bridge.is_some() {
            // SAFETY: `handle` is valid and `Stop` is called from the owner
            // thread, never the callback thread.
            let hr = unsafe { sys::Toupcam_Stop(self.handle) };
            // Drop the bridge/receiver only after Stop has returned.
            self.bridge = None;
            self.events = None;
            hr_check(hr)?;
        }
        Ok(())
    }

    /// Whether a pull session is currently active.
    #[must_use]
    pub fn is_pulling(&self) -> bool {
        self.bridge.is_some()
    }
}

#[cfg(not(feature = "simulation"))]
impl Drop for Camera {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // Stop the pull session (off the callback thread) before the bridge is
            // freed, then close the handle. Teardown errors are ignored.
            // SAFETY: `handle` is valid and owned; not called from the callback
            // thread. After this the `Box<EventBridge>` field drops — never while
            // the SDK thread could still touch it.
            unsafe {
                sys::Toupcam_Stop(self.handle);
                sys::Toupcam_Close(self.handle);
            }
        }
    }
}

/// Bytes per pixel for a requested bit depth (RAW mono), minimum 1.
#[cfg(not(feature = "simulation"))]
fn bytes_per_pixel(bits: u32) -> usize {
    (bits as usize).div_ceil(8).max(1)
}

// ---------------------------------------------------------------------------
// Simulation implementation (no SDK calls).
// ---------------------------------------------------------------------------

/// Simulated analog-gain bounds in percent (100 = 1.0× … 1000 = 10×).
#[cfg(feature = "simulation")]
const SIM_GAIN_DEFAULT: u16 = 100;
#[cfg(feature = "simulation")]
const SIM_GAIN_MAX: u16 = 1000;
/// Simulated exposure-time bounds in microseconds (100 µs … 3600 s). The upper
/// bound is below 100 000 s so the out-of-range exposure scenario (E3) is rejected.
#[cfg(feature = "simulation")]
const SIM_EXPOSURE_MIN_US: u32 = 100;
#[cfg(feature = "simulation")]
const SIM_EXPOSURE_MAX_US: u32 = 3_600_000_000;

/// In-Rust state backing a simulated camera.
#[cfg(feature = "simulation")]
struct SimState {
    /// Pending pull-mode events (pushed by `trigger_single`, drained by
    /// `wait_for_event`).
    events: std::sync::Mutex<std::collections::VecDeque<u32>>,
    /// Whether a pull session is active.
    started: std::sync::atomic::AtomicBool,
    /// Current analog gain in percent (mirrors `put/get_ExpoAGain`).
    gain: std::sync::Mutex<u16>,
    /// Current black level (mirrors `OPTION_BLACKLEVEL`).
    black_level: std::sync::Mutex<i32>,
    /// Whether the cooler is on (mirrors `OPTION_TEC`).
    cooler_on: std::sync::atomic::AtomicBool,
    /// Cooler target temperature in 0.1 °C (mirrors `OPTION_TECTARGET`).
    target_temp_tenths: std::sync::Mutex<i16>,
}

#[cfg(feature = "simulation")]
impl Camera {
    pub(crate) fn open_by_index(_index: u32, info: CameraInfo) -> Result<Self> {
        Ok(Self {
            info,
            sim: SimState {
                events: std::sync::Mutex::new(std::collections::VecDeque::new()),
                started: std::sync::atomic::AtomicBool::new(false),
                gain: std::sync::Mutex::new(SIM_GAIN_DEFAULT),
                black_level: std::sync::Mutex::new(0),
                cooler_on: std::sync::atomic::AtomicBool::new(false),
                target_temp_tenths: std::sync::Mutex::new(0),
            },
        })
    }

    /// No-op in simulation (records nothing); succeeds.
    pub fn set_exposure_time_us(&self, _micros: u32) -> Result<()> {
        Ok(())
    }

    /// Records the simulated analog gain (read back by [`gain_percent`]).
    pub fn set_gain_percent(&self, percent: u16) -> Result<()> {
        *self
            .sim
            .gain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = percent;
        Ok(())
    }

    /// The current simulated analog gain in percent.
    pub fn gain_percent(&self) -> Result<u16> {
        Ok(*self
            .sim
            .gain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    /// The simulated analog-gain range in percent (100 = 1.0× … 1000 = 10×).
    pub fn gain_range(&self) -> Result<(u16, u16)> {
        Ok((SIM_GAIN_DEFAULT, SIM_GAIN_MAX))
    }

    /// The simulated exposure-time range in microseconds (100 µs … 3600 s).
    pub fn exposure_range_us(&self) -> Result<(u32, u32)> {
        Ok((SIM_EXPOSURE_MIN_US, SIM_EXPOSURE_MAX_US))
    }

    /// The current simulated black level.
    pub fn black_level(&self) -> Result<i32> {
        Ok(*self
            .sim
            .black_level
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    /// Records the simulated black level.
    pub fn set_black_level(&self, value: i32) -> Result<()> {
        *self
            .sim
            .black_level
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = value;
        Ok(())
    }

    /// Records the simulated cooler on/off state.
    pub fn set_cooler(&self, on: bool) -> Result<()> {
        self.sim
            .cooler_on
            .store(on, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// Whether the simulated cooler is on.
    pub fn cooler_on(&self) -> Result<bool> {
        Ok(self
            .sim
            .cooler_on
            .load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Simulated cooler power: a clean 60 % when on, 0 % when off.
    pub fn cooler_power_percent(&self) -> Result<u32> {
        Ok(
            if self
                .sim
                .cooler_on
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                60
            } else {
                0
            },
        )
    }

    /// Records the simulated cooler target temperature in 0.1 °C units.
    pub fn set_target_temperature_tenths(&self, tenths: i16) -> Result<()> {
        *self
            .sim
            .target_temp_tenths
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = tenths;
        Ok(())
    }

    /// The simulated cooler target temperature in 0.1 °C units (defaults to 0 °C
    /// before any setpoint is written, mirroring a real model's power-on default).
    pub fn target_temperature_tenths(&self) -> Result<i16> {
        Ok(*self
            .sim
            .target_temp_tenths
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    /// No-op in simulation; succeeds (the SDK would time the ST4 pulse).
    pub fn st4_pulse_guide(&self, _direction: GuideDirection, _duration_ms: u32) -> Result<()> {
        Ok(())
    }

    /// No-op in simulation; succeeds.
    pub fn set_roi(&self, _x: u32, _y: u32, _width: u32, _height: u32) -> Result<()> {
        Ok(())
    }

    /// No-op in simulation; succeeds.
    pub fn set_option(&self, _option: u32, _value: i32) -> Result<()> {
        Ok(())
    }

    /// Returns a fixed simulated sensor temperature (25.0 °C, in 0.1 °C units).
    pub fn temperature_tenths_c(&self) -> Result<i16> {
        Ok(250)
    }

    /// No-op in simulation; succeeds.
    pub fn enable_trigger_mode(&self) -> Result<()> {
        Ok(())
    }

    /// Marks the simulated pull session active.
    pub fn start_pull_mode(&mut self) -> Result<()> {
        self.sim
            .started
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// Queues a simulated still-frame event.
    pub fn trigger_single(&self) -> Result<()> {
        self.sim
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(sys::TOUPCAM_EVENT_STILLIMAGE);
        Ok(())
    }

    /// Returns the next queued simulated event, or [`SdkError::Timeout`] if none.
    pub fn wait_for_event(&self, _timeout: Duration) -> Result<Event> {
        let next = self
            .sim
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front();
        match next {
            Some(code) => Ok(Event::from_code(code)),
            None => Err(Error::Sdk(SdkError::Timeout)),
        }
    }

    /// Fabricates a frame of simulated sensor noise.
    pub fn pull_image(&self, width: u32, height: u32, bits: u32) -> Result<Frame> {
        let bytes_per_px = (bits as usize).div_ceil(8).max(1);
        let mut data = vec![0u8; width as usize * height as usize * bytes_per_px];
        crate::simulation::fill_noise(&mut data);
        Ok(Frame {
            width,
            height,
            bits,
            data,
        })
    }

    /// Ends the simulated pull session and clears any queued events.
    pub fn stop(&mut self) -> Result<()> {
        self.sim
            .started
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.sim
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        Ok(())
    }

    /// Whether a simulated pull session is active.
    #[must_use]
    pub fn is_pulling(&self) -> bool {
        self.sim.started.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(all(test, feature = "simulation"))]
mod tests {
    use super::*;

    fn sim_info() -> CameraInfo {
        CameraInfo {
            id: "sim-0".to_owned(),
            display_name: "Simulated ToupTek Camera".to_owned(),
            model_name: "ToupTek Simulator".to_owned(),
            flag: u64::from(sys::TOUPCAM_FLAG_TEC)
                | u64::from(sys::TOUPCAM_FLAG_ST4)
                | u64::from(sys::TOUPCAM_FLAG_MONO)
                | u64::from(sys::TOUPCAM_FLAG_BLACKLEVEL),
            pixel_size_x: 3.76,
            pixel_size_y: 3.76,
            max_width: 6248,
            max_height: 4176,
            bit_depth: 16,
            is_color: false,
            supported_bins: vec![1, 2, 3, 4],
        }
    }

    #[test]
    fn has_tec_and_st4_read_the_flags() {
        let info = sim_info();
        assert!(info.has_tec());
        assert!(info.has_st4());
    }

    #[test]
    fn gain_and_black_level_round_trip_in_simulation() {
        let cam = Camera::open_by_index(0, sim_info()).unwrap();
        let (min, max) = cam.gain_range().unwrap();
        assert!(min <= max);
        cam.set_gain_percent(max).unwrap();
        assert_eq!(cam.gain_percent().unwrap(), max);
        cam.set_black_level(123).unwrap();
        assert_eq!(cam.black_level().unwrap(), 123);
        // Cooler + exposure-range simulation.
        assert!(!cam.cooler_on().unwrap());
        cam.set_cooler(true).unwrap();
        assert!(cam.cooler_on().unwrap());
        assert!((0..=100).contains(&cam.cooler_power_percent().unwrap()));
        let (emin, emax) = cam.exposure_range_us().unwrap();
        assert!(emin < emax);
    }

    #[test]
    fn trigger_then_pull_a_simulated_frame() {
        let mut cam = Camera::open_by_index(0, sim_info()).unwrap();
        cam.enable_trigger_mode().unwrap();
        cam.start_pull_mode().unwrap();
        assert!(cam.is_pulling());

        cam.trigger_single().unwrap();
        let event = cam.wait_for_event(Duration::from_secs(1)).unwrap();
        assert_eq!(event, Event::StillImage);

        let frame = cam.pull_image(64, 48, 16).unwrap();
        assert_eq!(frame.width, 64);
        assert_eq!(frame.height, 48);
        assert_eq!(frame.data.len(), 64 * 48 * 2);
        assert!(frame.data.iter().any(|&b| b != 0));

        cam.stop().unwrap();
        assert!(!cam.is_pulling());
    }

    #[test]
    fn wait_without_trigger_times_out() {
        let cam = Camera::open_by_index(0, sim_info()).unwrap();
        assert_eq!(
            cam.wait_for_event(Duration::from_millis(1)).unwrap_err(),
            Error::Sdk(SdkError::Timeout)
        );
    }
}
