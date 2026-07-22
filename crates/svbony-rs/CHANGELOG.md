# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Initial repository scaffold for `svbony-rs` (safe wrapper) and
  `libsvbony-sys` (raw FFI), sibling to `qhyccd-rs` and `zwo-rs`, vendored
  first-party from day one (no external-repo detour).
- `libsvbony-sys`: hand-written `extern "C"` bindings (no bindgen — SVBony's
  SDK header carries no license text) for `SVBCameraSDK.h` (SDK 1.13.4);
  per-OS link directives for `libSVBCameraSDK` + `libusb-1.0`, gated by
  `SVBONY_SKIP_NATIVE_LINK`; no Windows support (indi-3rdparty declares it
  unsupported).
- `svbony-rs`: `Sdk` entry point + `simulation` feature; the `Camera` handle
  (open/close, enumeration with pre-open serial identity, property/capability
  queries, ROI, controls with typed gain/exposure/black-level/cooling
  wrappers, camera mode, the video-capture exposure model incl. the
  soft-trigger flow, ST4 guiding, pixel size), backing the future
  `svbony-camera` ASCOM Alpaca driver.
- Simulation backend: fabricated `SV605CC-Simulated` camera, seeded
  xorshift64 frame noise fill, a simulated soft-trigger video-capture state
  machine, and a poll-based cooling ramp.
- Dual MIT/Apache-2.0 licensing.

[Unreleased]: https://github.com/ivonnyssen/rusty-photon/commits/main/crates/svbony-rs
