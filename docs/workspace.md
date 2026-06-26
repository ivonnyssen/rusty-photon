# Workspace Design

Top-level reference for the rusty-photon workspace. This document indexes
project-wide documentation and captures workspace-level concerns that don't
belong in any single service design doc.

## Services

| Service | ASCOM Type | Port | Design Doc |
|---------|-----------|------|------------|
| [filemonitor](services/filemonitor.md) | SafetyMonitor | 11111 | `docs/services/filemonitor.md` |
| [ppba-driver](services/ppba-driver.md) | Switch + ObservingConditions | 11112 | `docs/services/ppba-driver.md` |
| [qhy-focuser](services/qhy-focuser.md) | Focuser | 11113 | `docs/services/qhy-focuser.md` |
| [phd2-guider](services/phd2-guider.md) | — (client library) | — | `docs/services/phd2-guider.md` |
| [sentinel](services/sentinel.md) | — (monitoring service) | 11114 | `docs/services/sentinel.md` |
| [rp](services/rp.md) | — (orchestrator) | 11115 | `docs/services/rp.md` |
| [plate-solver](services/plate-solver.md) | — (rp-managed service wrapping ASTAP) | 11131 | `docs/services/plate-solver.md` |
| [calibrator-flats](services/calibrator-flats.md) | — (orchestrator plugin) | 11170 | `docs/services/calibrator-flats.md` |
| [sky-survey-camera](services/sky-survey-camera.md) | Camera (simulator) | 11116 | `docs/services/sky-survey-camera.md` |
| [qhy-camera](services/qhy-camera.md) | Camera (+ FilterWheel) — QHYCCD hardware | 11121 | `docs/services/qhy-camera.md` (implemented v0; native QHYCCD SDK dep — links `static=qhyccd` + `libusb-1.0`; **built + tested on GitHub-hosted Linux/macOS/Windows** via the `qhyccd-sdk-install@v3` action, plus the Pi nightly for linux-arm64. Vendored first-party (ADR-009); sanitized under `safety.yml` via the SDK-free `simulation` path (`QHYCCD_SKIP_NATIVE_LINK=1`) — only `bdd-infra` is excluded there) |
| [zwo-camera](services/zwo-camera.md) | Camera (+ FilterWheel) — ZWO ASI/EFW hardware | 11122 | Phase E (full Camera) landed: full `Device + Camera` over `zwo-rs` (exposure state machine, ROI/bin, gain/offset, cooling, readout, ST4 pulse-guiding), serial identity, config actions; 45 unit + 57 BDD green, ConformU passes. Bazel first-class (`lib`/`binary`/`unit_test`; `bdd`/`conformu` run under Bazel). EFW FilterWheel is Phase F. ConformU is wired into `conformu.yml` (per-service matrix + `install-zwo-sdk`), and the nightly `native.yml` builds the real linked path on Linux/macOS/Windows; the `rp` `CameraConfig` consumer is the only Phase-G tail item left. Native ZWO SDK dep, gated out of the default build. See `docs/services/zwo-camera.md` + ADR-008. |
| [star-adventurer-gti](services/star-adventurer-gti.md) | Telescope | 11117 | `docs/services/star-adventurer-gti.md` (implemented — `ITelescopeV3` subset: async slew, sync, sidereal tracking, software park, pulse guiding; all BDD scenarios green) |
| [pa-falcon-rotator](services/falcon-rotator.md) | Rotator + Switch (status) | 11118 | `docs/services/falcon-rotator.md` |
| [dsd-fp2](services/dsd-fp2.md) | CoverCalibrator | 11119 | `docs/services/dsd-fp2.md` (first adopter of `rusty-photon-shared-transport`) |
| [ui-htmx](services/ui-htmx.md) | — (web config UI / BFF, not an ASCOM device) | 11120 | `docs/services/ui-htmx.md` |

## Documentation Index

