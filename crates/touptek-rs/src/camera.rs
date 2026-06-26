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
}

impl CameraInfo {
    /// Whether the model reports a thermo-electric cooler (`TOUPCAM_FLAG_TEC`).
    #[must_use]
    pub fn has_tec(&self) -> bool {
        self.flag & u64::from(sys::TOUPCAM_FLAG_TEC) != 0
    }
}

/// A pulled sensor frame. `data` is the raw little-endian pixel buffer; its
/// layout depends on the configured pixel format / bit depth.
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

/// In-Rust state backing a simulated camera.
#[cfg(feature = "simulation")]
struct SimState {
    /// Pending pull-mode events (pushed by `trigger_single`, drained by
    /// `wait_for_event`).
    events: std::sync::Mutex<std::collections::VecDeque<u32>>,
    /// Whether a pull session is active.
    started: std::sync::atomic::AtomicBool,
}

#[cfg(feature = "simulation")]
impl Camera {
    pub(crate) fn open_by_index(_index: u32, info: CameraInfo) -> Result<Self> {
        Ok(Self {
            info,
            sim: SimState {
                events: std::sync::Mutex::new(std::collections::VecDeque::new()),
                started: std::sync::atomic::AtomicBool::new(false),
            },
        })
    }

    /// No-op in simulation (records nothing); succeeds.
    pub fn set_exposure_time_us(&self, _micros: u32) -> Result<()> {
        Ok(())
    }

    /// No-op in simulation; succeeds.
    pub fn set_gain_percent(&self, _percent: u16) -> Result<()> {
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
            flag: u64::from(sys::TOUPCAM_FLAG_TEC),
            pixel_size_x: 3.76,
            pixel_size_y: 3.76,
        }
    }

    #[test]
    fn has_tec_reads_the_flag() {
        assert!(sim_info().has_tec());
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
