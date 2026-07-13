# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `libqhyccd-sys`'s `build.rs` honors a `QHYCCD_SDK_DIR` override on macOS
  (the directory containing `libqhyccd.a`), mirroring the existing Windows
  and Linux branches, so builds can link an SDK staged outside
  `GITHUB_WORKSPACE` / `/usr/local/lib`.

### Changed

- **BREAKING:** error handling is now fully typed. Fallible `Sdk` / `Camera` /
  `FilterWheel` methods return `qhyccd_rs::Result<T>` (`Result<T, QHYError>`)
  instead of `eyre::Result<T>`, and a public `Result<T>` alias is exported.
  `QHYError` gains `NoImageAvailable`, `NoImageMetadataAvailable`, `InvalidUtf8`,
  and `InvalidCameraId` variants. Code that matched on `eyre::Report` must match
  `QHYError` instead.
- Real-backend `Camera` methods now return `CameraNotOpenError` (not an
  operation-specific error such as `BeginLiveError`) when called on an unopened
  camera, matching the simulation backend.
- Target QHYCCD SDK **26.06.04**. The 26.x distribution changed packaging
  (dot-stripped repo dir `260604`, `.tar.gz` archives, no `install.sh`, and the
  per-OS archives renamed `macMix`→`mac_x64` / `WinMix`→`win64` /
  `Arm64`→`linux_arm64`). `libqhyccd-sys`'s `build.rs` now resolves the macOS
  extract dirs (`sdk_mac_arm_<ver>` / `sdk_mac_x64_<ver>`) and the Windows
  `sdk_win64_<ver>` layout accordingly; the Linux `/usr/local/lib` link path is
  unchanged. Validated on real hardware (QHY178M + 7-slot CFW, ConformU 0 errors).

### Removed

- **BREAKING:** dropped the `eyre` and `educe` dependencies. `Camera`'s `PartialEq`
  (which ignores the backend handle) now uses `derive_more`'s `#[partial_eq(skip)]`.

### Internal

- Switched internal locks from `std::sync::RwLock` to the non-poisoning
  `parking_lot::RwLock` (the workspace standard, already used by the consuming
  camera services). Lock acquisition is now infallible, so the poison-handling
  paths and the `LockPoisoned` error variant are gone. Adds a `parking_lot`
  dependency.
- Upgraded `rand` to 0.10 (the `Rng`/`RngExt` trait split). `rand`, `rayon`,
  `thiserror`, and `tracing` now inherit the workspace dependency pins.
- Moved the demo programs from `src/bin/` to `examples/` and made
  `tracing-subscriber` a dev-dependency, so library consumers no longer pull it.

## [0.1.9] - 2026-01-19

### Fixed

- Fixed simulation exposure cancellation bug: `stop_exposure()` now correctly preserves image data while `abort_exposure_and_readout()` discards it, matching QHYCCD SDK behavior
- Fixed double-binning bug in simulation where ROI dimensions were incorrectly divided by binning factor, causing images to be half the expected size
- Updated `get_current_image_dimensions()` to return ROI dimensions directly as they are already in binned coordinates when set via ASCOM Alpaca

### Changed

- Split simulation exposure cancellation into two distinct methods: `stop_exposure()` (preserves image) and `abort_exposure()` (discards image)
- Updated design documentation to reflect exposure cancellation behavior and ROI/binning coordinate system

## [0.1.8] - 2026-01-18

### Added

- Comprehensive design documentation for the library architecture
- Automatic default simulated camera when using `Sdk::new()` with simulation feature enabled

### Changed

- Improved simulation performance with rayon parallelization and smart waiting for exposure completion
- Refactored lib.rs into modular structure for better code organization
- Simulation feature is now transparent - `Sdk::new()` automatically provides simulated devices when simulation feature is enabled
- Updated rand dependency to 0.9.2
- Marked mock FFI functions as unsafe for better type safety

### Fixed

- Resolved simulation conformity issues with more robust testing
- Fixed cooler parameter handling bugs in simulation mode
- Removed unused imports and addressed clippy warnings

## [0.1.7] - 2025-01-01

### Changed

- **BREAKING**: Removed vendored feature from libqhyccd-sys - this change should
only affect the CI builds, as any real-world use of the library
needs the SDK installed locally
- Updated SDK version references from 24.12.26 to 25.09.29 in README
- CI/CD now uses system-installed SDK via [qhyccd-sdk-install](https://github.com/ivonnyssen/qhyccd-sdk-install) GitHub action
- Simplified build.rs to only link system libraries

### Removed

- Vendored SDK files no longer bundled with the crate
- All `--features libqhyccd-sys/vendored` flags from CI workflows

### Fixed

- Updated installation instructions in README to use correct SDK version

## [0.1.6] - Previous Release

- Previous functionality with vendored SDK support

[Unreleased]: https://github.com/ivonnyssen/rusty-photon/commits/main/crates/qhyccd-rs
