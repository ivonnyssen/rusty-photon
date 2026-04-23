# Plan: Migrate remaining test harnesses to bdd-infra

**Date:** 2026-04-23
**Depends on:** PR #89 (removing the cargo-run fallback from bdd-infra)

## Context

PR #89 established a contract for bdd-infra: **never invoke cargo from inside a test**. Production paths (`ServiceHandle::start`, `try_start`, `run_once`) and the `ensure_rp_binary` test helper all panic with a clear diagnostic rather than shelling out to `cargo build` or `cargo run`.

Eight `Command::new("cargo")` sites still remain in the workspace, across two surfaces that predate bdd-infra and have not been migrated:

| Surface | Files | Sites |
|---|---|---|
| filemonitor CLI tests | `services/filemonitor/tests/test_cli.rs` | 4 |
| ConformU integration tests | `services/{filemonitor,ppba-driver,qhy-focuser}/tests/conformu_integration.rs` | 4 |

Both reimplement subsets of what bdd-infra already provides (binary spawning, stdout capture, port parsing, graceful shutdown). Migrating them gets us three wins:

1. **One contract.** Every test in the workspace discovers binaries the same way and never secretly builds them.
2. **Less code.** Each site loses 10–20 lines of boilerplate (manual `Command` construction, stdout handling, child-cleanup dances).
3. **Bazel hermeticity.** `test_cli.rs` currently carries `tags = ["requires-cargo"]` in `services/filemonitor/BUILD.bazel` and is listed under "Known test gaps" in `docs/plans/bazel-migration.md`. After migration it can join the regular Bazel test graph with `data = [":filemonitor"]` + `env = {"FILEMONITOR_BINARY": "$(rootpath :filemonitor)"}`.

## Scope

Two phases, one per surface. Each phase is independently shippable and reviewable.

### Phase 1 — filemonitor `test_cli.rs`

The 4 tests spawn `cargo run --bin filemonitor` with different argument sets (`--help`, a valid config, an invalid config, varying log levels) and assert on stdout/stderr/exit-status. This is exactly what `bdd_infra::run_once(package_name, args, stdin)` is for — it's how rp's `init-tls` and `hash-password` tests work today.

- [ ] Rewrite each `Command::new("cargo").args(["run", "--", …])` call as `bdd_infra::run_once("filemonitor", &[…], None)`.
- [ ] Pre-build filemonitor in CI before the test binary runs. The workspace build step on `ubuntu / stable` / `macos` / `windows / nextest` already does this for all packages; no CI change needed for Cargo.
- [ ] Verify `filemonitor`'s clap parser accepts the flags the tests pass. The BDD suite uses `--config`; `test_cli.rs` uses a mix of short and long forms — add clap aliases if anything's missing.
- [ ] Update `services/filemonitor/BUILD.bazel`:
  - Drop `tags = ["requires-cargo"]` from the `test_cli` target.
  - Add `data = [":filemonitor"]` + `env = {"FILEMONITOR_BINARY": "$(rootpath :filemonitor)"}` (mirroring the `bdd` target's wiring).
  - Add `rustc_env = {"CARGO_PKG_NAME": "filemonitor"}` so `bdd_infra::run_once` derives the right env-var name under Bazel.
- [ ] Remove the `//services/filemonitor:test_cli` bullet from `docs/plans/bazel-migration.md`'s "Known test gaps" section.

**Exit criteria:** `bazel test //services/filemonitor:test_cli` passes without `requires-cargo`; `cargo test -p filemonitor --test test_cli` still passes on Linux/macOS/Windows CI; no `Command::new("cargo")` remains in `test_cli.rs`.

### Phase 2 — ConformU integration tests

The 4 sites (filemonitor ×1, ppba-driver ×2, qhy-focuser ×1) are all near-identical:

```rust
let mut child = Command::new("cargo").args(["run", "--", "-c", cfg]).spawn()?;
let stdout = child.stdout.take()?;
let (port, drain) = bdd_infra::parse_bound_port(stdout).await?;
run_conformu_tests(&format!("http://localhost:{}", port), 0).await?;
let _ = child.kill().await;
drain.abort();
```

That's `ServiceHandle::start` open-coded, with `child.kill()` replacing the graceful SIGTERM that `handle.stop()` does. Replacing with the helper:

```rust
let mut handle = ServiceHandle::start("filemonitor", cfg_path).await;
run_conformu_tests(&format!("http://localhost:{}", handle.port), 0).await?;
handle.stop().await;
```

- [ ] Replace each manual spawn + `parse_bound_port` + `child.kill()` trio with `ServiceHandle::start` + `handle.stop()`.
- [ ] The `-c <path>` short-form flag must survive — these binaries already use it. Either add a `--config` alias in each service's clap config (recommended, for consistency with `ServiceHandle`'s `--config` call), or have a variant of `ServiceHandle` that accepts a custom flag (not worth it for one edge case).
- [ ] Verify the scheduled conformu workflow (`.github/workflows/conformu.yml`) still passes. It's the only CI surface that runs these tests (they carry `#[ignore]`).

**Exit criteria:** `scheduled / conformu` workflow green on its next run; no `Command::new("cargo")` remains in any `conformu_integration.rs`; each test shrinks by ~15 lines.

## Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| Service CLIs don't accept `--config` long form | Medium | Add `#[arg(long = "config", short = 'c')]` alias in clap definitions; one-line change per service. |
| ConformU workflow regressions caught late | Medium | Workflow is scheduled (nightly-ish). Trigger it manually via `workflow_dispatch` after Phase 2 merges before claiming done. |
| Bazel `test_cli` target misses a subtle Cargo-layout assumption | Low | Run `bazel test //services/filemonitor:test_cli` locally on Linux/macOS/Windows before merging Phase 1. |

## Non-goals

- Moving test_cli or conformu_integration to other homes (e.g., into the BDD suite). They stay as standalone integration tests; only the process-spawn mechanism changes.
- Rewiring the scheduled ConformU workflow's external-tool installation logic. Orthogonal.
- Touching `services/phd2-guider/tests/test_integration.rs` — it uses `env!("CARGO_MANIFEST_DIR")` for path construction only and does not shell out to cargo. Nothing to migrate.

## Success metrics

- Zero `Command::new("cargo")` in the workspace after both phases land.
- `docs/plans/bazel-migration.md`'s "Known test gaps" section loses the `test_cli` entry.
- No behavior change visible in CI: all existing green checks stay green, conformu workflow continues to pass.
