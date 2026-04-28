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
| [calibrator-flats](services/calibrator-flats.md) | — (orchestrator plugin) | 11170 | `docs/services/calibrator-flats.md` |
| sentinel-app | — (Leptos frontend; `cargo leptos` build target `sentinel-dashboard`) | served by sentinel on 11114 | — |

## Documentation Index

| Document | Purpose |
|----------|---------|
| **Rules** | |
| [docs/AGENTS.md](AGENTS.md) | Rules for all AI agents and human operators (`CLAUDE.md` is a symlink to this file) |
| **Skills** (how-to playbooks — read before performing the respective task) | |
| [docs/skills/development-workflow.md](skills/development-workflow.md) | Skill: design-first, test-first development workflow |
| [docs/skills/testing.md](skills/testing.md) | Skill: writing and organizing tests (test pyramid, BDD, unit tests) |
| [docs/skills/pre-push.md](skills/pre-push.md) | Skill: running CI quality gates before pushing |
| **References** | |
| [docs/references/ascom-alpaca.md](references/ascom-alpaca.md) | ASCOM Alpaca protocol reference |
| **Decisions** (Architecture Decision Records — see [docs/decisions/](decisions/)) | |
| [ADR-001](decisions/001-fits-file-support.md) | FITS file support |
| [ADR-002](decisions/002-tls-for-inter-service-communication.md) | TLS for inter-service communication |
| [ADR-003](decisions/003-authentication-for-device-access.md) | Authentication for device access |
| **Plans** (in-flight initiatives — see [docs/plans/](plans/)) | |
| [bazel-migration.md](plans/bazel-migration.md) | Bazel build alongside Cargo (shadow mode) |
| [build-optimization-test-reorganization.md](plans/build-optimization-test-reorganization.md) | Build/test reorganization |
| [duration-naming-convention.md](plans/duration-naming-convention.md) | Rename `_seconds` → `_secs` and bare duration fields |
| [duration-type-migration.md](plans/duration-type-migration.md) | Follow-up: switch config duration fields to `std::time::Duration` |
| [filemonitor-packaging.md](plans/filemonitor-packaging.md) | Filemonitor OS packaging |
| [migrate-test-harnesses-to-bdd-infra.md](plans/migrate-test-harnesses-to-bdd-infra.md) | Migrate per-service BDD harnesses to `bdd-infra` |

## Shared Crates

| Crate | Location | Purpose |
|-------|----------|---------|
| [bdd-infra](../crates/bdd-infra/) | `crates/bdd-infra` | Shared BDD test infrastructure: `ServiceHandle` for spawning, managing, and stopping service binaries. Config read from each service's `[package.metadata.bdd]` in Cargo.toml. See [testing.md](skills/testing.md) Section 5.1. |
| [rp-tls](../crates/rp-tls/) | `crates/rp-tls` | Opt-in TLS for inter-service communication: certificate generation, dual-stack TCP binding, TLS/plain serving, and client CA trust. See [ADR-002](decisions/002-tls-for-inter-service-communication.md). |
| [rp-auth](../crates/rp-auth/) | `crates/rp-auth` | Opt-in HTTP Basic Auth: Argon2id credential hashing/verification, axum tower middleware, and config types. See [ADR-003](decisions/003-authentication-for-device-access.md). |

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
rmcp = { version = "1.4", default-features = false }
```

Service feature selections:
- `rp`: `features = ["server", "macros", "transport-streamable-http-server", "schemars"]`
- `calibrator-flats`: `features = ["client", "transport-streamable-http-client-reqwest"]`

`schemars` 1.0 is also a workspace dependency — rmcp's `#[tool]` macro
generates JSON Schema from parameter structs via `schemars::JsonSchema`.

## Shared Architecture Patterns

### Serial-based services (ppba-driver, qhy-focuser)

```
config.rs      — Configuration types and JSON loading
error.rs       — Service-specific error enum (thiserror)
io.rs          — Traits (SerialReader, SerialWriter, SerialPortFactory)
serial.rs      — tokio-serial implementation of the traits
mock.rs        — In-memory mock factory (cfg(feature = "mock"))
protocol.rs    — Wire-format encode/decode for the device's serial protocol
serial_manager.rs — Ref-counted connection + background polling
*_device.rs    — ASCOM trait implementation
lib.rs         — ServerBuilder (CLI args → server)
main.rs        — Entry point
```

ppba-driver additionally has `switches.rs` (Switch device wiring) and
`mean.rs` (running-mean smoothing for ObservingConditions readings).

### HTTP gateway services (rp)

