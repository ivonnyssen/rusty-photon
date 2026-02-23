# rusty_photon [![Build Status](https://github.com/ivonnyssen/rusty-photon/workflows/test/badge.svg)](https://github.com/ivonnyssen/rusty-photon/actions) [![Codecov](https://codecov.io/github/ivonnyssen/rusty-photon/coverage.svg?branch=main)](https://codecov.io/gh/ivonnyssen/rusty-photon) [![Dependency status](https://deps.rs/repo/github/ivonnyssen/rusty-photon/status.svg)](https://deps.rs/repo/github/ivonnyssen/rusty-photon) [![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

Cross-platform [ASCOM Alpaca](https://ascom-standards.org/Developer/Alpaca.htm) services and tools for observatory automation. ASCOM Alpaca is an open HTTP/REST standard for controlling astronomy equipment — these services expose real hardware as network-accessible devices that any Alpaca-compatible client (NINA, SGPro, Voyager, etc.) can discover and control.

## Services

| Service | Type | Port | Description |
|---------|------|------|-------------|
| [filemonitor](services/filemonitor) | ASCOM SafetyMonitor | 11111 | Monitors file content for observatory safety status |
| [ppba-driver](services/ppba-driver) | ASCOM Switch + ObservingConditions | 11112 | Driver for Pegasus Astro Pocket Powerbox Advance Gen2 |
| [qhy-focuser](services/qhy-focuser) | ASCOM Focuser | 11113 | Driver for QHY Q-Focuser (EAF) |
| [phd2-guider](services/phd2-guider) | Client library | — | Rust client for PHD2 autoguiding via JSON RPC |
| [sentinel](services/sentinel) | Monitoring service | 11114 | Polls devices, sends notifications, serves web dashboard |

### Filemonitor

ASCOM Alpaca SafetyMonitor that reads a plain text file and evaluates configurable regex/contains rules to determine observatory safety status. Supports case-sensitive and case-insensitive matching with per-rule safe/unsafe outcomes.

**Platforms:** Linux, macOS, Windows

See [docs/services/filemonitor.md](docs/services/filemonitor.md) for design documentation.

### PPBA Driver

ASCOM Alpaca Switch and ObservingConditions driver for the Pegasus Astro Pocket Powerbox Advance Gen2. Exposes 16 switches (6 controllable power/dew/USB outputs, 10 read-only sensors) over serial. Includes dynamic write protection for dew heaters when auto-dew is enabled.

**Platforms:** Linux, macOS, Windows

See [docs/services/ppba-driver.md](docs/services/ppba-driver.md) for design documentation.

### QHY Focuser

ASCOM Alpaca Focuser driver for the QHY Q-Focuser (Electronic Auto Focuser). Communicates via a JSON-based command/response protocol over USB-CDC serial. Supports absolute and relative moves, speed configuration, temperature readout, and motor hold current settings.

**Platforms:** Linux, macOS, Windows

See [docs/services/qhy-focuser.md](docs/services/qhy-focuser.md) for design documentation.

### PHD2 Guider

Rust client library for programmatic control of [PHD2](https://openphdguiding.org/) autoguiding. Provides JSON RPC 2.0 communication, event subscription, guiding control (start, stop, dither, pause), calibration, camera control, profile management, and auto-reconnect logic. Includes a `mock_phd2` binary for testing without hardware.

See [docs/services/phd2-guider.md](docs/services/phd2-guider.md) for design documentation.

### Sentinel

Observatory monitoring and notification service. Polls ASCOM Alpaca SafetyMonitor devices, detects safe/unsafe state transitions, sends push notifications via Pushover, and serves a live web dashboard. Unlike the other services, sentinel is a **client/consumer** of ASCOM devices, not a server.

**Platforms:** Linux, macOS, Windows

See [services/sentinel/README.md](services/sentinel/README.md) for usage and [docs/services/sentinel.md](docs/services/sentinel.md) for design documentation.

## Getting Started

### Prerequisites

- **Rust** (edition 2021, MSRV 1.85.0 for most services, 1.88.0 for ppba-driver/qhy-focuser/sentinel)
- **libcfitsio** — required by phd2-guider's FITS support:
  - Ubuntu/Debian: `sudo apt install libcfitsio-dev`
  - macOS: `brew install cfitsio`
  - Windows: available via vcpkg
  - To skip: build individual services with `cargo build -p <service>`

### Building

```bash
# Build everything
cargo build --all

# Build a single service
cargo build -p filemonitor
```

### Running

```bash
# Run a service (example: filemonitor)
cargo run -p filemonitor -- --help

# Run sentinel with a config file
cargo run -p sentinel -- -c services/sentinel/examples/config.json
```

## Testing

The project uses a layered test strategy. See [docs/testing-rules.md](docs/testing-rules.md) for the full testing guide.

```bash
# Run all unit and BDD tests
cargo test --all

# Run tests for a single service
cargo test -p filemonitor

# Run tests requiring mock hardware
cargo test -p ppba-driver --features mock
cargo test -p qhy-focuser --features mock
```

### ConformU (ASCOM Compliance)

ASCOM Alpaca compliance testing is integrated via [ConformU](https://github.com/ASCOMStandards/ConformU):

```bash
# Install ConformU (first time only)
./scripts/test-conformance.sh --install-conformu

# Run ConformU compliance tests
cargo test --test conformu_integration -- --ignored
```

### Local CI

CI workflows can be run locally using [act](https://github.com/nektos/act). See [CI_LOCAL.md](CI_LOCAL.md) for instructions, or run the pre-push quality gate:

```bash
cargo build --all --quiet --color never
cargo test --all --quiet --color never
cargo fmt
```

## Project Structure

```
rusty-photon/
  Cargo.toml              Workspace root with shared dependencies
  services/
    filemonitor/           ASCOM SafetyMonitor (file-based)
    ppba-driver/           ASCOM Switch + ObservingConditions (serial)
    qhy-focuser/           ASCOM Focuser (serial)
    phd2-guider/           PHD2 client library (TCP/JSON RPC)
    sentinel/              Monitoring service (HTTP consumer)
    sentinel-app/          Leptos web frontend for sentinel dashboard
  docs/
    services/              Per-service design documentation
    decisions/             Architecture Decision Records
    workspace.md           Workspace architecture and shared patterns
    testing-rules.md       Test strategy and conventions
    ascom-alpaca.md        ASCOM Alpaca protocol reference
    pre-push-checklist.md  CI checks mapped to local commands
  scripts/                 CI and ConformU setup scripts
  external/phd2/           PHD2 source (git submodule, reference only)
```

## Documentation

| Document | Description |
|----------|-------------|
| [docs/workspace.md](docs/workspace.md) | Workspace architecture, shared patterns, dependency policy |
| [docs/testing-rules.md](docs/testing-rules.md) | Test pyramid, BDD conventions, unit test guidelines |
| [docs/ascom-alpaca.md](docs/ascom-alpaca.md) | ASCOM Alpaca protocol reference |
| [docs/pre-push-checklist.md](docs/pre-push-checklist.md) | Local equivalents of CI checks |
| [CI_LOCAL.md](CI_LOCAL.md) | Running CI workflows locally with `act` |
| [docs/decisions/](docs/decisions/) | Architecture Decision Records |

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option.
