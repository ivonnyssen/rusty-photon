# Bazel Migration Plan

**Status:** In progress
**Started:** 2026-04-16
**Target cutover:** TBD (dependent on shadow-mode validation)

## Motivation

Three concrete problems drive this migration:

1. **CI rebuilds without explanation.** `cargo-rail`'s `FILE_KIND_TOML_WORKSPACE` classifier forces `infra=true` on any root `Cargo.toml` edit, triggering full-workspace rebuilds even when only `[workspace.dependencies]` changed. Swatinem/rust-cache also evicts unpredictably under GHA's 10 GB per-repo cap, causing cold rebuilds that appear random.
2. **Slow critical path.** `aws-lc-sys` cmake build (41.9 s) blocks rustls → reqwest → ascom-alpaca → every workspace crate. 55 test binaries account for 52 % of compile CPU time. Windows BDD spawn overhead is 5 s per cucumber scenario × 145 scenarios.
3. **TypeScript is coming** for the UI. Cargo + npm in CI means two dependency graphs, two cache stories, no shared action graph. Bazel unifies this under one remote cache and one action-graph-level change detection.

Bazel's remote cache is the structural fix for items 1 and 2 — the cache is content-addressed and unbounded (vs GHA's 10 GB ceiling that killed sccache). Action-graph change detection replaces cargo-rail's heuristics with ground truth. Cross-language support handles item 3 when `rules_js`/`rules_ts` arrive.

## Scope

**In-scope:**
- All 11 workspace crates build and test under Bazel.
- Remote cache wired up (BuildBuddy free tier to start; self-host later if needed).
- New GHA workflow running Bazel in shadow mode; Cargo remains required until parity.
- BDD cucumber tests run under Bazel via custom `rust_test` wrappers.
- `sentinel-app` Leptos WASM compiles under Bazel.
- Release packaging (`cargo-deb`, `cargo-generate-rpm`) migrated to `rules_pkg`.
- Documentation (`CLAUDE.md`, `docs/skills/pre-push.md`) updated.

**Out-of-scope (deferred):**
- Removing Cargo entirely. `Cargo.toml` remains the source of truth for dependency versions via `crate_universe`'s `from_cargo`. Developers can still run `cargo` locally for IDE/rust-analyzer support.
- Miri under Bazel. rules_rust miri support is thin; keep the scheduled Cargo job.
- ConformU runs. External ASCOM tool; keep the existing Cargo invocation.
- Migrating `cargo-husky` pre-commit hooks. Bazel-native alternative would be a `sh_binary` hook installer, but out of scope here.

## Target architecture

```
repo root/
├── MODULE.bazel          ← bzlmod, pulls rules_rust + deps from crates.io via Cargo.lock
├── .bazelrc              ← shared build flags, remote cache config
├── .bazelversion         ← pinned Bazel version (bazelisk reads this)
├── .bazelignore          ← excludes target/, external/, .claude/, etc.
├── Cargo.toml            ← canonical dep versions; crate_universe reads this
├── Cargo.lock            ← canonical lockfile; crate_universe reads this
├── BUILD.bazel           ← top-level aliases (optional)
├── crates/<name>/
│   ├── Cargo.toml        ← unchanged
│   └── BUILD.bazel       ← rust_library / rust_binary / rust_test targets
└── services/<name>/
    ├── Cargo.toml        ← unchanged
    └── BUILD.bazel
```

**Dependency resolution.** `MODULE.bazel` uses `crate.from_cargo(manifests = ["//:Cargo.toml", ...])`. Cargo.toml and Cargo.lock stay as single source of truth. Adding a dep is still `cargo add`, followed by `CARGO_BAZEL_REPIN=1 bazel mod tidy` to refresh the Bazel crate index.

**Known limitation.** rules_rust issue #1574: `workspace.dependencies` inheritance has edge cases in `crate_universe`. Mitigation: if repin fails on a specific crate, declare that crate directly in MODULE.bazel with `crate.spec(...)`. Track cases in this file as they arise.