```
config.rs            — Configuration types and loading
error.rs             — RpError enum + Result alias (thiserror)
equipment.rs         — EquipmentRegistry, ASCOM Alpaca client
events.rs            — EventBus, webhook delivery
imaging.rs           — FITS read/write and pixel statistics
mcp.rs               — rmcp tool_router: #[tool] methods, ServerHandler impl
session.rs           — SessionManager, orchestrator invocation
routes.rs            — Axum router (REST + MCP endpoints)
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

### Monitoring service + Leptos frontend (sentinel, sentinel-app)

`sentinel` is the standalone Axum + reqwest backend. `sentinel-app` is a
separate Leptos crate (`cdylib + rlib`) wired to `sentinel` via the
`[[workspace.metadata.leptos]]` block at the workspace root, intended to be
built and served through `cargo leptos` (build target name
`sentinel-dashboard`). At present the `sentinel` binary does not directly
depend on the `sentinel-app` crate — the integration is declared but not
yet wired into the `sentinel` Axum router.

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
  dashboard.rs     — Axum routes for JSON API + dashboard HTML
  lib.rs / main.rs — Server bootstrap and entry point

sentinel-app/src/
  lib.rs           — Crate root (re-exports App)
  app.rs           — Root Leptos component
  api.rs           — Client-side API helpers
  components/      — monitor_table, history_table, status_badge
```

## MSRV

The minimum supported Rust version is pinned in `[workspace.package]` of the
root `Cargo.toml` (`rust-version = "1.94.1"`). All workspace members —
services (`filemonitor`, `phd2-guider`, `ppba-driver`, `qhy-focuser`,
`calibrator-flats`, `rp`, `sentinel`, `sentinel-app`) and shared crates
(`bdd-infra`, `rp-auth`, `rp-tls`) — inherit it via
`rust-version.workspace = true`.

## Workspace Dependencies

Dependencies used by two or more services are declared in the workspace
`Cargo.toml` under `[workspace.dependencies]` (CLAUDE.md Rule 10). Services
reference them with `dep.workspace = true`.

### Pre-commit hooks

The workspace uses `cargo-husky` as a dev-dependency configured with
`default-features = false` and the `precommit-hook` + `user-hooks` features
(see root `Cargo.toml`). The `user-hooks` feature tells `cargo-husky` to
install a custom hook script kept in the repo at
`.cargo-husky/hooks/pre-commit`, which currently runs:

```sh
cargo clippy --all --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

The hook is installed automatically the first time any test build pulls
`cargo-husky` in as a dev-dependency.

## Coding Conventions

### Duration Units

Use the unit that is natural for the magnitude and **always include
the unit suffix in the field name**. This eliminates ambiguity without
forcing unnatural values.

- **`_ms`** for sub-second precision: `duration_ms`, `exposure_min_ms`,
  `exposure_max_ms`, `initial_duration_ms`
- **`_secs`** for human-facing config values: `poll_interval_secs`,
  `polling_interval_secs`, `settle_time_secs`, `timeout_secs`

Never use a bare `duration` or `timeout` field without a unit suffix.

## Feature Flags

- **`mock`** — Enables `MockSerialPortFactory` with persistent state for
  integration testing (ConformU, server tests). Used by ppba-driver and
  qhy-focuser. Not used for unit tests — those define inline mocks.
- **`hydrate`** / **`ssr`** — Leptos rendering modes for `sentinel-app`.
  `ssr` is intended for native compilation linked into a server binary;
  `hydrate` is intended for `wasm32-unknown-unknown` + wasm-bindgen
  client-side hydration. The `sentinel` binary does not yet link
  `sentinel-app` in either mode — `cargo build -p sentinel-app` exercises
  the crate in isolation pending the integration work.

## Build Notes

- The `ascom-alpaca` crate is a git dependency from
  `ivonnyssen/ascom-alpaca-rs.git` (branch `integration`,
  `default-features = false`).

### Bazel (shadow mode)

A Bazel build is being introduced alongside Cargo — see
[docs/plans/bazel-migration.md](plans/bazel-migration.md). Cargo remains the
canonical build system during the migration: `Cargo.toml` and `Cargo.lock`
are the single source of truth for dependency versions, and Bazel's
`crate_universe` reads them. The repo root holds `MODULE.bazel` and
`BUILD.bazel`; `bazel test //...` runs all non-`requires-cargo`, non-BDD
targets. Bazel is **not** a required pre-push gate yet — the canonical
pre-push command is still `cargo rail run --merge-base` (see
[docs/skills/pre-push.md](skills/pre-push.md)).

After adding a crates.io dependency to the workspace, run
`CARGO_BAZEL_REPIN=1 bazel mod tidy` to refresh `MODULE.bazel.lock` before
committing.
