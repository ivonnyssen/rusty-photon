# Phase 0 spike: USB-CDC host on ESP32-S3

Prove that an ESP32-S3 can open the Star Adventurer GTi's USB-CDC
interface as USB host, write `:e1\r`, and read back `=03300C\r`.
Nothing else.

Phase 0 of [`docs/plans/star-adventurer-gti-embedded.md`](../../../docs/plans/star-adventurer-gti-embedded.md).
This crate is throwaway — deleted at the end of Phase 1.

## Status

**Scaffolded, not yet exercised against hardware.** The firmware boots
and logs a heartbeat over UART. The USB-CDC host sequence itself is
laid out as a TODO in `src/main.rs` and is the actual work this spike
is meant to validate.

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

## Next step — the actual spike

Open `src/main.rs`. There is a step-by-step outline of the USB Host
Library + CDC-ACM class driver sequence to fill in. Iterate against
the bench until you see `=03300C\r` in the monitor (mount type 0x03,
firmware version 0x30.0x0C — same banner the desktop driver logs at
connect time).

**Fallback path:** if the `esp-idf-svc` Rust wrapping of
`cdc_acm_host_*` is incomplete, drop down to raw FFI through
`esp_idf_sys`. The symbols come from the ESP-IDF `usb_host_cdc_acm`
component which `sdkconfig.defaults` already pulls in. C reference:
https://github.com/espressif/esp-idf/tree/v5.3.1/examples/peripherals/usb/host/cdc/cdc_acm_host

**Escalation:** if the USB Host Library itself fights (device not
enumerating, no descriptor, IRQ storms), don't burn more than a day
chasing it before switching strategy:

1. **TinyUSB host via C bindings.** TinyUSB has a more compact CDC-ACM
   host driver that some users find easier to get going than ESP-IDF's
   native one. Trade-off: pulls in another build dependency.
2. **Board change.** Move the spike to the RP2350 (Pico 2 W) target.
   The plan time-boxes Phase 0 to 2 days total; if neither USB host
   path works on either board within that window, revisit the
   architecture (USB-to-WiFi bridge, dedicated USB host MCU, etc.).
