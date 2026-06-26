# zwo-camera

ASCOM Alpaca **Camera** (and, later, **FilterWheel**) driver for ZWO ASI
hardware, served on port **11122**. It is the ZWO analogue of `qhy-camera`,
built on the author-maintained [`zwo-rs`](https://github.com/ivonnyssen/zwo-rs)
FFI crate.

See [`docs/services/zwo-camera.md`](../../docs/services/zwo-camera.md) for the
full design, [`docs/plans/zwo-driver.md`](../../docs/plans/zwo-driver.md) for the
decision record, and [ADR-008](../../docs/decisions/008-zwo-camera-native-sdk-ffi.md)
for the native-SDK / FFI-crate decision.

## Status — Phase E (full Camera) landed

This crate implements the full ASCOM `Device` + `Camera` surface over the
`zwo-rs` SDK seam (`backend.rs`): connection lifecycle, sensor geometry (with
`CameraXSize`/`CameraYSize` reported aligned so the binned full frame stays a
valid ASI ROI), symmetric binning, ROI with the ASI `%8`/`%2` alignment rules,
gain/offset, cooling, readout modes, asynchronous ST4 pulse guiding, and the
snap-mode exposure state
machine (start; abort *discards* / graceful stop *preserves*; `ImageArray`,
`CameraState`, `PercentCompleted`, mid-exposure `Error`), plus serial-derived
identity and the `config.get`/`apply`/`schema` actions. Validated by **45 unit
tests** (against the in-crate mock seam), **57 BDD scenarios**, and a full
**ConformU** pass (both `alpacaprotocol` and `conformance` suites). Roadmap:

- **Phase F** — EFW `FilterWheel` (gated on `filterwheel.enabled`).
- **Phase G** — mostly done: ConformU is wired into `conformu.yml` (per-service
  matrix, native ZWO SDK provisioned via `install-zwo-sdk`), and the nightly
  `native.yml` builds the real linked path on Linux/macOS/Windows. Remaining
  tail: the `rp` `CameraConfig` consumer.

The six camera BDD feature files under `tests/features/` are live;
`filter_wheel.feature` stays `@wip` for Phase F.

## Native dependency (the crux)

`zwo-rs`'s `libzwo-sys` links the ZWO ASI/EFW SDK (`libASICamera2` +
`libEFWFilter` + `libusb-1.0`) **unconditionally**. Consequences:

- **Every machine that compiles this package needs the SDK installed** — dev
  laptops, CI runners, Bazel actions — not just machines with a camera attached.
- The `simulation` feature makes the build **camera-free, not SDK-free**: it
  fabricates frames/EFW state at runtime, but the native SDK is still linked.
- The build **fails to link without the SDK**, so **install it before building**
  (see below). `bazel build //...` includes this package, so the SDK is a required
  local-dev prerequisite — CI and the Bazel actions install it the same way.

## Building locally

```sh
# 1. Install the MIT-licensed ZWO SDK (Linux x86_64 = x64; aarch64 = armv8;
#    macOS arm64 = mac_arm64). Mirrors .github/actions/install-zwo-sdk — pulls
#    ZWO's prebuilt blobs from the INDI mirror.
#    (Linux) also: sudo apt-get install libusb-1.0-0-dev clang libclang-dev
# Pinned to a commit SHA (not `master`) so the blobs match CI and the Pi runner;
# bump it in lockstep with .github/actions/install-zwo-sdk to adopt a newer SDK.
BASE=https://github.com/indilib/indi-3rdparty/raw/b0802f2/libasi
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

bazel build //services/zwo-camera/...
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
  "devices": {},          // per-serial name/description/filter_names overrides
  "filterwheel": { "enabled": false },  // register discovered EFWs (Phase F)
  "server": { "port": 11122 }
}
```

## Features

| Feature | Effect |
|---|---|
| `simulation` | Forwards to `zwo-rs/simulation`: a fabricated `ASI2600MM-Pro-Simulated` camera. Removes the camera, **not** the SDK link. |
| `mock` | Alias for `simulation` (the cross-driver test-backend name). |
| `conformu` | Enables `mock`; builds + runs the ConformU compliance test (`tests/conformu_integration.rs`). |
