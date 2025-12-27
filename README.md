# rusty_photon [![Build Status](https://github.com/ivonnyssen/rusty-photon/workflows/test/badge.svg)](https://github.com/ivonnyssen/rusty-photon/actions) [![Codecov](https://codecov.io/github/ivonnyssen/rusty-photon/coverage.svg?branch=main)](https://codecov.io/gh/ivonnyssen/rusty-photon) [![Dependency status](https://deps.rs/repo/github/ivonnyssen/rusty-photon/status.svg)](https://deps.rs/repo/github/ivonnyssen/rusty-photon) [![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

Cross-platform ASCOM Alpaca services for astronomy applications.

## Services

### Filemonitor
ASCOM Alpaca SafetyMonitor that monitors file content for observatory safety status.

**Platforms:** Linux, macOS, Windows  
**Features:** File monitoring, configurable parsing rules, ASCOM compliance

See [docs/services/filemonitor.md](docs/services/filemonitor.md) for detailed documentation.

## Testing

### ConformU Integration
ASCOM Alpaca compliance testing is integrated into the test suite using ConformU:

```bash
# Install ConformU (first time only)
./test-conformance.sh --install-conformu

# Run ConformU compliance tests
cargo test --test conformu_integration -- --ignored
```

