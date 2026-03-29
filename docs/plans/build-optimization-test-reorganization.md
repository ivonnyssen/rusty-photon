# Plan: Reorganize Test Suite to Reduce Build Times

**Date:** 2026-03-28 (updated after TLS merge into main)
**Baseline:** clean build 2m 46s, 55 test binaries, 1050s total CPU time

## Context

A clean `cargo build --all --all-targets --all-features` takes **2m 46s**. 52% of CPU time (547s of 1050s) goes to compiling **55 test binaries** -- each `.rs` file in `tests/` becomes its own binary that independently compiles and links. An audit found most of these files are unit or mock-based component tests that belong in `src/` as `#[cfg(test)]` modules, not in `tests/`.

### Build timing breakdown (from `cargo build --timings`, 2026-03-28)

| Category | CPU time | Share |
|----------|----------|-------|
| Dependencies | 345.4s | 33% |
| Workspace code (non-test) | 157.6s | 15% |
| **Workspace test binaries** | **547.1s** | **52%** |

### Test binary count per crate

| Crate | Test binaries | Test compile time |
|-------|--------------|-------------------|
| phd2-guider | 15 | 28.2s |
| ppba-driver | 11 | 131.4s |
| qhy-focuser | 9 | 115.8s |
| filemonitor | 8 | 125.5s |
| rp | 3 | 80.6s |
| sentinel | 3 | 46.1s |
| rp-tls | 2 | 14.7s |
| bdd-infra | 3 | 3.9s |
| sentinel-app | 1 | 0.9s |

### Critical path observations

The build finishes at 166s. The last items to complete are:
- `ppba-driver test "bdd"` ends at 166.0s
- `qhy-focuser test "bdd"` ends at 165.2s
- `ppba-driver test "conformu_integration"` ends at 158.7s
- `ppba-driver test "test_serial_manager"` ends at 156.5s
- `qhy-focuser test "test_config"` ends at 156.1s

The ppba-driver and qhy-focuser unit test binaries compile in the 130-166s range, sitting on the **critical path**. Eliminating them directly reduces wall clock time.

### Other bottlenecks identified

- **aws-lc-sys**: 41.9s building C code via cmake. Blocks rustls -> reqwest -> ascom-alpaca -> all workspace crates. Switching to `ring` was attempted but is not viable for this project.
- **No fast linker**: Neither mold nor lld installed. All link targets use default GNU ld.
- **No debug info optimization**: Full DWARF debuginfo for all crates in dev profile.

These are orthogonal improvements that can be pursued independently of the test reorganization.

---

## Test Audit Results

Each test file in `tests/` was classified (excluding bdd.rs, conformu_integration.rs, and new infrastructure tests):

| Category | Count | Description |
|----------|-------|-------------|
| Pure unit | 13 | Protocol parsing, config defaults, error Display, serialization -- belong in `src/` |
| Mock-based component | 7 | Internal wiring with hand-written mocks -- belong in `src/` |
| Redundant with BDD | 2 | Already covered by existing BDD scenarios -- delete |
| Server/process integration | 7 | Start real servers or spawn processes -- consolidate or migrate to BDD |

Correctly placed (no action needed): 5 bdd.rs, 3 conformu_integration.rs, 1 rp-tls/tls_roundtrip.rs, 1 bdd-infra/service_handle.rs.

---

## Phase 1: Move pure unit tests to `src/` (13 files, eliminates 13 binaries)

These files test internal functions (parsing, config defaults, error Display, serialization) through the crate's public API. They belong as `#[cfg(test)] mod tests` blocks in the corresponding source module.

**Pattern for each move:**
1. Append a `#[cfg(test)] mod tests { use super::*; ... }` block at the bottom of the target `src/` file
2. Copy test functions in, replacing `use ppba_driver::foo::Bar` with `use super::*`
3. Preserve `#[cfg(not(miri))]` guards (file-level `#![]` becomes module-level `#[]`)
4. Delete the `tests/test_X.rs` file
5. Update docs that reference the old file name

### ppba-driver (4 files) -- on critical path, high wall-clock impact

