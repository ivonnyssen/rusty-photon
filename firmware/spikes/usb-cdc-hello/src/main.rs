//! Phase 0 spike: USB-CDC host on ESP32-S3, talking to a Star Adventurer GTi.
//!
//! Boot order:
//!   1. Install the USB Host Library and spawn an OS thread to pump events.
//!   2. Install the `cdc_acm_host` managed component's driver.
//!   3. Open the mount by VID/PID `0483:5740` (STM32 Virtual COM Port).
//!   4. Configure 115200 8N1 line coding + assert DTR/RTS.
//!   5. Send `:e1\r` (firmware-version query).
//!   6. The RX callback buffers bytes until `\r`, then logs the response —
//!      expect `=03300C\r` (mount type 0x03, firmware 0x30.0x0C).
//!
//! The CDC-ACM host driver's Rust API isn't wrapped by `esp-idf-svc`, so the
//! driver entry points are declared as `extern "C"` here. The managed
//! component is pulled in via `idf_component.yml`.

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Result};
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::sys::{
    self, usb_host_config_t, usb_host_device_free_all, usb_host_install,
    usb_host_lib_handle_events, ESP_OK, USB_HOST_LIB_EVENT_FLAGS_ALL_FREE,
    USB_HOST_LIB_EVENT_FLAGS_NO_CLIENTS,
};
use log::{error, info, warn};

const GTI_VID: u16 = 0x0483;
const GTI_PID: u16 = 0x5740;
const GTI_BAUD: u32 = 115_200;
const QUERY: &[u8] = b":e1\r";

#[repr(C)]
struct CdcDev {
    _opaque: [u8; 0],
}
type CdcDevHdl = *mut CdcDev;

#[repr(C)]
#[derive(Clone, Copy)]
struct CdcAcmHostDriverConfig {
    driver_task_stack_size: usize,
    driver_task_priority: u32,
    x_core_id: i32,
    new_dev_cb: Option<extern "C" fn(*const c_void, *mut c_void)>,
}

#[repr(C)]
struct CdcAcmHostDeviceConfig {
    connection_timeout_ms: u32,
    out_buffer_size: usize,
    in_buffer_size: usize,
    event_cb: Option<extern "C" fn(*const c_void, *mut c_void)>,
    data_cb: Option<extern "C" fn(*const u8, usize, *mut c_void) -> bool>,
    user_arg: *mut c_void,
}

// Field names mirror the USB CDC PSTN spec (Table 17: Line Coding Structure)
// so cross-referencing the C `cdc_acm_host.h` header stays trivial.
#[repr(C, packed)]
#[allow(non_snake_case)]
struct CdcAcmLineCoding {
    dwDTERate: u32,
    bCharFormat: u8,
    bParityType: u8,
    bDataBits: u8,
}

extern "C" {
    fn cdc_acm_host_install(driver_config: *const CdcAcmHostDriverConfig) -> i32;
    // The public C name `cdc_acm_host_open` is a `_Generic` macro that
    // dispatches between the legacy 5-arg form and a newer struct-based
    // form. Bind directly to the symbol the legacy dispatch resolves to.
    #[link_name = "cdc_acm_host_open_v1_dispatch"]
    fn cdc_acm_host_open(
        vid: u16,
        pid: u16,
        interface_idx: u8,
        dev_config: *const CdcAcmHostDeviceConfig,
        cdc_hdl_ret: *mut CdcDevHdl,
    ) -> i32;
    fn cdc_acm_host_data_tx_blocking(
        cdc_hdl: CdcDevHdl,
        data: *const u8,
        data_len: usize,
        timeout_ms: u32,
    ) -> i32;
    fn cdc_acm_host_line_coding_set(
        cdc_hdl: CdcDevHdl,
        line_coding: *const CdcAcmLineCoding,
    ) -> i32;
    fn cdc_acm_host_set_control_line_state(cdc_hdl: CdcDevHdl, dtr: bool, rts: bool) -> i32;
}

fn check(name: &str, rc: i32) -> Result<()> {
    if rc == ESP_OK as i32 {
        Ok(())
    } else {
        Err(anyhow!("{} failed: rc=0x{:x}", name, rc as u32))
    }
}

/// RX accumulator. The cdc_acm data callback runs in a FreeRTOS task spawned
/// by the driver; we just append bytes here and let the main thread log
/// whenever the running buffer ends in `\r`.
static RX_BUF: OnceLock<Mutex<Vec<u8>>> = OnceLock::new();

fn rx_buf() -> &'static Mutex<Vec<u8>> {
    RX_BUF.get_or_init(|| Mutex::new(Vec::with_capacity(64)))
}

