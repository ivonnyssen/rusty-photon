# zwo-camera

ASCOM Alpaca **Camera** (and, later, **FilterWheel**) driver for ZWO ASI
hardware, served on port **11122**. It is the ZWO analogue of `qhy-camera`,
built on the author-maintained [`zwo-rs`](https://github.com/ivonnyssen/zwo-rs)
FFI crate.

See [`docs/services/zwo-camera.md`](../../docs/services/zwo-camera.md) for the
full design, [`docs/plans/zwo-driver.md`](../../docs/plans/zwo-driver.md) for the
decision record, and [ADR-008](../../docs/decisions/008-zwo-camera-native-sdk-ffi.md)
for the native-SDK / FFI-crate decision.

## Status — Phase C (Track A) scaffold

This crate currently stands up a **bare** Alpaca server: it enumerates every
connected ASI camera via `zwo-rs` and registers each as a minimal `Camera`
device (identity + cached sensor geometry; most of the imaging surface is the
trait's `NOT_IMPLEMENTED` default). Its purpose is to prove the build/link chain
and the CI / Bazel gating around the native SDK **before** the full device-trait
work. Roadmap:

- **Phase E** — full `Camera`: exposure state machine, ROI/binning, gain/offset,
  cooling, readout modes, ST4 pulse guiding, config actions.
- **Phase F** — EFW `FilterWheel` (gated on `filterwheel.enabled`).
- **Phase G** — BDD + ConformU on the simulation backend; `rp` consumer.

The BDD feature files under `tests/features/` are the Phase E–G contract and are
tagged `@wip` until their scenarios are implemented.

## Native dependency (the crux)

`zwo-rs`'s `libzwo-sys` links the ZWO ASI/EFW SDK (`libASICamera2` +
`libEFWFilter` + `libusb-1.0`) **unconditionally**. Consequences:

- **Every machine that compiles this package needs the SDK installed** — dev
  laptops, CI runners, Bazel actions — not just machines with a camera attached.
- The `simulation` feature makes the build **camera-free, not SDK-free**: it
  fabricates frames/EFW state at runtime, but the native SDK is still linked.
- `cargo build -p zwo-camera` is **expected to fail to link without the SDK**.
  Devs without it use the rest of the workspace normally; `cargo rail`'s
  affected-packages-only mode builds this crate only when its files change.

## Building locally

```sh
# 1. Install the MIT-licensed ZWO SDK (Linux x86_64 = x64; aarch64 = armv8;
#    macOS arm64 = mac_arm64). Mirrors .github/actions/install-zwo-sdk — pulls
#    ZWO's prebuilt blobs from the INDI mirror.
#    (Linux) also: sudo apt-get install libusb-1.0-0-dev clang libclang-dev
BASE=https://github.com/indilib/indi-3rdparty/raw/master/libasi
sudo install -d /usr/local/lib /usr/local/include
# Headers + license. bindgen actually reads the copies vendored inside
# libzwo-sys, so these are for completeness (and to keep the MIT notice with
# the libs), matching the CI/INDI installer.
for h in ASICamera2.h EFW_filter.h EAF_focuser.h license.txt; do
  sudo curl -fsSL "$BASE/$h" -o "/usr/local/include/$h"
done
# Shared libraries (INDI's .bin == ZWO's upstream .so), under the linker name.
sudo curl -fsSL "$BASE/x64/libASICamera2.bin" -o /usr/local/lib/libASICamera2.so
sudo curl -fsSL "$BASE/x64/libEFWFilter.bin"  -o /usr/local/lib/libEFWFilter.so
sudo ldconfig

# 2. bindgen needs libclang; point LIBCLANG_PATH at it if not auto-found
#    (e.g. /usr/lib64 on Fedora, /usr/lib/$(uname -m)-linux-gnu on Debian).
export LIBCLANG_PATH=/usr/lib64

cargo build -p zwo-camera
cargo run  -p zwo-camera --features simulation -- --port 11122
```

## CLI

| Flag | Description |
|---|---|
| `--config <path>` | Config file (default: `~/.config/rusty-photon/zwo-camera.json`). |
| `--port <port>` | Override `server.port`. |
| `--log-level <level>` | `trace`\|`debug`\|`info`\|`warn`\|`error` (default `info`). |

## Configuration

```jsonc
{
  "devices": {},          // per-serial name/description/filter_names overrides (Phase E+)
  "filterwheel": { "enabled": false },  // register discovered EFWs (Phase F)
  "server": { "port": 11122 }
}
```

## Features

| Feature | Effect |
|---|---|
| `simulation` | Forwards to `zwo-rs/simulation`: a fabricated `ASI2600MM-Pro-Simulated` camera. Removes the camera, **not** the SDK link. |
| `mock` | Alias for `simulation` (the cross-driver test-backend name). |
