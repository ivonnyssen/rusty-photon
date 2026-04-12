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
| sentinel-app | — (Leptos web frontend for sentinel) | — | — |

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
| **Decisions** | |
| [docs/decisions/](decisions/) | Architecture Decision Records |

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
io.rs          — Traits (SerialReader, SerialWriter, SerialPortFactory)
serial.rs      — tokio-serial implementation of the traits
serial_manager.rs — Ref-counted connection + background polling
*_device.rs    — ASCOM trait implementation
lib.rs         — ServerBuilder (CLI args → server)
main.rs        — Entry point
```

### HTTP gateway services (rp)

```
config.rs      — Configuration types and loading
equipment.rs   — EquipmentRegistry, ASCOM Alpaca client
events.rs      — EventBus, webhook delivery
mcp.rs         — rmcp tool_router: #[tool] methods, ServerHandler impl
session.rs     — SessionManager, orchestrator invocation
routes.rs      — Axum router (REST + MCP endpoints)
lib.rs         — ServerBuilder (two-phase: build → start)
main.rs        — Entry point
```

## MSRV

| Service | rust-version |
|---------|-------------|
| phd2-guider | 1.85.0 |
| filemonitor, ppba-driver, qhy-focuser, rp, sentinel, sentinel-app | 1.88.0 |

## Workspace Dependencies

Dependencies used by two or more services are declared in the workspace
`Cargo.toml` under `[workspace.dependencies]` (CLAUDE.md Rule 10). Services
reference them with `dep.workspace = true`.

### Pre-commit hooks

The workspace uses `cargo-husky` as a dev-dependency with `precommit-hook`,
`run-cargo-fmt`, `run-cargo-clippy`, and `run-for-all` features. This
automatically installs a git pre-commit hook that runs `cargo fmt` and
`cargo clippy` on every commit.

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
- **`hydrate`** / **`ssr`** — Leptos rendering modes for sentinel-app.
  `ssr` is used by the sentinel binary for server-side rendering; `hydrate`
  is for future WASM-hydrated frontend builds.

## Build Notes

- The `ascom-alpaca` crate is a git dependency from
  `ivonnyssen/ascom-alpaca-rs.git` (branch `fix/macos-trait-recursion-overflow`).
