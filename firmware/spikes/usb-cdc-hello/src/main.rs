use anyhow::Result;
use esp_idf_svc::hal::prelude::Peripherals;
use esp_idf_svc::log::EspLogger;
use log::info;
use std::thread;
use std::time::Duration;

fn main() -> Result<()> {
    // ESP-IDF startup boilerplate — required for std-targeted Rust.
    esp_idf_svc::sys::link_patches();
    EspLogger::initialize_default();

    let _peripherals = Peripherals::take()?;
    info!("USB-CDC spike booting on ESP32-S3");

    // Phase 0 goal: open the Star Adventurer GTi's USB-CDC interface as USB
    // host, write `:e1\r`, read the response, log `=03300C\r`. See
    // ../../../docs/services/star-adventurer-gti.md §"Init handshake".
    //
    // Implementation outline — fill this in with the board on the bench:
    //
    // 1. Install the USB Host Library (esp_idf_sys::usb_host_install).
    // 2. Spawn a FreeRTOS task that loops on usb_host_lib_handle_events.
    // 3. Install the CDC-ACM class driver (cdc_acm_host_install).
    // 4. Open the mount's CDC-ACM device. The GTi enumerates through the
    //    STMicroelectronics USB-CDC composite stack; confirm VID/PID
    //    empirically with `lsusb` on a Linux host. Best guess: VID =
    //    0x0483 (STMicroelectronics), PID = TBD.
    // 5. Register an RX callback in the device config that pushes bytes
    //    into a queue/event group.
    // 6. cdc_acm_host_data_tx_blocking(handle, b":e1\r", 4, 1000).
    // 7. Drain the RX queue until `\r`, log the bytes as ASCII.
    //
    // Expected output: `=03300C\r` — mount type 0x03, firmware version
    // 0x30.0x0C. Anything else (timeout, NAKs, wrong VID/PID) means the
    // spike is teaching us something; iterate.
    //
    // ESP-IDF C reference for the sequence above:
    //   https://github.com/espressif/esp-idf/tree/v5.3.1/examples/peripherals/usb/host/cdc/cdc_acm_host

    // Heartbeat so the UART monitor confirms the firmware is alive while
    // the USB host code is still TODO.
    loop {
        info!("alive");
        thread::sleep(Duration::from_secs(1));
    }
}
