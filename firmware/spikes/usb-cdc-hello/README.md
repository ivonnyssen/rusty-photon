# Phase 0 spike: USB-CDC host on ESP32-S3

Prove that an ESP32-S3 can open the Star Adventurer GTi's USB-CDC
interface as USB host, write `:e1\r`, and read back `=03300C\r`.
Nothing else.

Phase 0 of [`docs/plans/star-adventurer-gti-embedded.md`](../../../docs/plans/star-adventurer-gti-embedded.md).
This crate is throwaway — deleted at the end of Phase 1.

## Status

**Phase 0 done — round-trip confirmed against real hardware on
2026-05-15.** Captured boot log:

```
I (366) usb_cdc_hello: USB-CDC spike booting on ESP32-S3
I (366) usb_cdc_hello: installing USB host library
I (406) usb_cdc_hello: installing cdc_acm host driver
I (406) usb_cdc_hello: opening GTi (VID=0483 PID=5740)
I (806) usb_cdc_hello: → ":e1\r"
I (806) usb_cdc_hello: ← "=03300C"
I (806) usb_cdc_hello: alive
```

Boot to first round-trip: **806 ms**. Mount reports type `0x03`
(Star Adventurer GTi), firmware `0x30.0x0C` — same banner the desktop
driver gets. The three Phase 0 unknowns are answered:

- USB Host Library on ESP32-S3 works.
- The managed `espressif/usb_host_cdc_acm` component links cleanly
  when threaded through a local extras component
  (`components/cdc_acm_pull/`, see CMakeLists / idf_component.yml).
- The STM32 VCP on the GTi enumerates at full-speed and accepts bulk
  TX/RX at 115200 8N1 with DTR=RTS=1. No reset gymnastics required.

This crate stays around for regression checks but the real work moves
to Phase 1 (`no_std`-ifying `skywatcher-motor-protocol`).

## Hardware

- Espressif **ESP32-S3-DevKitC-1, N16R8** (16 MB flash, 8 MB PSRAM).
  Two USB-C ports: one labelled `UART` for programming + console, one
  labelled `USB` for the native USB-OTG (host to the mount).
- USB-C cable between the board's `USB` port and the mount's USB-C
  port. The dev board sources VBUS as host — the GTi has its own
  power supply, so this should be fine.
- USB-C cable between the board's `UART` port and your host machine
  (for flashing + monitor).

## One-time tooling install

```bash
# Espressif's Rust toolchain manager. Installs the Xtensa rustc fork,
# LLVM with Xtensa backend, and GCC for Xtensa. ~2 GB on disk.
cargo install espup --locked
espup install

# Source the env in your shell rc — adds the Xtensa toolchain to PATH.
echo 'source $HOME/export-esp.sh' >> ~/.bashrc
source $HOME/export-esp.sh

# Flashing + serial monitor + linker shim.
cargo install espflash --locked
cargo install ldproxy --locked
```

The first `cargo build` will also download and build the full ESP-IDF
C framework (~15 min one-time, cached afterwards in `~/.espressif`).

## Build, flash, monitor

```bash
cd firmware/spikes/usb-cdc-hello
cargo run --release
```

`espflash` talks to the board over the `UART` USB-C port. Expect a few
seconds of flashing, then the monitor opens. With the current scaffold
you should see:

```
I (350) usb_cdc_hello: USB-CDC spike booting on ESP32-S3
I (1351) usb_cdc_hello: alive
I (2351) usb_cdc_hello: alive
…
```

If you don't see the boot banner, check `dmesg` (Linux) or Device
Manager (Windows) for the `UART` port's `/dev/ttyUSB*` / `COMx` mapping,
and confirm `espflash` is talking to the right one (`espflash flash
--port /dev/ttyUSB0` to pin it).

## Bench procedure

1. Disconnect the mount from your workstation (only one USB host at a
   time — when the ESP32 enumerates the mount, your PC can't be).
2. Connect the dev board's **USB** (OTG) port to the mount with a
   USB-C cable. The mount is self-powered, so the board sourcing VBUS
   is harmless.
3. Connect the dev board's **UART** USB-C port to your workstation —
   the monitor reads from this one.
4. `cargo run --release` flashes and opens the monitor. Expected log
   sequence on success:

   ```
   I (350)  usb_cdc_hello: USB-CDC spike booting on ESP32-S3
   I (360)  usb_cdc_hello: installing USB host library
   I (370)  usb_cdc_hello: installing cdc_acm host driver
   I (380)  usb_cdc_hello: opening GTi (VID=0483 PID=5740)
   I (520)  usb_cdc_hello: → ":e1\r"
   I (540)  usb_cdc_hello: ← "=03300C"
   I (545)  usb_cdc_hello: alive
   …
   ```

5. The reply `=03300C` is the success signal: mount type `0x03` (Star
   Adventurer GTi), firmware `0x30.0x0C`. Anything else is data the
   spike is teaching us — capture the log and iterate.

## Failure-mode crib sheet

| Symptom | Likely cause | First thing to try |
|---|---|---|
| `cdc_acm_host_open failed: rc=0x103` (`ESP_ERR_TIMEOUT`) | device not enumerated | check the cable goes into the **USB** port not the **UART** one; check `lsusb` from the workstation still sees `0483:5740` after you re-attach |
| `usb_host_install failed: rc=0x102` (`ESP_ERR_INVALID_STATE`) | host stack already up | this should not happen on a cold boot — power-cycle the board |
| `cdc_acm_host_data_tx_blocking failed: rc=0x103` | tx endpoint NAKed for too long | the mount's USB stack may want DTR/RTS held low first; flip the `set_control_line_state` arguments |
| TX OK but `no response within 2s` | RX callback never fired, or terminator mismatch | print the raw RX-buf bytes in the warn message — already does that |
| Garbled / partial response | line coding mismatch, mount expects 9600 | bump `GTI_BAUD` to `9600` in `src/main.rs` |

## Escalation — don't burn more than ~2 days

1. **Switch to TinyUSB host via C bindings.** TinyUSB has a more
   compact CDC-ACM host driver that some users find easier to get going
   than ESP-IDF's `cdc_acm_host`.
2. **Board change to RP2350 (Pico 2 W).** Plan-doc Phase 9 already
   lists this as a validation target; promote it to primary if the
   ESP32 USB-host path is uncooperative.
3. **Architecture change.** USB-to-WiFi bridge dongle, dedicated USB
   host MCU front-ending the ESP32, etc. Document and revisit the
   plan-doc Hardware section.

## C reference

When the Rust FFI fights, the closest working code is the ESP-IDF C
example. Same API, no Rust glue:
https://github.com/espressif/esp-idf/tree/v5.3.1/examples/peripherals/usb/host/cdc/cdc_acm_host
