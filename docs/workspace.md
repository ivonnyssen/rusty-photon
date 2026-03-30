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
mcp.rs         — JSON-RPC 2.0 dispatcher, tool handlers
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
