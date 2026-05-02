# rusty_photon [![Build Status](https://github.com/ivonnyssen/rusty-photon/workflows/test/badge.svg)](https://github.com/ivonnyssen/rusty-photon/actions) [![Codecov](https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg)](https://codecov.io/gh/ivonnyssen/rusty-photon) [![Dependency status](https://deps.rs/repo/github/ivonnyssen/rusty-photon/status.svg)](https://deps.rs/repo/github/ivonnyssen/rusty-photon) [![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

Cross-platform [ASCOM Alpaca](https://ascom-standards.org/Developer/Alpaca.htm) services and tools for observatory automation. ASCOM Alpaca is an open HTTP/REST standard for controlling astronomy equipment — these services expose real hardware as network-accessible devices that any Alpaca-compatible client (NINA, SGPro, Voyager, etc.) can discover and control.

**Platforms:** Linux, macOS, Windows (all services). Designed to run efficiently on hardware as small as a Raspberry Pi 5.

## Services

| Service | Type | Port | Coverage | Description |
|---------|------|------|----------|-------------|
| [rp](services/rp) | Equipment gateway | 11115 | [![coverage][cov-rp]][cov-rp-link] | Main application: MCP tools, event bus, safety enforcer |
| [filemonitor](services/filemonitor) | ASCOM SafetyMonitor | 11111 | [![coverage][cov-filemonitor]][cov-filemonitor-link] | Monitors file content for observatory safety status |
| [ppba-driver](services/ppba-driver) | ASCOM Switch + ObservingConditions | 11112 | [![coverage][cov-ppba-driver]][cov-ppba-driver-link] | Driver for Pegasus Astro Pocket Powerbox Advance Gen2 |
| [qhy-focuser](services/qhy-focuser) | ASCOM Focuser | 11113 | [![coverage][cov-qhy-focuser]][cov-qhy-focuser-link] | Driver for QHY Q-Focuser (EAF) |
| [phd2-guider](services/phd2-guider) | Client library | — | [![coverage][cov-phd2-guider]][cov-phd2-guider-link] | Rust client for PHD2 autoguiding via JSON RPC |
| [sentinel](services/sentinel) | Monitoring service | 11114 | [![coverage][cov-sentinel]][cov-sentinel-link] | Polls devices, sends notifications, serves web dashboard |
| [calibrator-flats](services/calibrator-flats) | Orchestrator plugin | 11170 | [![coverage][cov-calibrator-flats]][cov-calibrator-flats-link] | Flat field calibration with CoverCalibrator device |
| [sky-survey-camera](services/sky-survey-camera) | ASCOM Camera (simulator) | 11116 | [![coverage][cov-sky-survey-camera]][cov-sky-survey-camera-link] | Camera simulator that returns NASA SkyView cutouts for the configured optics |

### RP (Main Application)

Equipment gateway, event bus, and safety enforcer. Exposes all hardware as MCP tools, emits events for plugins to consume, and enforces safety constraints. Orchestration is handled by a separate orchestrator plugin that drives the session by calling tools on `rp`.

See [docs/services/rp.md](docs/services/rp.md) for design documentation.

### Filemonitor

ASCOM Alpaca SafetyMonitor that reads a plain text file and evaluates configurable regex/contains rules to determine observatory safety status. Supports case-sensitive and case-insensitive matching with per-rule safe/unsafe outcomes.

See [docs/services/filemonitor.md](docs/services/filemonitor.md) for design documentation.

### PPBA Driver

ASCOM Alpaca Switch and ObservingConditions driver for the Pegasus Astro Pocket Powerbox Advance Gen2. Exposes 16 switches (6 controllable power/dew/USB outputs, 10 read-only sensors) over serial. Includes dynamic write protection for dew heaters when auto-dew is enabled.

See [docs/services/ppba-driver.md](docs/services/ppba-driver.md) for design documentation.

### QHY Focuser

ASCOM Alpaca Focuser driver for the QHY Q-Focuser (Electronic Auto Focuser). Communicates via a JSON-based command/response protocol over USB-CDC serial. Supports absolute and relative moves, speed configuration, temperature readout, and motor hold current settings.

See [docs/services/qhy-focuser.md](docs/services/qhy-focuser.md) for design documentation.

### PHD2 Guider

Rust client library for programmatic control of [PHD2](https://openphdguiding.org/) autoguiding. Provides JSON RPC 2.0 communication, event subscription, guiding control (start, stop, dither, pause), calibration, camera control, profile management, and auto-reconnect logic. Includes a `mock_phd2` binary for testing without hardware.

See [docs/services/phd2-guider.md](docs/services/phd2-guider.md) for design documentation.

### Sentinel

Observatory monitoring and notification service. Polls ASCOM Alpaca SafetyMonitor devices, detects safe/unsafe state transitions, sends push notifications via Pushover, and serves a live web dashboard. Unlike the other services, sentinel is a **client/consumer** of ASCOM devices, not a server.

See [services/sentinel/README.md](services/sentinel/README.md) for usage and [docs/services/sentinel.md](docs/services/sentinel.md) for design documentation.

### Calibrator Flats

Orchestrator plugin for flat field calibration using a CoverCalibrator device (flat panel / light box). Connects to `rp` as an MCP client, iteratively determines the correct exposure time per filter to achieve 50% of the camera's well depth, then captures the requested number of flat frames. Manages the full CoverCalibrator lifecycle (close cover, turn on light, capture, turn off, open cover).

See [docs/services/calibrator-flats.md](docs/services/calibrator-flats.md) for design documentation.

### Sky Survey Camera

ASCOM Alpaca Camera **simulator** that synthesises exposures from NASA SkyView cutouts. Given a configured optical system (focal length, sensor pixel count, pixel size) and a sky position (RA/Dec, settable at runtime via a custom HTTP endpoint), it returns an `ImageArray` matching the field of view the equivalent real telescope would see. Useful for driving ASCOM clients and the rest of the rusty-photon stack end-to-end without hardware.

See [docs/services/sky-survey-camera.md](docs/services/sky-survey-camera.md) for design documentation.

## Getting Started

### Prerequisites

- **Rust** (edition 2021, MSRV 1.94.1 — inherited by all workspace members)
- **[cargo-nextest](https://nexte.st/)** (`cargo install cargo-nextest --locked`) — required by the pre-push profile
- **[cargo-rail](https://github.com/loadingalias/cargo-rail)** (optional, recommended) — runs the affected-package pre-push profile

### Building

Cargo is the canonical build. A Bazel build runs in shadow mode alongside it (see [docs/plans/bazel-migration.md](docs/plans/bazel-migration.md)) and is not yet a required pre-push step.

```bash
# Build everything
cargo build --all

# Build a single service
cargo build -p filemonitor

# (Optional) exercise the Bazel shadow build
bazel test //...
```

### Running

```bash
# Run a service (example: filemonitor)
cargo run -p filemonitor -- --help

# Run sentinel with a config file
cargo run -p sentinel -- -c services/sentinel/examples/config.json
```

## Testing

The project uses a layered test strategy. See [docs/skills/testing.md](docs/skills/testing.md) for the full testing guide.

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

CI workflows can be run locally using [act](https://github.com/nektos/act). See [docs/skills/pre-push.md](docs/skills/pre-push.md) for the full guide, or run the minimum pre-push checks. Use **either** the preferred path **or** the fallback — not both.

**Preferred** — cargo-rail's `commit` profile (affected packages only, with `--locked --all-features --all-targets` baked in; defined in `.config/rail.toml`):

```bash
cargo rail run --profile commit -q
cargo fmt
```

**Fallback** — if cargo-rail is not installed:

```bash
cargo build --all --all-targets --all-features --locked --quiet --color never
cargo nextest run --locked --all-features --all-targets --color never
cargo fmt
```

## Project Structure

```
rusty-photon/
  Cargo.toml              Workspace root with shared dependencies
  MODULE.bazel            Bazel module (shadow build, see docs/plans/bazel-migration.md)
  CLAUDE.md / AGENTS.md   Operating rules for AI agents and human contributors
  crates/
    bdd-infra/             Shared BDD test infrastructure (ServiceHandle)
    rp-auth/               Shared HTTP Basic Auth (Argon2id + axum middleware, see ADR-003)
    rp-fits/               FITS file reader/writer wrapper (see ADR-001)
    rp-tls/                Shared TLS/ACME helpers for inter-service comms (ADR-002)
  services/
    rp/                    Main application: equipment gateway, event bus
    filemonitor/           ASCOM SafetyMonitor (file-based)
    ppba-driver/           ASCOM Switch + ObservingConditions (serial)
    qhy-focuser/           ASCOM Focuser (serial)
    phd2-guider/           PHD2 client library (TCP/JSON RPC)
    sentinel/              Monitoring service (HTTP consumer)
    sentinel-app/          Leptos web frontend for sentinel dashboard
    calibrator-flats/      Flat field calibration orchestrator (CoverCalibrator)
    sky-survey-camera/     ASCOM Camera simulator backed by NASA SkyView
  docs/
    services/              Per-service design documentation
    skills/                How-to playbooks for agents and operators
    references/            Protocol and standards reference
    decisions/             Architecture Decision Records (ADRs)
    plans/                 Active migration and roadmap plans
    workspace.md           Workspace architecture and shared patterns
  scripts/                 CI and ConformU setup scripts
  external/
    phd2/                  PHD2 source (git submodule, reference only)
    homebrew-rusty-photon/ Homebrew tap (git submodule)
```

## Documentation

| Document | Description |
|----------|-------------|
| [docs/workspace.md](docs/workspace.md) | Workspace architecture, shared patterns, dependency policy |
| [docs/skills/development-workflow.md](docs/skills/development-workflow.md) | Skill: design-first, test-first development workflow |
| [docs/skills/testing.md](docs/skills/testing.md) | Skill: writing and organizing tests |
| [docs/skills/pre-push.md](docs/skills/pre-push.md) | Skill: running CI quality gates before pushing |
| [docs/references/ascom-alpaca.md](docs/references/ascom-alpaca.md) | ASCOM Alpaca protocol reference |
| [docs/decisions/](docs/decisions/) | Architecture Decision Records |

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option.

<!-- per-service coverage badges -->
[cov-rp]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=rp
[cov-rp-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=rp
[cov-filemonitor]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=filemonitor
[cov-filemonitor-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=filemonitor
[cov-ppba-driver]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=ppba-driver
[cov-ppba-driver-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=ppba-driver
[cov-qhy-focuser]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=qhy-focuser
[cov-qhy-focuser-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=qhy-focuser
[cov-phd2-guider]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=phd2-guider
[cov-phd2-guider-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=phd2-guider
[cov-sentinel]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=sentinel
[cov-sentinel-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=sentinel
[cov-calibrator-flats]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=calibrator-flats
[cov-calibrator-flats-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=calibrator-flats
[cov-sky-survey-camera]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=sky-survey-camera
[cov-sky-survey-camera-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=sky-survey-camera