## Phases

### Phase 0 — Foundation (DONE)
- [x] Migration plan doc (this file).
- [x] `.bazelversion`, `.bazelrc`, `MODULE.bazel`, `.bazelignore`.
- [x] `crate_universe` wired to root `Cargo.toml` + `Cargo.lock`.
- [x] `bazel mod tidy` succeeds; external crate index generated.
- [x] bazelisk installed locally and pinned via `.bazelversion`.

**Exit criteria:** `bazel build @cr//...` resolves all crates.io deps without error.

### Phase 1 — Leaf crates (DONE)
- [x] `crates/rp-auth/BUILD.bazel` — `rust_library` + `rust_test`.
- [x] `crates/rp-tls/BUILD.bazel` — `rust_library` + `rust_test`.
- [x] `crates/bdd-infra/BUILD.bazel` — `rust_library` + `rust_test`. Note: `TEST_SERVICE_BINARY` env var for integration tests needs a `sh_test` wrapper.
- [x] `services/phd2-guider/BUILD.bazel` — `rust_library` + `rust_binary` (mock_phd2) + `rust_test`.

**Exit criteria:** `bazel build //crates/... //services/phd2-guider/...` and `bazel test //crates/... //services/phd2-guider/... --test_tag_filters=-bdd` pass.

### Phase 2 — Service binaries (DONE)
- [x] `services/calibrator-flats/BUILD.bazel` — simplest service, rmcp client only.
- [x] `services/sentinel/BUILD.bazel` — adds tower/tower-http deps.
- [x] `services/filemonitor/BUILD.bazel` — Windows-conditional `windows-service`, conformu feature.
- [x] `services/qhy-focuser/BUILD.bazel` — mock + conformu features.
- [x] `services/ppba-driver/BUILD.bazel` — mock + conformu features.
- [x] `services/rp/BUILD.bazel` — largest binary; rmcp server + many ascom-alpaca features.

**Exit criteria:** all service binaries build; non-BDD unit tests pass.

### Phase 3 — BDD cucumber tests (DONE)
- [x] `rust_test` with `use_libtest_harness = False` + `BDD_PACKAGE_DIR` env var and a new chdir helper in `bdd_main!` so relative paths (`tests/features`, `./Cargo.toml`) behave the same under Bazel as under `cargo test`.
- [x] All five services wired up: filemonitor, sentinel (cross-spawns filemonitor), rp (cross-spawns calibrator-flats), ppba-driver (mock feature binary), qhy-focuser (mock feature binary). Tagged `bdd`.
- [x] `bdd-infra` gained `resolve_bdd_config`: if the conventional `{PACKAGE_UPPER}_BINARY` env var is set, skip reading Cargo.toml. Lets hermetic builds provide the pre-built binary path directly.
- [x] `load_config` falls back to `./Cargo.toml` when the compile-time `CARGO_MANIFEST_DIR` path is no longer valid at runtime (Bazel sandbox tear-down).
- [x] Cross-platform: Bazel CI now runs on `ubuntu-latest`, `macos-latest`, and `windows-latest`. The `lld` linker flag is scoped to Linux via `.bazelrc` `build:linux`.

**Exit criteria met:** `bazel test --test_tag_filters=bdd //...` passes on Linux (5 targets, 150 s total wall on a warm cache — rp:bdd alone is 150 s with 84 scenarios).

### Phase 4 — `sentinel-app` WASM (later)
- [ ] `rust_shared_library` with `crate_type = ["cdylib", "rlib"]`.
- [ ] `wasm_bindgen` integration via rules_rust's `@rules_rust//wasm_bindgen`.
- [ ] `select()` on `@platforms//cpu:wasm32` for hydrate feature.
- [ ] ssr feature as a default build target.

**Exit criteria:** `bazel build //services/sentinel-app:sentinel_app_wasm` produces the same `.wasm` + JS bindings that `cargo leptos build` does today.