extern "C" fn on_rx(data: *const u8, data_len: usize, _user_arg: *mut c_void) -> bool {
    let slice = unsafe { std::slice::from_raw_parts(data, data_len) };
    if let Ok(mut buf) = rx_buf().lock() {
        buf.extend_from_slice(slice);
    }
    true
}

extern "C" fn on_event(_event: *const c_void, _user_ctx: *mut c_void) {
    // Driver-level event (error, serial state change, disconnect). We don't
    // act on these in the spike — just note them in the log so we can spot
    // disconnects.
    warn!("cdc_acm event callback fired");
}

fn drain_response() -> Option<Vec<u8>> {
    let mut buf = rx_buf().lock().ok()?;
    let end = buf.iter().position(|&b| b == b'\r')?;
    let mut frame: Vec<u8> = buf.drain(..=end).collect();
    if frame.last() == Some(&b'\r') {
        frame.pop();
    }
    Some(frame)
}

fn run_spike() -> Result<()> {
    info!("installing USB host library");
    // Zero-init then set the fields we care about. The struct layout drifts
    // across ESP-IDF versions (e.g. v5.3 added `root_port_unpowered`), so
    // avoid pinning to a specific field set.
    let host_cfg = usb_host_config_t {
        intr_flags: sys::ESP_INTR_FLAG_LEVEL1 as i32,
        ..unsafe { std::mem::zeroed() }
    };
    check("usb_host_install", unsafe { usb_host_install(&host_cfg) })?;

    // Pump USB Host Library events on a dedicated OS thread. esp-idf-svc's
    // std::thread maps onto FreeRTOS tasks under the hood.
    thread::Builder::new()
        .name("usb_host_evt".into())
        .stack_size(4096)
        .spawn(|| loop {
            let mut event_flags: u32 = 0;
            let rc = unsafe { usb_host_lib_handle_events(u32::MAX, &mut event_flags) };
            if rc != ESP_OK as i32 {
                error!("usb_host_lib_handle_events: rc={rc}");
                continue;
            }
            if event_flags & USB_HOST_LIB_EVENT_FLAGS_NO_CLIENTS != 0 {
                info!("USB host: no clients, releasing devices");
                unsafe { usb_host_device_free_all() };
            }
            if event_flags & USB_HOST_LIB_EVENT_FLAGS_ALL_FREE != 0 {
                info!("USB host: all devices free");
            }
        })?;

    info!("installing cdc_acm host driver");
    check("cdc_acm_host_install", unsafe {
        cdc_acm_host_install(std::ptr::null())
    })?;

    info!("opening GTi (VID={GTI_VID:04x} PID={GTI_PID:04x})");
    let dev_cfg = CdcAcmHostDeviceConfig {
        connection_timeout_ms: 5_000,
        out_buffer_size: 64,
        in_buffer_size: 64,
        event_cb: Some(on_event),
        data_cb: Some(on_rx),
        user_arg: std::ptr::null_mut(),
    };
    let mut cdc_hdl: CdcDevHdl = std::ptr::null_mut();
    check("cdc_acm_host_open", unsafe {
        cdc_acm_host_open(GTI_VID, GTI_PID, 0, &dev_cfg, &mut cdc_hdl)
    })?;

    let line_coding = CdcAcmLineCoding {
        dwDTERate: GTI_BAUD,
        bCharFormat: 0, // 1 stop bit
        bParityType: 0, // none
        bDataBits: 8,
    };
    check("cdc_acm_host_line_coding_set", unsafe {
        cdc_acm_host_line_coding_set(cdc_hdl, &line_coding)
    })?;
    check("cdc_acm_host_set_control_line_state", unsafe {
        cdc_acm_host_set_control_line_state(cdc_hdl, true, true)
    })?;

    info!("→ {:?}", String::from_utf8_lossy(QUERY));
    check("cdc_acm_host_data_tx_blocking", unsafe {
        cdc_acm_host_data_tx_blocking(cdc_hdl, QUERY.as_ptr(), QUERY.len(), 1_000)
    })?;

    // Wait up to 2 s for the response, then keep heartbeating regardless so
    // the monitor stays useful when iterating.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if let Some(frame) = drain_response() {
            info!("← {:?}", String::from_utf8_lossy(&frame));
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    warn!(
        "no response within 2s — RX buf: {:?}",
        rx_buf().lock().map(|b| b.clone()).ok()
    );
    Ok(())
}

fn main() -> Result<()> {
    sys::link_patches();
    EspLogger::initialize_default();

    info!("USB-CDC spike booting on ESP32-S3");

    if let Err(e) = run_spike() {
        error!("spike failed: {e:#}");
    }

    loop {
        info!("alive");
        thread::sleep(Duration::from_secs(5));
    }
}