| Delete | Move tests to | Existing `#[cfg(test)]`? |
|--------|--------------|--------------------------|
| `tests/test_protocol.rs` (6.0s) | `src/protocol.rs` | No |
| `tests/test_config.rs` (8.3s) | `src/config.rs` | No |
| `tests/test_error.rs` (6.7s) | `src/error.rs` | No |
| `tests/test_switches.rs` (6.7s) | `src/switches.rs` | No |

### qhy-focuser (3 files) -- on critical path, high wall-clock impact

| Delete | Move tests to | Existing `#[cfg(test)]`? |
|--------|--------------|--------------------------|
| `tests/test_protocol.rs` (12.0s) | `src/protocol.rs` | No |
| `tests/test_config.rs` (9.9s) | `src/config.rs` | No |
| `tests/test_error.rs` (7.2s) | `src/error.rs` | No |

### phd2-guider (5 files) -- compiles early, low wall-clock impact but reduces binary count

| Delete | Move tests to | Existing `#[cfg(test)]`? |
|--------|--------------|--------------------------|
| `tests/test_events.rs` (1.2s) | `src/events.rs` | No |
| `tests/test_types.rs` (1.1s) | `src/types.rs` | No |
| `tests/test_rpc.rs` (1.1s) | `src/rpc.rs` | No |
| `tests/test_config.rs` (0.9s) | `src/config.rs` | No |
| `tests/test_client.rs` (2.0s) | `src/client.rs` | No |

### filemonitor (1 file)

| Delete | Move tests to | Existing `#[cfg(test)]`? |
|--------|--------------|--------------------------|
| `tests/test_property.rs` (14.1s) | `src/lib.rs` (as `mod property_tests`) | No |

### Docs to update
- `docs/skills/testing.md` -- references `test_protocol.rs`, `test_config.rs`, `test_error.rs` by name
- `docs/services/ppba-driver.md` -- lists test files in project structure; test command examples
- `docs/services/phd2-guider.md` -- references test file names
- `docs/services/qhy-focuser.md` -- references test files

**Risk: LOW.** Mechanical move. Tests call only public APIs. `cargo test` immediately catches name collisions or missing imports.

---

## Phase 2: Move mock-based component tests to `src/` (7 files, eliminates 7 binaries)

These test internal wiring using hand-written mocks (MockSerialReader/Writer, MockConnectionFactory, etc.). Moving them to `src/` lets them access `pub(crate)` internals directly.

**Mock duplication strategy:** ppba-driver has identical mock structs across 3 test files. When moved, duplicate them per-module (each `#[cfg(test)]` block gets its own copy). A future cleanup can extract shared mocks if desired.

### ppba-driver (3 files) -- on critical path

| Delete | Move tests to | Module name | Notes |
|--------|--------------|-------------|-------|
| `tests/test_serial_manager.rs` (10.6s) | `src/serial_manager.rs` | `mod tests` | No existing test module. Includes mock structs. |
| `tests/test_switch_device.rs` (6.5s) | `src/switch_device.rs` | `mod tests` | Includes mock structs (duplicated). |
| `tests/test_oc_device.rs` (6.9s) | `src/observingconditions_device.rs` | `mod tests` | Includes mock structs (duplicated). |

### qhy-focuser (1 file) -- on critical path

| Delete | Move tests to | Module name | Notes |
|--------|--------------|-------------|-------|
| `tests/test_serial_manager.rs` (7.5s) | `src/serial_manager.rs` | `mod mock_tests` | **Has existing `#[cfg(test)] mod tests`** at line 541. Add as `#[cfg(all(test, feature = "mock"))] mod mock_tests`. |

### phd2-guider (3 files + test_fits.rs) -- low wall-clock impact

| Delete | Move tests to | Module name | Notes |
|--------|--------------|-------------|-------|
| `tests/test_client_mock.rs` (3.2s) | `src/client.rs` | `mod mock_tests` | Phase 1 creates `mod tests` here; use `mock_tests` to avoid collision. |
| `tests/test_connection_mock.rs` (1.6s) | `src/connection.rs` | `mod mock_tests` | **Has existing `#[cfg(test)] mod tests`** at line 356. |
| `tests/test_process_mock.rs` (2.4s) | `src/process.rs` | `mod tests` | No existing test module. |
| `tests/test_fits.rs` (1.8s) | `src/fits.rs` | `mod tests` | Uses tempfile (dev-dep), tests internal utility functions. |

