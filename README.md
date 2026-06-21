# rusty_photon [![Build Status](https://github.com/ivonnyssen/rusty-photon/workflows/test/badge.svg)](https://github.com/ivonnyssen/rusty-photon/actions) [![Codecov](https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg)](https://codecov.io/gh/ivonnyssen/rusty-photon) [![Dependency status](https://deps.rs/repo/github/ivonnyssen/rusty-photon/status.svg)](https://deps.rs/repo/github/ivonnyssen/rusty-photon) [![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

Cross-platform [ASCOM Alpaca](https://ascom-standards.org/Developer/Alpaca.htm) services and tools for observatory automation. ASCOM Alpaca is an open HTTP/REST standard for controlling astronomy equipment — these services expose real hardware as network-accessible devices that any Alpaca-compatible client (NINA, SGPro, Voyager, etc.) can discover and control.

**Platforms:** Linux, macOS, Windows (all services). Designed to run efficiently on hardware as small as a Raspberry Pi 5.

## Services

Coverage has two columns: **Cargo** is the canonical, required coverage; **Bazel** is the shadow-mode `bazel coverage` job (`.github/workflows/bazel-coverage.yml`), uploaded under `bazel-<pkg>` Codecov flags. During shadow mode the Bazel badges reflect the most recent `main` commit whose `bazel-coverage` run completed (Codecov carries the last value forward when a run is cancelled), so they may lag the Cargo badges — and may read lower where BDD child-process coverage is still being validated. They are not gating; the goal is Cargo↔Bazel parity before cutover.

| Service | Type | Port | Coverage (Cargo) | Coverage (Bazel) | Description |
|---------|------|------|------------------|------------------|-------------|
| [rp](services/rp) | Equipment gateway | 11115 | [![coverage][cov-rp]][cov-rp-link] | [![coverage][cov-bazel-rp]][cov-bazel-rp-link] | Main application: MCP tools, event bus, safety enforcer |
| [filemonitor](services/filemonitor) | ASCOM SafetyMonitor | 11111 | [![coverage][cov-filemonitor]][cov-filemonitor-link] | [![coverage][cov-bazel-filemonitor]][cov-bazel-filemonitor-link] | Monitors file content for observatory safety status |
| [ppba-driver](services/ppba-driver) | ASCOM Switch + ObservingConditions | 11112 | [![coverage][cov-ppba-driver]][cov-ppba-driver-link] | [![coverage][cov-bazel-ppba-driver]][cov-bazel-ppba-driver-link] | Driver for Pegasus Astro Pocket Powerbox Advance Gen2 |
| [qhy-focuser](services/qhy-focuser) | ASCOM Focuser | 11113 | [![coverage][cov-qhy-focuser]][cov-qhy-focuser-link] | [![coverage][cov-bazel-qhy-focuser]][cov-bazel-qhy-focuser-link] | Driver for QHY Q-Focuser (EAF) |
| [phd2-guider](services/phd2-guider) | Client library | — | [![coverage][cov-phd2-guider]][cov-phd2-guider-link] | [![coverage][cov-bazel-phd2-guider]][cov-bazel-phd2-guider-link] | Rust client for PHD2 autoguiding via JSON RPC |
| [sentinel](services/sentinel) | Monitoring service | 11114 | [![coverage][cov-sentinel]][cov-sentinel-link] | [![coverage][cov-bazel-sentinel]][cov-bazel-sentinel-link] | Polls devices, sends notifications, serves web dashboard |
| [calibrator-flats](services/calibrator-flats) | Orchestrator plugin | 11170 | [![coverage][cov-calibrator-flats]][cov-calibrator-flats-link] | [![coverage][cov-bazel-calibrator-flats]][cov-bazel-calibrator-flats-link] | Flat field calibration with CoverCalibrator device |
| [sky-survey-camera](services/sky-survey-camera) | ASCOM Camera (simulator) | 11116 | [![coverage][cov-sky-survey-camera]][cov-sky-survey-camera-link] | [![coverage][cov-bazel-sky-survey-camera]][cov-bazel-sky-survey-camera-link] | Camera simulator that returns NASA SkyView cutouts for the configured optics |
| [star-adventurer-gti](services/star-adventurer-gti) | ASCOM Telescope | 11117 | [![coverage][cov-star-adventurer-gti]][cov-star-adventurer-gti-link] | [![coverage][cov-bazel-star-adventurer-gti]][cov-bazel-star-adventurer-gti-link] | Driver for Sky-Watcher Star Adventurer GTi (USB and WiFi/UDP) |
| [pa-falcon-rotator](services/pa-falcon-rotator) | ASCOM Rotator + Switch (status) | 11118 | [![coverage][cov-pa-falcon-rotator]][cov-pa-falcon-rotator-link] | [![coverage][cov-bazel-pa-falcon-rotator]][cov-bazel-pa-falcon-rotator-link] | Driver for Pegasus Astro Falcon Rotator (firmware ≥ 1.3) |
| [dsd-fp2](services/dsd-fp2) | ASCOM CoverCalibrator | 11119 | [![coverage][cov-dsd-fp2]][cov-dsd-fp2-link] | [![coverage][cov-bazel-dsd-fp2]][cov-bazel-dsd-fp2-link] | Driver for Deep Sky Dad Flat Panel 2 (motorised flat field panel) |
| [ui-htmx](services/ui-htmx) | Web config UI (BFF) | 11120 | [![coverage][cov-ui-htmx]][cov-ui-htmx-link] | [![coverage][cov-bazel-ui-htmx]][cov-bazel-ui-htmx-link] | Server-rendered configuration UI (axum + Maud + HTMX); edits any driver's config via its `config.get`/`config.apply` actions |
| [plate-solver](services/plate-solver) | rp-managed HTTP service | 11131 | [![coverage][cov-plate-solver]][cov-plate-solver-link] | [![coverage][cov-bazel-plate-solver]][cov-bazel-plate-solver-link] | Wraps the ASTAP CLI for plate solving in a supervised, crash-isolated process |
| [qhy-camera](services/qhy-camera) | ASCOM Camera (+ FilterWheel) | 11121 | [![coverage][cov-qhy-camera]][cov-qhy-camera-link] | [![coverage][cov-bazel-qhy-camera]][cov-bazel-qhy-camera-link] | Driver for QHYCCD cameras + filter wheels (vendored `qhyccd-rs` bindings; links the proprietary SDK unless `QHYCCD_SKIP_NATIVE_LINK=1`) |
| [zwo-camera](services/zwo-camera) | ASCOM Camera | 11122 | [![coverage][cov-zwo-camera]][cov-zwo-camera-link] | [![coverage][cov-bazel-zwo-camera]][cov-bazel-zwo-camera-link] | Driver for ZWO ASI cameras (vendored `zwo-rs` bindings, MIT SDK; links the SDK unless `ZWO_SKIP_NATIVE_LINK=1`); EFW filter-wheel support in progress |

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

### Star Adventurer GTi

ASCOM Alpaca Telescope driver for the Sky-Watcher Star Adventurer GTi, an entry-level GoTo equatorial mount. Speaks the Sky-Watcher motor-controller protocol over USB-CDC serial (115200 baud) and UDP (192.168.4.1:11880 in mount-AP mode). Implements connect/disconnect, RA/Dec reads, sync, async slew, sidereal tracking, software park, abort, and pulse guiding — leaving custom tracking rates and Alt/Az slew for follow-up. The shared codec lives in the `skywatcher-motor-protocol` workspace crate so other Sky-Watcher mounts can reuse it.

See [docs/services/star-adventurer-gti.md](docs/services/star-adventurer-gti.md) for design documentation.

### Pa Falcon Rotator

ASCOM Alpaca Rotator + Switch driver for the Pegasus Astro Falcon Rotator (firmware ≥ 1.3). Exposes the rotator as `IRotatorV4` with sky/mechanical position separation (`Sync` is a driver-side offset; the wire-level `SD` command is never issued) and a second `ISwitchV3` device that surfaces the Falcon's raw input voltage and `FA.limit_detect` flag as two read-only switches. Communicates via 9600-baud USB-CDC serial; every property read maps to a live serial command (no cache, no background poller) so the device is always the authoritative source.

See [docs/services/falcon-rotator.md](docs/services/falcon-rotator.md) for design documentation.

### DSD FP2

ASCOM Alpaca CoverCalibrator driver for the Deep Sky Dad Flat Panel 2 (FP2), a motorised flat-field panel combining a 4096-step EL light source with a servo-driven cover. Built on the workspace's `rusty-photon-shared-transport` crate (PR #269): the FP2's bracketed-ASCII protocol (`[GFRM]`, `[STRG270]`, `[SLBR1234]`, …) is plugged in as an `Fp2Codec`, `Fp2SerialTransportFactory` opens the USB-CDC port (115200 baud, `/dev/ttyACM*`), and a thin `FlatPanelManager` over `SharedTransport<Fp2Codec>` handles refcounting, request arbitration, and the polling task via `Hooks`. Pairs with `calibrator-flats` for automated flat-field calibration without any orchestrator changes.

See [docs/services/dsd-fp2.md](docs/services/dsd-fp2.md) for design documentation.

### UI-HTMX

Browser-facing, server-rendered configuration UI — a standalone backend-for-frontend (BFF) that holds no UI logic inside `rp`. Renders HTML with axum + Maud and adds interactivity with HTMX. It is a **client** of the drivers, reading and writing each one's configuration through the cross-driver `config.get` / `config.apply` / `config.schema` ASCOM actions, so a single UI can configure any driver without driver-specific knowledge.

See [docs/services/ui-htmx.md](docs/services/ui-htmx.md) for design documentation.

### Plate Solver

An **rp-managed** HTTP service that wraps an operator-supplied [ASTAP](https://www.hnsky.org/astap.htm) CLI install and exposes a narrow solve API to `rp`. Plate solving is a hang-prone, crash-prone external binary, so it runs in its own supervised process where its failure modes cannot threaten `rp`'s liveness. Stateless across requests: every solve spawns a fresh `astap_cli` subprocess under a wall-clock timeout.

See [docs/services/plate-solver.md](docs/services/plate-solver.md) for design documentation.

### QHY Camera

ASCOM Alpaca **Camera (+ FilterWheel)** driver for real QHYCCD hardware, built natively on the vendored first-party `qhyccd-rs` bindings crate (ADR-009). It enumerates every connected QHY camera and CFW and exposes each as an ASCOM device on one port. Implemented (v0). By default the build links the proprietary QHYCCD SDK; for an SDK-free build set `QHYCCD_SKIP_NATIVE_LINK=1` together with `--features simulation` (which `cfg`s out the native FFI) — the path CI and the sanitizer jobs use. See [docs/services/qhy-camera.md](docs/services/qhy-camera.md) for design documentation.

### ZWO Camera

ASCOM Alpaca **Camera** driver for ZWO ASI hardware, built natively on the vendored first-party `zwo-rs` bindings crate (ADR-008 / ADR-010). The MIT ZWO SDK itself is not vendored — it is provisioned at build time and linked unless `ZWO_SKIP_NATIVE_LINK=1` is set (the simulation-only path CI uses). Exposes the full `Device + Camera` surface — exposure state machine, ROI/binning, gain/offset, cooling, readout modes, and ST4 pulse guiding — and passes ConformU. EFW filter-wheel support is the next phase. See [docs/services/zwo-camera.md](docs/services/zwo-camera.md) for design documentation.

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

# Run a service's ConformU suite. The tests are feature-gated behind `conformu`
# (a no-op without it). The canonical per-service command lives in each
# Cargo.toml's [package.metadata.conformu], e.g. for filemonitor:
cargo test -p filemonitor --features conformu --test conformu_integration -- --nocapture
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
    bdd-infra/                       Shared BDD test infrastructure (ServiceHandle + rp-harness)
    rp-auth/                         HTTP Basic Auth utilities (Argon2id + axum, ADR-003)
    rp-catalog/                      Embedded Messier/NGC/IC catalog with name resolution
    rp-ephemeris/                    Astronomical math (Ephemeris + ERFA wrapper + Site)
    rp-fits/                         FITS reader/writer wrapper (ADR-001)
    rp-plate-solver/                 HTTP client for the plate-solver service
    rp-tls/                          TLS/ACME helpers for inter-service comms (ADR-002)
    rusty-photon-config/             Config-path + first-run UniqueID + config.get/apply/schema protocol
    rusty-photon-driver/             Shared ASCOM-driver runtime: DriverError + config-action dispatch (ADR-007)
    rusty-photon-i18n/               Workspace Fluent i18n loader + locale resolver
    rusty-photon-i18n-derive/        Proc-macro deriving LocalizedParser for clap structs
    rusty-photon-service-lifecycle/  Unified lifecycle: runtime + signals + optional Windows SCM
    rusty-photon-shared-transport/   Refcounted multi-client transport scaffolding (serial + UDP)
    skywatcher-motor-protocol/       Sky-Watcher motor-controller wire protocol codec (USB + UDP)
    qhyccd-rs/                       Vendored QHYCCD SDK bindings + nested libqhyccd-sys FFI (ADR-009)
    zwo-rs/                          Vendored ZWO ASI/EFW SDK bindings + nested libzwo-sys FFI (ADR-008/010)
  services/
    rp/                    Main application: equipment gateway, event bus, safety enforcer
    filemonitor/           ASCOM SafetyMonitor (file-based)
    ppba-driver/           ASCOM Switch + ObservingConditions (serial)
    qhy-focuser/           ASCOM Focuser (serial)
    dsd-fp2/               ASCOM CoverCalibrator — Deep Sky Dad FP2 (serial)
    pa-falcon-rotator/     ASCOM Rotator + Switch — Pegasus Falcon (serial)
    star-adventurer-gti/   ASCOM Telescope — Sky-Watcher GTi (USB + WiFi/UDP)
    sky-survey-camera/     ASCOM Camera simulator backed by NASA SkyView
    qhy-camera/            ASCOM Camera + FilterWheel — QHYCCD hardware (implemented v0; vendored SDK)
    zwo-camera/            ASCOM Camera — ZWO ASI hardware (implemented; vendored MIT SDK)
    phd2-guider/           PHD2 client library (TCP/JSON RPC)
    sentinel/              Monitoring service (HTTP consumer)
    calibrator-flats/      Flat-field calibration orchestrator plugin (CoverCalibrator)
    plate-solver/          rp-managed HTTP service wrapping the ASTAP CLI
    ui-htmx/               Server-rendered web configuration UI (BFF)
  docs/
    services/              Per-service design documentation
    crates/                Per-crate design documentation
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
| [docs/skills/raspberry-pi-runner.md](docs/skills/raspberry-pi-runner.md) | Skill: setting up the Pi 5 ARM64 nightly self-hosted runner |
| [docs/references/ascom-alpaca.md](docs/references/ascom-alpaca.md) | ASCOM Alpaca protocol reference |
| [docs/decisions/](docs/decisions/) | Architecture Decision Records |

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option.

<!-- per-service coverage badges (Cargo, flag=<pkg>) -->
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
[cov-star-adventurer-gti]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=star-adventurer-gti
[cov-star-adventurer-gti-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=star-adventurer-gti
[cov-dsd-fp2]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=dsd-fp2
[cov-dsd-fp2-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=dsd-fp2
[cov-pa-falcon-rotator]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=pa-falcon-rotator
[cov-pa-falcon-rotator-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=pa-falcon-rotator
[cov-ui-htmx]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=ui-htmx
[cov-ui-htmx-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=ui-htmx
[cov-plate-solver]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=plate-solver
[cov-plate-solver-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=plate-solver
[cov-qhy-camera]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=qhy-camera
[cov-qhy-camera-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=qhy-camera
[cov-zwo-camera]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=zwo-camera
[cov-zwo-camera-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=zwo-camera

<!-- per-service coverage badges (Bazel shadow build, flag=bazel-<pkg>; read "unknown" until .github/workflows/bazel-coverage.yml has run on main) -->
[cov-bazel-rp]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=bazel-rp
[cov-bazel-rp-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=bazel-rp
[cov-bazel-filemonitor]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=bazel-filemonitor
[cov-bazel-filemonitor-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=bazel-filemonitor
[cov-bazel-ppba-driver]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=bazel-ppba-driver
[cov-bazel-ppba-driver-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=bazel-ppba-driver
[cov-bazel-qhy-focuser]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=bazel-qhy-focuser
[cov-bazel-qhy-focuser-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=bazel-qhy-focuser
[cov-bazel-phd2-guider]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=bazel-phd2-guider
[cov-bazel-phd2-guider-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=bazel-phd2-guider
[cov-bazel-sentinel]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=bazel-sentinel
[cov-bazel-sentinel-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=bazel-sentinel
[cov-bazel-calibrator-flats]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=bazel-calibrator-flats
[cov-bazel-calibrator-flats-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=bazel-calibrator-flats
[cov-bazel-sky-survey-camera]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=bazel-sky-survey-camera
[cov-bazel-sky-survey-camera-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=bazel-sky-survey-camera
[cov-bazel-star-adventurer-gti]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=bazel-star-adventurer-gti
[cov-bazel-star-adventurer-gti-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=bazel-star-adventurer-gti
[cov-bazel-dsd-fp2]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=bazel-dsd-fp2
[cov-bazel-dsd-fp2-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=bazel-dsd-fp2
[cov-bazel-pa-falcon-rotator]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=bazel-pa-falcon-rotator
[cov-bazel-pa-falcon-rotator-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=bazel-pa-falcon-rotator
[cov-bazel-ui-htmx]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=bazel-ui-htmx
[cov-bazel-ui-htmx-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=bazel-ui-htmx
[cov-bazel-plate-solver]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=bazel-plate-solver
[cov-bazel-plate-solver-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=bazel-plate-solver
[cov-bazel-qhy-camera]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=bazel-qhy-camera
[cov-bazel-qhy-camera-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=bazel-qhy-camera
[cov-bazel-zwo-camera]: https://codecov.io/gh/ivonnyssen/rusty-photon/branch/main/graph/badge.svg?flag=bazel-zwo-camera
[cov-bazel-zwo-camera-link]: https://codecov.io/gh/ivonnyssen/rusty-photon?flags[0]=bazel-zwo-camera
