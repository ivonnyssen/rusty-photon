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

## Status — Phase E (full `Camera`) + Phase F gate (ConformU clean)

The full driver is live against the simulation backend. The service enumerates
(real or simulated), registers an ASCOM `Device` + `Camera`, and binds `:11123`,
implementing the whole `Camera` surface over the `backend.rs` `CameraHandle` seam:
the trigger-mode exposure state machine (the `touptek-rs` callback→pull bridge,
abort, cancel-on-disconnect), digital binning, ROI (even + ≥ 8 validation),
gain/offset, cooling, RAW16 readout + `[x][y]` transpose, sensor type, and the
asynchronous ST4 `PulseGuide`. All **60 BDD scenarios** and ~55 unit tests run
green with **no SDK** — the unit-test mock seam forces the paths the simulation
cannot (C2/C4/E9/GO1/K1/PG2). **ConformU** (`alpacaprotocol` + `conformance`)
validates the simulated driver to **0 errors / 0 issues**, and is wired into the
nightly `conformu.yml` (skip-link, no SDK provisioned). Roadmap (see the plan):

- **Phase D** — design doc + BDD feature files. ✅ landed.
- **Phase E** — full `Camera` over `touptek-rs`; `@wip` tags removed. ✅ landed.
- **Phase F (gate)** — ConformU to 0 errors / 0 issues on the sim backend +
  `conformu.yml` wiring. ✅ landed.
- **Phase F (real link)** — `install-toupcam-sdk` is **wired into `native.yml`**
  on all three OSes: Linux/macOS link the INDI-mirror blob, Windows links
  ToupTek's own vendor SDK (the action's default rolling `download.php?soft=toupcamsdk`
  URL → `win/x64/toupcam.{lib,dll}`). The Linux + macOS legs are CI-green; the
  Windows leg is the newest addition. The real Bazel `rust_binary` still remains. ⏳
- **Phase G** — the `rp` `CameraConfig` consumer + real-hardware ConformU on each
  target platform. ⏳ needs hardware.

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
  until the (now-drafted) `.github/actions/install-toupcam-sdk` is wired into a
  real-link workflow (Phase F provisioning). That action installs `libtoupcam`
  from the INDI mirror (Linux/macOS) under the linker name `-ltoupcam` resolves;
  with it staged, `cargo build -p touptek-camera` real-links (verified on aarch64).

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
| `conformu` | Enables `mock`; builds + runs the ConformU compliance test (0 errors / 0 issues on the sim backend; wired into `conformu.yml`). |