### Docs to update
- `docs/services/ppba-driver.md` -- references `test_serial_manager.rs`, `test_switch_device.rs`, `test_oc_device.rs`
- `docs/services/phd2-guider.md` -- references `test_client_mock.rs`
- `docs/services/qhy-focuser.md` -- mentions serial_manager tests
- `docs/skills/testing.md` -- references `test_serial_manager.rs`

**Risk: MEDIUM-LOW.** Mock struct duplication requires care. Feature gates (`#[cfg(feature = "mock")]`) must be correctly applied on qhy-focuser.

---

## Phase 3: Consolidate remaining integration tests + delete redundant tests

### filemonitor: Merge 3 files into 1

Combine `test_server.rs` (13.3s), `test_reload.rs` (15.9s), `test_cli.rs` (1.4s) into `tests/test_integration.rs`. Delete `test_cli_invalid_config` (redundant with configuration.feature "Reject invalid configuration sources").

**Result:** 3 files become 1 file with 5 tests. Eliminates 2 binaries.

### qhy-focuser: Delete 1 redundant test

Remove `test_server_returns_configured_focuser_name` from `tests/test_lib.rs` (redundant with device_metadata.feature "Device reports configured name"). File stays with 3 tests.

**Risk: LOW.** Verify BDD scenarios cover the deleted behavior before removing.

---

## Phase 4: phd2-guider integration test consolidation

Consolidate `tests/test_integration.rs` (3.2s) and `tests/test_main_integration.rs` (0.8s) into a single `tests/test_integration.rs`. Both spawn processes and test end-to-end behavior.

Leave `tests/test_mock_server.rs` (4.0s) as-is -- it binds TCP ports and tests client library internals.

**Result:** 3 test files become 2. Eliminates 1 binary.

### Future: phd2-guider BDD suite

The consolidated CLI/integration tests are excellent BDD candidates (they already read as Given/When/Then). This is a separate initiative -- estimated 2-3 days of work to create feature files, step definitions, and BDD infrastructure for phd2-guider.

---

## Summary

| Phase | Binaries eliminated | Risk | Effort |
|-------|-------------------|------|--------|
| 1: Unit tests to `src/` | 13 | Low | 2-3 hours |
| 2: Mock tests to `src/` | 7 | Medium-Low | 3-4 hours |
| 3: Consolidate + delete redundant | 2 | Low | 1-2 hours |
| 4: phd2-guider consolidation | 1 | Low | 1 hour |
| **Total** | **23 of 55** | | **~8 hours** |

Test binary count: **55 -> 32** (42% reduction).

Phases 1 and 2 have the highest wall-clock impact because they eliminate ppba-driver and qhy-focuser test binaries that compile on the **critical path** (130-166s range). Phase 1 alone removes 7 critical-path binaries.

## Verification (after all phases)

```bash
cargo build --all --all-targets --all-features --quiet --color never
cargo test --all --all-features --quiet --color never
cargo fmt
cargo build --all --all-targets --all-features --timings  # compare with baseline 2m46s
```

Each phase is its own PR. Phase 1 first (safest, biggest win), then 2-4 in any order.

## Additional build optimizations (independent of test reorganization)

These can be pursued in parallel:

1. **Install mold linker** -- `sudo apt install mold` + `.cargo/config.toml` -- 40-60% faster linking for all targets
2. **Reduce debug info** -- `[profile.dev] debug = "line-tables-only"` + `[profile.dev.package."*"] debug = false` in workspace Cargo.toml -- 20-30% smaller targets
3. **cargo-nextest** -- ✅ DONE. Configured in `.config/nextest.toml`. Benchmarked at ~44% faster wall time (19m43s vs 35m20s) on 701 tests / 7 cores. BDD tests (`harness = false`) are excluded via `default-filter = "not binary(bdd)"` and must be run separately with `cargo test --test bdd`. Flaky test `test_try_start_via_cargo_run_with_fail_config` removed (redundant with pre-built binary variant; flaked due to cargo lock contention on the `cargo run` fallback path).

Note: switching TLS crypto from aws-lc-rs to ring was investigated and is not viable for this project. aws-lc-sys (41.9s cmake build) remains a bottleneck but is mitigated by caching on incremental builds.
