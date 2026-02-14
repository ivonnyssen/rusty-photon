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

## Documentation Index

| Document | Purpose |
|----------|---------|
| [CLAUDE.md](../CLAUDE.md) | Rules for Claude Code |
| [docs/AGENTS.md](AGENTS.md) | Rules for Kiro CLI |
| [docs/testing-rules.md](testing-rules.md) | Test pyramid, BDD conventions, unit test rules |
| [docs/ascom-alpaca.md](ascom-alpaca.md) | ASCOM Alpaca protocol reference |
| [docs/pre-push-checklist.md](pre-push-checklist.md) | CI checks mapped to local commands |
| [CI_LOCAL.md](../CI_LOCAL.md) | Running CI workflows locally with `act` |
| [docs/decisions/](decisions/) | Architecture Decision Records |

## Shared Architecture Patterns

All serial-based services (ppba-driver, qhy-focuser) follow a common layered
architecture. See individual design docs for details; the shared pattern is:

```
io.rs          — Traits (SerialReader, SerialWriter, SerialPortFactory)
serial.rs      — tokio-serial implementation of the traits
serial_manager.rs — Ref-counted connection + background polling
*_device.rs    — ASCOM trait implementation
lib.rs         — ServerBuilder (CLI args → server)
main.rs        — Entry point
```

## Workspace Dependencies

Dependencies used by two or more services are declared in the workspace
`Cargo.toml` under `[workspace.dependencies]` (CLAUDE.md Rule 10). Services
reference them with `dep.workspace = true`.

## Feature Flags

- **`mock`** — Enables `MockSerialPortFactory` with persistent state for
  integration testing (ConformU, server tests). Used by ppba-driver and
  qhy-focuser. Not used for unit tests — those define inline mocks.

## Build Notes

- The `filemonitor` crate depends on `fitsio-sys` which requires the `cfitsio`
  system library (`libcfitsio-dev` on Ubuntu, `cfitsio` via Homebrew on macOS).
  Use `-p <package>` to build other services when cfitsio is not installed.
- The `ascom-alpaca` crate is a git dependency from
  `ivonnyssen/ascom-alpaca-rs.git` (branch `feature/conformu-settings-file`).