### Phase 5 — Remote cache + CI (DONE)
- [x] `.github/workflows/bazel.yml` — triggers on PR + push to main, runs `bazel test //...` with remote cache.
- [x] BuildBuddy free tier credentials in GHA secrets (`BUILDBUDDY_API_KEY` read+write for push to main, `BUILDBUDDY_API_KEY_READONLY` for PRs to prevent cache poisoning).
- [x] Shadow mode: job is **not required** for merges; runs alongside Cargo jobs for 2+ weeks of parity validation.
- [ ] Compare wall-clock and correctness against Cargo jobs weekly.

**Exit criteria:** Bazel CI job green for 2 consecutive weeks with no flakes; wall-clock within ±20 % of Cargo or better.

### Phase 6 — Packaging (later)
- [ ] `rules_pkg` for `.deb` (filemonitor today; eventually all services).
- [ ] `rules_pkg` for `.rpm`.
- [ ] Windows service wrapping via `pkg_zip` or equivalent.
- [ ] `.github/workflows/release.yml` switched to Bazel.

**Exit criteria:** release artifacts byte-identical (or functionally equivalent) to the Cargo path.

### Phase 7 — Cutover (later)
- [ ] Bazel job becomes **required** on PRs.
- [ ] Cargo CI jobs moved to a scheduled nightly (as safety net).
- [ ] `docs/skills/pre-push.md` rewritten for `bazel test //...` as the primary pre-push command.
- [ ] `cargo-rail` dependency removed from CI (the 50-LOC upstream PR becomes moot).
- [ ] `.config/rail.toml` deleted.

**Exit criteria:** 30 days of required-Bazel CI with zero reverts to Cargo jobs.

## Rollback plan

At every phase until Phase 7, Cargo is unchanged and remains the required CI path. Rollback at any point is: delete `MODULE.bazel`, `.bazelrc`, `.bazelversion`, `BUILD.bazel` files, and the Bazel GHA workflow. Nothing in Cargo depends on Bazel.

After Phase 7: the Cargo nightly job remains as a safety net for 30 days. Rollback means re-enabling the Cargo required jobs from git history.

