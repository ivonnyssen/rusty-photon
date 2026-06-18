# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Initial repository scaffold for `zwo-rs` (safe wrapper) and `libzwo-sys` (raw
  FFI), sibling to `qhyccd-rs`.
- `libzwo-sys`: `bindgen`-generated bindings (build-time) from the vendored MIT
  ZWO SDK headers (`ASICamera2.h`, `EFW_filter.h`, `EAF_focuser.h`), parsed as
  C++; per-OS link directives for `libASICamera2` + `libEFWFilter` + `libusb-1.0`.
- `zwo-rs`: skeleton `Sdk` entry point + `simulation` feature scaffold.
- CI (`check`, `test`), Claude Code workflows, pre-commit hook (clippy + fmt),
  dual MIT/Apache-2.0 licensing.

[Unreleased]: https://github.com/ivonnyssen/zwo-rs/commits/main
