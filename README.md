# rusty_photon [![Build Status](https://github.com/ivonnyssen/rusty-photon/workflows/test/badge.svg)](https://github.com/ivonnyssen/rusty-photon/actions) [![Codecov](https://codecov.io/github/ivonnyssen/rusty-photon/coverage.svg?branch=main)](https://codecov.io/gh/ivonnyssen/rusty-photon) [![Dependency status](https://deps.rs/repo/github/ivonnyssen/rusty-photon/status.svg)](https://deps.rs/repo/github/ivonnyssen/rusty-photon) [![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

Cross-platform ASCOM Alpaca services for astronomy applications.

## Services

## Project Structure

This is a monorepo containing multiple ASCOM Alpaca services:

- **services/**: Individual ASCOM Alpaca device implementations and tools
  - **filemonitor/**: SafetyMonitor that monitors file content
  - **sentinel/**: Observatory monitoring, notification, and dashboard service
- **scripts/**: CI and testing scripts
- **docs/services/**: Per-service design documentation

Each service is a separate Rust crate managed by the workspace.

### Filemonitor
ASCOM Alpaca SafetyMonitor that monitors file content for observatory safety status.

**Platforms:** Linux, macOS, Windows  
**Features:** File monitoring, configurable parsing rules, ASCOM compliance

See [docs/services/filemonitor.md](docs/services/filemonitor.md) for detailed documentation.

### Sentinel
Observatory monitoring and notification service. Polls ASCOM Alpaca SafetyMonitor devices, detects safe/unsafe state transitions, sends push notifications via Pushover, and serves a live web dashboard.

**Platforms:** Linux, macOS, Windows
**Features:** ASCOM Alpaca polling, Pushover notifications, web dashboard, configurable transition rules

See [services/sentinel/README.md](services/sentinel/README.md) for usage and [docs/services/sentinel.md](docs/services/sentinel.md) for design documentation.

## Testing

### ConformU Integration
ASCOM Alpaca compliance testing is integrated into the test suite using ConformU:

```bash
# Install ConformU (first time only)
./scripts/test-conformance.sh --install-conformu

# Run ConformU compliance tests
cargo test --test conformu_integration -- --ignored
```

