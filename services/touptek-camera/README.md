# touptek-camera

ASCOM Alpaca **Camera** driver for ToupTek (and the OEM-rebrand family — Altair,
Omegon, Meade, Bresser, Mallincam, RisingCam/Ogma, SVBony, StarShootG, Nncam,
Tscam), served on port **11123**. It is the ToupTek analogue of `zwo-camera` /
`qhy-camera`, built on the author-maintained
[`touptek-rs`](../../crates/touptek-rs) FFI crate.

See [`docs/plans/touptek-driver.md`](../../docs/plans/touptek-driver.md) for the
decision record and [`docs/services/touptek-camera.md`](../../docs/services/touptek-camera.md)
for the full service design (the build-gating crux, the callback→blocking exposure
bridge, and the behavioural contracts the BDD suite encodes).

## Status — Phase D (design doc + BDD contract)

The bare service is live and the **design + test contract is committed**: the
design doc, the eight `tests/features/*.feature` files (committed `@wip`, so the
runner skips them until Phase E), the BDD harness driving the typed `ascom-alpaca`
Camera client, and the ConformU integration test. The Phase-C scaffold builds,
enumerates (real or simulated), registers a minimal ASCOM `Device` + `Camera`
(connection lifecycle, id-derived identity, sensor pixel size, cached sub-frame
origin), and binds `:11123`. The full `Camera` surface — exposure state machine
(trigger mode + the `touptek-rs` callback→pull bridge), digital binning, ROI,
gain/offset, cooling, RAW16 readout + transpose, sensor type, and ST4 `PulseGuide`
— is **Phase E**; those members fall back to the trait's `NotImplemented` defaults
until then. Roadmap (see the plan):

- **Phase D** — design doc + BDD feature files. ✅ landed.
- **Phase E** — full `Camera` over `touptek-rs` (remove the `@wip` tags as each feature turns green).
- **Phase F** — BDD + ConformU to 0 errors / 0 issues on the sim backend; wire the
  real-link `native.yml` / `conformu.yml` jobs.
- **Phase G** — the `rp` `CameraConfig` consumer + real-hardware ConformU on each
  target platform.

## Native dependency & build gating (the crux)

`touptek-rs`'s `libtoupcam-sys` links the proprietary ToupTek SDK (`libtoupcam` +
`libusb-1.0`/`libudev`) on the **real** FFI path. The build is gated so the
simulated path needs **no SDK**:

- The `simulation` feature makes the build **camera-free** (it fabricates frames),
  matching `zwo-camera`/`qhy-camera`.
- Unlike those, the **Bazel `_sim` chain additionally skip-links the SDK**: the
  `libtoupcam-sys` simulation build script runs with `TOUPCAM_SKIP_NATIVE_LINK=1`,
  so it emits no link directives. The simulated code references no `Toupcam_*`
  symbols, so the sim library / binary / unit-test **link cleanly with no SDK
  present** — i.e. `bazel test //...` needs nothing provisioned.
- The **real `touptek-camera_lib`** is an `rlib`, which defers linking, so it too
  builds without the SDK (it just compiles the real FFI code). The real
  **`rust_binary`** — the only target that actually links the SDK — is deferred
  until the `install-toupcam-sdk` CI action lands (Phase F provisioning).

## Building locally

```sh
# bindgen needs libclang (NOT the SDK). On Debian/Ubuntu:
#   sudo apt-get install clang libclang-dev libusb-1.0-0-dev
# Point LIBCLANG_PATH at it if not auto-found (e.g. /usr/lib/$(uname -m)-linux-gnu).

# The simulated path needs no ToupTek SDK:
bazel build //services/touptek-camera:touptek-camera_lib_sim
bazel test  //services/touptek-camera:touptek-camera_unit_test
cargo run -p touptek-camera --features simulation -- --port 11123

# The real link (real rust_binary) needs the SDK installed — Phase F.
```

## CLI

| Flag | Description |
|---|---|
| `--config <path>` | Config file (default: `~/.config/rusty-photon/touptek-camera.json`). |
| `--port <port>` | Override `server.port`. |
| `--log-level <level>` | `trace`\|`debug`\|`info`\|`warn`\|`error` (default `info`). |

## Configuration

```jsonc
{
  "devices": {},          // per-id name/description overrides
  "server": { "port": 11123 }
}
```

## Features

| Feature | Effect |
|---|---|
| `simulation` | Forwards to `touptek-rs/simulation`: a fabricated `Simulated ToupTek Camera`. Removes the camera; the Bazel `_sim` chain also skip-links the SDK. |
| `mock` | Alias for `simulation` (the cross-driver test-backend name). |
| `conformu` | Enables `mock`; builds + runs the ConformU compliance test (wired in Phase F). |