## Risks and mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| `crate_universe` can't resolve `workspace.dependencies` inheritance | Medium | Fall back to direct `crate.spec(...)` for specific crates. Document in this file. |
| `ascom-alpaca` git dep on fork breaks repin | Medium | Use `crate.annotation(...)` with `git_repository` override. |
| BDD custom harness (`harness = false`) incompatible with `rust_test` | Medium | Wrap as `sh_test` calling the cucumber binary directly with env vars. |
| Leptos hydrate/ssr WASM rules are missing | High | Defer to Phase 4; prototype separately before committing. If blocked, keep `cargo leptos` as an escape hatch via `genrule`. |
| rust-analyzer breaks under Bazel | Medium | Developers can still use Cargo locally (it's not removed). `rust-project.json` generator from rules_rust is also available. |
| Team learning curve | Certain | This plan doc + pair programming on first few BUILD files. |
| BuildBuddy free tier exceeded | Low | Self-host `bazel-remote` on a $5 VPS if we outgrow 100 GB/month transfer. |
| `aws-lc-sys` build fails on Windows under Bazel (MAX_PATH + bswap) | Hit | Four fixes: (1) shortened `from_cargo` name from `"crates"` to `"cr"` — the repo name appears twice in every build-script runfiles path, saving 8 chars; (2) `build_script_data_glob = ["**"]` annotation ensures all vendored C files are materialised in Bazel's runfiles; (3) `AWS_LC_SYS_NO_JITTER_ENTROPY=1` disables jitterentropy on Windows — its `tree_drbg_jitter_entropy.c` uses a deep `../../../../` relative `#include` whose un-normalised intermediate form (~280 chars) exceeds MSVC's MAX_PATH; (4) `AWS_LC_SYS_CMAKE_BUILDER=1` forces CMake on Windows — the cc-crate builder's feature check for `__builtin_bswap*` wrongly passes on MSVC (implicit declaration in C89 mode), causing LNK2019 unresolved externals; CMake's `check_compiler` does try-link and detects the failure correctly. |

## Success metrics

Measured weekly in shadow mode, then post-cutover:

- **PR CI wall-clock p50 and p95.** Target: ≥30 % reduction vs current Cargo+cargo-rail baseline (~7–8 min p50).
- **Cache hit ratio.** Target: ≥80 % on PR builds touching fewer than 5 crates.
- **Flaky re-run rate.** Target: <2 % of jobs require a retry (vs current unexplained rebuilds).
- **Developer time to first build on a fresh clone.** Target: ≤3 min (remote cache hit) vs current ~8 min (cold `cargo build`).

## Known test gaps under Bazel

Captured after Phase 1 pilot; these tests pass under Cargo but fail under Bazel's sandbox because they shell out to `cargo` or read the workspace `Cargo.toml` at runtime:

- `//crates/bdd-infra:bdd-infra_unit_test` — 4 of 18 tests: `test_run_once_*` variants that exercise `bdd-infra`'s internal cargo-build machinery.
- `//crates/bdd-infra:service_handle` — 1 of 9 tests: `test_start_via_cargo_run` explicitly tests the cargo-run fallback path.
- `//services/phd2-guider:phd2-guider_unit_test` — 8 of 213 tests: `test_start_phd2_*` variants that spawn a phd2 child process via cargo-discovered paths.

**Resolution plan (Phase 3 or later):** either mark these tests as `#[cfg(not(bazel))]` and set `rustc_flags = ["--cfg=bazel"]` on the Bazel `rust_test` targets, or refactor them to accept an explicit binary path via env var (which the non-cargo-code-paths already support). For now, tag them `requires-cargo` in BUILD files and run Bazel tests with `--test_tag_filters=-requires-cargo`.

Not a migration blocker — 217 of 230 tests across these three targets pass; the failures are confined to code that tests cargo-integration machinery which is inherently Cargo-specific.

### BDD conventions under Bazel (post Phase 3)

Each service's `tests/bdd.rs` is now a Bazel `rust_test` with:

- `use_libtest_harness = False` (cucumber has its own main).
- `BDD_PACKAGE_DIR = "services/<name>"` set in `env`; `bdd_main!` chdirs there at startup and absolutizes any `*_BINARY` env vars first so binary discovery still resolves relative to the runfiles root.
- `<SERVICE>_BINARY = "$(rootpath :<binary>)"` for self-spawn. Cross-spawn services set additional binaries: sentinel sets `FILEMONITOR_BINARY`, rp sets `CALIBRATOR_FLATS_BINARY`.
- `data` includes the service binary, `Cargo.toml`, `tests/features/**`, and any fixture JSON (`tests/config.json` etc.).
- ppba-driver and qhy-focuser have a second mock-feature binary target (`_mock` suffix) because Bazel treats `crate_features` as compile-time; the BDD test points at the mock binary.

Env var names follow the `{UPPER_SNAKE_PACKAGE}_BINARY` convention except ppba-driver which is `PPBA_BINARY` (matching the existing `[package.metadata.bdd]` contract).

## Open questions

1. **bzlmod vs WORKSPACE.** Starting with bzlmod. Fallback to WORKSPACE mode if `crate_universe` bzlmod issues block progress.
2. **Remote cache vendor.** Starting with BuildBuddy free tier. Consider self-hosted `bazel-remote` on a dedicated VM once cache size exceeds 10 GB.
3. **TypeScript addition.** Deferred until UI work actually starts. `rules_js` and `aspect_rules_ts` are bzlmod-first — will integrate cleanly then.
4. **rust-analyzer.** Does the team use cargo directly for IDE, or do we need `rust-project.json` generation from rules_rust? Decide after Phase 2.