| Document | Purpose |
|----------|---------|
| **Rules** | |
| [docs/AGENTS.md](AGENTS.md) | Rules for all AI agents and human operators (`CLAUDE.md` is a symlink to this file) |
| **Skills** (how-to playbooks — read before performing the respective task) | |
| [docs/skills/development-workflow.md](skills/development-workflow.md) | Skill: design-first, test-first development workflow |
| [docs/skills/testing.md](skills/testing.md) | Skill: writing and organizing tests (test pyramid, BDD, unit tests) |
| [docs/skills/pre-push.md](skills/pre-push.md) | Skill: running CI quality gates before pushing |
| [docs/skills/service-lifecycle.md](skills/service-lifecycle.md) | Skill: scaffolding a long-running service binary (`main.rs`, runtime + shutdown handling) |
| [docs/skills/archiving-plans.md](skills/archiving-plans.md) | Skill: archiving a completed plan into `docs/plans/archive/` |
| [docs/skills/bazel-remote-cache.md](skills/bazel-remote-cache.md) | Skill: using the self-hosted Bazel remote cache |
| [docs/skills/raspberry-pi-runner.md](skills/raspberry-pi-runner.md) | Skill: the Pi 5 self-hosted ARM64 nightly runner |
| **Crate design docs** (substantial workspace libraries — see [docs/crates/](crates/)) | |
| [docs/crates/rp-ephemeris.md](crates/rp-ephemeris.md) | `rp-ephemeris` — `Ephemeris` trait, ERFA wrapping, panic-safety + NaN-degradation, derived helpers, time-scale treatment |
| [docs/crates/rp-targets.md](crates/rp-targets.md) | `rp-targets` — `redb`-backed imaging-plan store: targets, acquisition goals, per-target grading-threshold + scheduling-constraint overrides; `TargetStore` trait. Design stage; crate not yet built. |
| [docs/crates/rusty-photon-service-lifecycle.md](crates/rusty-photon-service-lifecycle.md) | `rusty-photon-service-lifecycle` — unified tokio runtime + signal handlers + optional Windows SCM, exposing a single `Shutdown` handle across the workspace |
| **References** | |
| [docs/references/ascom-alpaca.md](references/ascom-alpaca.md) | ASCOM Alpaca protocol reference |
| [docs/references/skywatcher-motor-controller-command-set.md](references/skywatcher-motor-controller-command-set.md) | Sky-Watcher motor-controller wire protocol (USB + UDP/11880) — used by `star-adventurer-gti` |
| [docs/references/omnisim.md](references/omnisim.md) | OmniSim (ASCOM Alpaca Simulators) reference — used by BDD/integration tests |
| [docs/references/qhyccd-sdk-manual.md](references/qhyccd-sdk-manual.md) | QHYCCD SDK manual (unofficial English translation, V2.1) — used by `qhy-camera` |
| [docs/services/config-actions.md](services/config-actions.md) | Cross-driver configuration protocol: the `config.get` / `config.apply` / `config.schema` ASCOM actions shared by every driver and consumed by `ui-htmx` |
| **Decisions** (Architecture Decision Records — see [docs/decisions/](decisions/)) | |
| [ADR-001](decisions/001-fits-file-support.md) | FITS file support |
| [ADR-002](decisions/002-tls-for-inter-service-communication.md) | TLS for inter-service communication |
| [ADR-003](decisions/003-authentication-for-device-access.md) | Authentication for device access |
| [ADR-004](decisions/004-testing-strategy-for-http-client-error-paths.md) | Testing strategy for HTTP-client error paths |
| [ADR-005](decisions/005-plate-solver.md) | Plate solver: ASTAP via subprocess + verification spike |
| [ADR-006](decisions/006-typed-physical-quantities-for-mount-pointing.md) | Typed physical quantities (newtypes) for mount pointing |
| [ADR-007](decisions/007-rusty-photon-driver-shared-crate.md) | Extract `rusty-photon-driver` — the shared ASCOM-driver adapter |
| [ADR-008](decisions/008-zwo-camera-native-sdk-ffi.md) | `zwo-camera` native ZWO SDK: author-maintained `zwo-rs`/`libzwo-sys` FFI + MIT-SDK public caching |
| [ADR-009](decisions/009-vendor-qhyccd-rs.md) | Vendor `qhyccd-rs` + `libqhyccd-sys` into the workspace (dual-homed) |
| [ADR-010](decisions/010-vendor-zwo-rs.md) | Vendor `zwo-rs` + `libzwo-sys` into the workspace (dual-homed) |
| **Plans** (in-flight initiatives — see [docs/plans/](plans/)) | |
| [predictive-deadlines-and-watchdog.md](plans/predictive-deadlines-and-watchdog.md) | Predictive operation deadlines in `rp` + Sentinel operation watchdog (5 phases). **Complete** — all phases shipped: Phase 1 event surface (#346), Phase 2 §2.1 slew (#348) + §2.2/§2.3 park+focuser (#352) + §2.4/§2.5 exposure+centering, Phase 3 SSE endpoint, Phase 4 Sentinel watchdog, Phase 5 corrective-action ladder (#373). Authoritative contracts now live in [rp.md §Real-Time Stream](services/rp.md#real-time-stream) and [sentinel.md §Operation Watchdog](services/sentinel.md#operation-watchdog). Archived sub-plans: [phase-1 event surface](plans/archive/predictive-deadlines-phase1-event-surface.md), [phase-2 §2.1 slew](plans/archive/predictive-deadlines-phase2-slew.md), [phase-2 §2.2/§2.3 park+focuser](plans/archive/predictive-deadlines-phase2-park-focuser.md); [phase-3 SSE endpoint](plans/predictive-deadlines-phase3-sse.md) archives on merge. |
| [filemonitor-packaging.md](plans/filemonitor-packaging.md) | Filemonitor OS packaging |
| [i18n.md](plans/i18n.md) | Workspace internationalization: scope, tech-stack, and translation-sourcing options |
| [ui-testing.md](plans/ui-testing.md) | ui-htmx UI-behavior testing strategy: BDD-embedded `scraper` DOM assertions + cross-OS `insta` snapshots + an advisory `thirtyfour` browser layer, with an anticipatory spike plan; Bazel-primary aware; Gherkin stays the source of truth |
| [zwo-driver.md](plans/zwo-driver.md) | ZWO ASI camera + EFW filter-wheel Alpaca driver (`zwo-camera`, port 11122) + author-maintained `zwo-rs`/`libzwo-sys` FFI; the ZWO analogue of `qhy-camera` (MIT SDK → public cache, but no pre-existing Rust FFI). See [`docs/services/zwo-camera.md`](services/zwo-camera.md) + [ADR-008](decisions/008-zwo-camera-native-sdk-ffi.md) |

Completed plans move to [`docs/plans/archive/`](plans/archive/) and are no longer
listed here.

## Shared Crates

| Crate | Location | Purpose |
|-------|----------|---------|
| [bdd-infra](../crates/bdd-infra/) | `crates/bdd-infra` | Shared BDD test infrastructure: `ServiceHandle` for spawning, managing, and stopping service binaries. The binary is located from the caller's package name (`env!("CARGO_PKG_NAME")`) via the conventional `{PACKAGE_UPPER_SNAKE}_BINARY` env override, else the Cargo / llvm-cov target dir (`$CARGO_TARGET_DIR` / `$CARGO_LLVM_COV_TARGET_DIR`, target-triple-aware), else by walking up for `target/debug/<pkg>`. See [testing.md](skills/testing.md) Section 5.1. |
| [rp-tls](../crates/rp-tls/) | `crates/rp-tls` | Opt-in TLS for inter-service communication: certificate generation, dual-stack TCP binding, TLS/plain serving, and client CA trust. See [ADR-002](decisions/002-tls-for-inter-service-communication.md). |
| [rp-auth](../crates/rp-auth/) | `crates/rp-auth` | Opt-in HTTP Basic Auth: Argon2id credential hashing/verification, axum tower middleware, and config types. See [ADR-003](decisions/003-authentication-for-device-access.md). |
| [rp-ephemeris](../crates/rp-ephemeris/) | `crates/rp-ephemeris` | Astronomical math: `Ephemeris` trait + `ErfarsEphemeris` impl wrapping the `erfars` ERFA bindings (BSD-licensed clean-room derivative of IAU SOFA). Pure functions for sidereal time, alt/az, transit, rise/set, twilight, sun + moon position. See [`docs/crates/rp-ephemeris.md`](crates/rp-ephemeris.md) for the crate design (panic safety, NaN-degradation, time scales); [`rp-planning-tools.md`](plans/archive/rp-planning-tools.md) for the original implementation plan. |
| [rp-catalog](../crates/rp-catalog/) | `crates/rp-catalog` | Embedded Messier + NGC + IC catalog (~13k objects, openNGC source, CC-BY-SA-4.0 attribution). `Catalog::resolve(name)` does case- and whitespace-insensitive lookup with alias support. See [`rp-planning-tools.md`](plans/archive/rp-planning-tools.md). |
| [skywatcher-motor-protocol](../crates/skywatcher-motor-protocol/) | `crates/skywatcher-motor-protocol` | Pure codec for the Sky-Watcher motor-controller wire protocol (USB + UDP/11880). Transport-agnostic; isolates the 24-bit low-byte-first hex encoding and the `+0x800000` position bias. Used by `star-adventurer-gti`. See [`docs/references/skywatcher-motor-controller-command-set.md`](references/skywatcher-motor-controller-command-set.md). |
| [rusty-photon-i18n](../crates/rusty-photon-i18n/) | `crates/rusty-photon-i18n` | Fluent loader + locale resolver shared across services. Reads `RP_LOCALE` / `LC_ALL` / `LC_MESSAGES` / `LANG` / OS, negotiates against the locales each consumer embeds, falls back to `en`. Owns `LocalizedParser` trait, `init` lifecycle, and an `ACTIVE_LOADER` thread-local for `value_parser` callbacks. First consumer: `ppba-driver` (CLI help + errors). See [`i18n.md`](plans/i18n.md) and [`i18n-cli-spike.md`](plans/archive/i18n-cli-spike.md). |
| [rusty-photon-i18n-derive](../crates/rusty-photon-i18n-derive/) | `crates/rusty-photon-i18n-derive` | Companion proc-macro crate. `#[derive(LocalizedParser)]` reads `#[localized(about = "key")]` / `#[localized(help = "key")]` attributes alongside `#[derive(Parser)]` and emits a `parse_localized(loader)` impl that mutates the clap `Command` before parse. Re-exported via `rusty_photon_i18n::LocalizedParser`. |
| [rusty-photon-shared-transport](../crates/rusty-photon-shared-transport/) | `crates/rusty-photon-shared-transport` | Refcounted multi-client lifecycle scaffolding for duplex transports (serial + UDP): `SharedTransport<Codec>`, the `TransportFactory` trait, and background polling. Basis of the shared-transport driver pattern (first adopter: `dsd-fp2`). |
| [rusty-photon-driver](../crates/rusty-photon-driver/) | `crates/rusty-photon-driver` | Shared ASCOM-driver runtime layer: the common `DriverError` model, its ASCOM error-code mapping, and the generic `config.get`/`apply`/`schema` action dispatch. See [ADR-007](decisions/007-rusty-photon-driver-shared-crate.md). |
| [rusty-photon-config](../crates/rusty-photon-config/) | `crates/rusty-photon-config` | Shared config-path resolution, first-run `UniqueID` materialization, and the `config.get`/`apply`/`schema` action protocol for rusty-photon drivers. See [config-actions.md](services/config-actions.md). |
| [rusty-photon-service-lifecycle](../crates/rusty-photon-service-lifecycle/) | `crates/rusty-photon-service-lifecycle` | Unified service lifecycle: tokio runtime + signal handlers + optional Windows SCM, exposing a single `Shutdown` handle across the workspace. See [`docs/crates/rusty-photon-service-lifecycle.md`](crates/rusty-photon-service-lifecycle.md). |
| [rp-fits](../crates/rp-fits/) | `crates/rp-fits` | FITS reader/writer wrapper (pure-Rust `fitsrs`) for Rusty Photon services. See [ADR-001](decisions/001-fits-file-support.md). |
| [rp-plate-solver](../crates/rp-plate-solver/) | `crates/rp-plate-solver` | HTTP client for the `plate-solver` rp-managed service, used by `rp`'s `plate_solve` MCP tool. See [ADR-005](decisions/005-plate-solver.md). |
| [qhyccd-rs](../crates/qhyccd-rs/) | `crates/qhyccd-rs` (+ nested `libqhyccd-sys`) | Vendored first-party safe bindings for the proprietary QHYCCD SDK; `libqhyccd-sys` holds the raw FFI. Used by `qhy-camera`. See [ADR-009](decisions/009-vendor-qhyccd-rs.md). |
| [zwo-rs](../crates/zwo-rs/) | `crates/zwo-rs` (+ nested `libzwo-sys`) | Vendored first-party safe bindings for the ZWO ASI camera + EFW filter-wheel SDK (MIT); `libzwo-sys` holds the raw FFI. Used by `zwo-camera`. See [ADR-008](decisions/008-zwo-camera-native-sdk-ffi.md) + [ADR-010](decisions/010-vendor-zwo-rs.md). |

## Inter-Service Communication: MCP via `rmcp`

`rp` communicates with orchestrator plugins (e.g., `calibrator-flats`) using the
[Model Context Protocol](https://modelcontextprotocol.io/) (MCP). MCP was chosen
so that both the server (`rp`) and clients (plugins) can use standard,
well-maintained crates instead of hand-rolling JSON-RPC.

The workspace uses [`rmcp`](https://crates.io/crates/rmcp) (the official MCP Rust
SDK from the modelcontextprotocol org). Key reasons for choosing `rmcp`:

- **Official SDK** — maintained by the modelcontextprotocol org, tracks spec
  changes first
- **Both roles, one crate** — `"server"` and `"client"` feature flags on the
  same crate, sharing types
- **Composable HTTP** — `StreamableHttpService` implements Tower `Service`, so
  it mounts on `rp`'s existing axum router via
  `Router::nest_service("/mcp", ...)`
- **Dependency alignment** — uses axum 0.8 and reqwest 0.13, matching the
  workspace
- **Ergonomic tool definitions** — `#[tool]` derive macro on impl methods

Workspace dependency (in root `Cargo.toml`):
```toml
rmcp = { version = "1.7", default-features = false }
```

Service feature selections:
- `rp`: `features = ["server", "macros", "transport-streamable-http-server", "schemars"]`
- `calibrator-flats`: `features = ["client", "transport-streamable-http-client-reqwest"]`

`schemars` 1.0 is also a workspace dependency — rmcp's `#[tool]` macro
generates JSON Schema from parameter structs via `schemars::JsonSchema`.

## Shared Architecture Patterns

### Serial-based services (ppba-driver, qhy-focuser)

```
config.rs         — Configuration types and JSON loading
config_actions.rs — `config.get` / `config.apply` / `config.schema` action handlers
error.rs          — Service-specific error enum (thiserror)
serial.rs         — tokio-serial-backed `TransportFactory` (wraps the port in a `SerialFrameTransport`)
codec.rs          — `Codec` adapter: device wire frames ⇄ `SharedTransport`
mock.rs           — In-memory mock `TransportFactory` (cfg(feature = "mock"))
protocol.rs       — Wire-format encode/decode for the device's serial protocol
manager.rs        — Thin wrapper over `rusty_photon_shared_transport::SharedTransport` (refcounted connect + background polling + cached state)
*_device.rs       — ASCOM trait implementation
lib.rs            — ServerBuilder (CLI args → server)
main.rs           — Entry point
```

The legacy per-service `io.rs` traits and `serial_manager.rs` are gone — the
refcounted connection lifecycle and the `TransportFactory` / `Codec` traits now
live in the
[`rusty-photon-shared-transport`](../crates/rusty-photon-shared-transport/)
crate; each service keeps only its handshake, poll body, and cached state.

ppba-driver additionally has `switches.rs` (Switch device wiring) and
`mean.rs` (running-mean smoothing for ObservingConditions readings); its device
files are `observingconditions_device.rs` + `switch_device.rs`.

### HTTP gateway services (rp)

```
config/              — Configuration types + loading (camera/mount/focuser/site/… submodules)
error.rs             — RpError enum + Result alias (thiserror)
equipment/           — EquipmentRegistry + ASCOM Alpaca client (per-device submodules)
events.rs            — EventBus, webhook + SSE delivery
imaging/             — FITS read/write, pixel statistics, analysis + tools
mcp/                 — rmcp tool_router: #[tool] methods, ServerHandler impl
persistence/         — redb document store + FITS cache (cache/document/fits)
planner/             — Observation planning (catalog/decision/primitives/convenience)
session.rs           — SessionManager, orchestrator invocation
routes.rs            — Axum router (REST + MCP + SSE endpoints)
hash_password_cmd.rs — `rp hash-password` subcommand (Argon2id hashing)
tls_cmd.rs           — `rp init-tls` subcommand (CA + per-service certs)
lib.rs               — ServerBuilder (two-phase: build → start)
main.rs              — Entry point
```

### Orchestrator plugins (calibrator-flats)

Plugins act as MCP clients of `rp` and expose an HTTP `/invoke` endpoint that
`rp` calls when a session is started.

```
config.rs    — Plugin config + FlatPlan request schema
error.rs     — CalibratorFlatsError enum
mcp_client.rs — rmcp StreamableHttpClient wrapper for calling rp's tools
workflow.rs  — Iterative exposure optimization + batch capture state machine
routes.rs    — Axum router: GET /health, POST /invoke
lib.rs       — Plugin server bootstrap
main.rs      — Entry point
```

### Monitoring service (sentinel)

`sentinel` is a standalone Axum + reqwest backend. The dashboard at
`http://127.0.0.1:11114/` is hand-rolled HTML built with `format!()` in
`services/sentinel/src/dashboard.rs`, refreshed client-side by a vanilla
`fetch()` loop hitting `/api/status` and `/api/history` every five seconds.

```
sentinel/src/
  config.rs        — Config types: monitors, notifiers, dashboard
  error.rs         — SentinelError enum
  io.rs            — HTTP client trait abstraction (testability)
  alpaca_client.rs — ASCOM Alpaca SafetyMonitor client
  monitor.rs       — Monitor trait + state types
  pushover.rs      — Pushover notifier
  notifier.rs      — Notifier trait
  state.rs         — Shared monitor status + notification history
  engine.rs        — Orchestrates monitors, transitions, notifiers
  watchdog.rs      — Operation watchdog (predictive-deadlines Phase 4)
  corrective.rs    — Corrective-action ladder (predictive-deadlines Phase 5)
  dashboard.rs     — Axum routes for JSON API + dashboard HTML
  lib.rs / main.rs — Server bootstrap and entry point
```

> A `sentinel-app` Leptos/WASM crate was scaffolded as an alternative
> dashboard frontend and later abandoned in favour of the hand-rolled UI
> above (and the `ui-htmx` direction for config UIs). It was removed in
> 2026-06; see
> [docs/plans/archive/sentinel-app-leptos-dashboard.md](plans/archive/sentinel-app-leptos-dashboard.md).

## MSRV

The minimum supported Rust version is pinned in `[workspace.package]` of the
root `Cargo.toml` (`rust-version = "1.94.1"`). Every member listed in
`[workspace].members` — all services and shared crates — inherits it via
`rust-version.workspace = true`.

## Workspace Dependencies

Dependencies used by two or more services are declared in the workspace
`Cargo.toml` under `[workspace.dependencies]` (CLAUDE.md Rule 10). Services
reference them with `dep.workspace = true`.

### Dual-homed crates inherit shared deps too

The dual-homed members (`zwo-rs` + `libzwo-sys`, `qhyccd-rs` + `libqhyccd-sys` —
ADR-009/010) follow the same rule: their **shared** third-party dependencies
(e.g. `thiserror`, `tracing`, and the simulation-only `rand`/`rayon` shared
between the two camera crates) inherit from `[workspace.dependencies]` with
`dep.workspace = true`. This is safe for their independent crates.io releases
because `cargo publish` **flattens** an inherited dependency into a concrete
version in the packaged manifest (verified by dry-run). What stays explicit on
these members is their **package identity metadata** (`version` / `edition` /
`license` / `authors` / `description` / `keywords` / `categories`) — *not*
`*.workspace = true` — so they release on their own cadence (the carve-out
recorded in ADR-009/010). A dep is left crate-local only when it is genuinely
single-consumer (e.g. `libzwo-sys`'s `bindgen` build-dep) or when the workspace
pin would force an unwanted feature (e.g. `qhyccd-rs` keeps `tracing-subscriber`
local to avoid the workspace's `env-filter`).

### Pre-commit hooks

The workspace uses `cargo-husky` as a dev-dependency configured with
`default-features = false` and the `precommit-hook` + `user-hooks` features
(see root `Cargo.toml`). The `user-hooks` feature tells `cargo-husky` to
install a custom hook script kept in the repo at
`.cargo-husky/hooks/pre-commit`, which currently runs:

```sh
cargo clippy --all --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
# Buildifier (BUILD / *.bzl / MODULE.bazel formatting + lint) — the same gate CI
# runs. Guarded on bazel being installed, so Cargo-only devs aren't blocked:
bazel test //:buildifier_check
```

The hook is installed automatically the first time any test build pulls
`cargo-husky` in as a dev-dependency.

## Coding Conventions

### Duration Units

**Durations are `std::time::Duration` system-wide.** Any field, parameter,
return value, or struct member that represents a time interval uses
`Duration` end-to-end — config, internal state, MCP tool parameters,
inter-service wire payloads, and (where types allow) telemetry. Integer
representations of duration (`u32 ms`, `u64 ms`, `u64 secs`) do **not**
appear in internal data structures; they exist only as transient values
at boundaries that demand them (third-party SDKs, JSON-RPC payloads
with a fixed wire schema, sentinel/dashboard JSON serialisation of
already-elapsed magnitudes).

**Precision floor: microseconds.** The system-wide precision contract
is 1 µs. This is finer than what most observing workflows need but
matches the actual minimum exposure of modern CMOS sensors (QHY174
~50 µs, QHY600 ~10 µs, ZWO ASI line ~32 µs). It is required for **bias
frames**, which use the camera's true minimum exposure to capture the
read-noise floor — a 1 ms floor would expose 20–100× longer than the
sensor's minimum and accumulate dark current that contaminates the
bias. Sub-microsecond precision is not required: ASCOM Alpaca's
`Camera.StartExposure` Duration is an `f64` in seconds (so the
protocol can express it), but no current sensor honours it, and
QHY's nanosecond-resolution SDK API offers no observable advantage
at this precision.

For **config types** (anything deserialised from a JSON config file),
use `std::time::Duration` with the `humantime-serde` adapter and **no
unit suffix in the field name**:

```rust
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileConfig {
    pub path: PathBuf,
    #[serde(with = "humantime_serde", default = "default_polling_interval")]
    pub polling_interval: Duration,
}

fn default_polling_interval() -> Duration {
    Duration::from_secs(60)
}
```

The wire format is a humantime string (`"60s"`, `"500ms"`, `"50us"`,
`"1m30s"`, `"2h"`). The unit lives in the value, not the field name —
the type already says `Duration` and the value already says the unit.
This removes the previous `_ms` vs `_secs` ambiguity in field names.

`humantime` accepts both compact forms (`"5m"`) and combinations
(`"1m30s500ms"`). It rejects bare integers (`"30"` is invalid — must be
`"30s"` or `"30ms"`).

For raw integer fields that are still magnitudes of time but **not**
internal `Duration`s (e.g. dashboard JSON serialising an elapsed
magnitude, or a `u64` epoch millisecond timestamp), keep the unit
suffix on the field name (`last_poll_epoch_ms`, `elapsed_ms`) so a
reader can tell the unit at the call site.

**Boundary conversions.** When a `Duration` must be flattened to an
integer or string for a third-party wire format, do it at the boundary
only — never store the integer back into an internal struct. Use
`humantime::format_duration(d)` to render a `Duration` to a humantime
string preserving µs precision (instead of `format!("{}ms",
d.as_millis())`, which collapses sub-ms values to `"0ms"`). When the
external schema demands a bare integer (e.g. PHD2's `time` and
`timeout` settle keys), apply whatever rounding the wire format
requires at the `json!` site — `.as_micros()` / `.as_millis()` /
`.as_secs()` when truncation is acceptable, or a boundary helper such
as `settle_secs_ceil` when sub-second values must round up instead of
truncating to `0`. See `services/phd2-guider/src/client.rs` for the
worked example.

## Feature Flags

- **`mock`** — Enables an in-memory mock factory with persistent device state
  for integration testing (ConformU, server tests); not used for unit tests,
  which define inline mocks. The serial drivers expose a per-service mock
  `TransportFactory` (`ppba-driver` → `MockPpbaTransportFactory`, `qhy-focuser`
  → `MockQhyTransportFactory`). Declared by `ppba-driver`, `qhy-focuser`,
  `pa-falcon-rotator`, `dsd-fp2`, `star-adventurer-gti`, `sky-survey-camera`
  (`mock = []`), the camera drivers `qhy-camera` / `zwo-camera`
  (`mock = ["simulation"]`), and `rp-plate-solver` (`mock = ["dep:mockall"]`).

## Build Notes

- The `ascom-alpaca` crate is a git dependency from
  `ivonnyssen/ascom-alpaca-rs.git` (branch `integration`,
  `default-features = false`).

### Bazel

Bazel is the per-PR build / test / coverage gate. `Cargo.toml` and `Cargo.lock`
remain the single source of truth for dependency versions, and Bazel's
`crate_universe` reads them. The repo root holds `MODULE.bazel` and `BUILD.bazel`;
`bazel test //...` runs all non-`requires-cargo`, non-BDD targets, and
`bazel test --test_tag_filters=bdd //...` adds the BDD suites. The required PR
checks are `bazel / <os>` (build + test on Linux/macOS/Windows), `bazel coverage`,
plus the Cargo `stable / fmt` and `stable / clippy` lint jobs (Bazel does not run
rustfmt/clippy). `bazel/cargo target parity` and the Cargo build/test jobs run
nightly as a safety net (coverage is Bazel-only). `bazel build //... && bazel test //...` is
the local pre-commit loop (see [docs/skills/pre-push.md](skills/pre-push.md)).

After adding a crates.io dependency to the workspace, run
`CARGO_BAZEL_REPIN=1 bazel mod tidy && bazel mod tidy` to refresh
`MODULE.bazel.lock` before committing. The second, un-forced `bazel mod tidy`
resets the lock's recorded `CARGO_BAZEL_REPIN` env fingerprint to `null` so the
committed lock doesn't churn on later plain `bazel` runs.
