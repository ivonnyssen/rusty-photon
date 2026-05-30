# Bazel Migration Plan

**Status:** In progress
**Started:** 2026-04-16
**Target cutover:** TBD (dependent on shadow-mode validation)

## Decisions (2026-05-24)

Three decisions taken to shorten the path to a Bazel-primary cutover (Phase 7):

1. **Cache backend: a Cloudflare Worker + R2 edge cache** (public-read /
   token-write), replacing the BuildBuddy free tier. Served from the edge so
   cloud CI isn't bottlenecked, ~$0 (R2 has no egress fees), retention
   controlled via an R2 lifecycle rule. Code + deploy:
   [tools/bazel-cache-worker/](../../tools/bazel-cache-worker/README.md);
   overview: [docs/skills/bazel-remote-cache.md](../skills/bazel-remote-cache.md).
2. **Leptos / `sentinel-app` WASM: abandoned.** Not used today; Phase 4 is
   dropped, not deferred.
3. **Release packaging: stays on Cargo permanently.** We release far less often
   than we merge, so `release.yml` keeps using `cargo-deb` /
   `cargo-generate-rpm`; Phase 6 is dropped. "Bazel-primary" applies to the
   per-PR build/test path only.

## Motivation

Three concrete problems drive this migration:

1. **CI rebuilds without explanation.** `cargo-rail`'s `FILE_KIND_TOML_WORKSPACE` classifier forces `infra=true` on any root `Cargo.toml` edit, triggering full-workspace rebuilds even when only `[workspace.dependencies]` changed. Swatinem/rust-cache also evicts unpredictably under GHA's 10 GB per-repo cap, causing cold rebuilds that appear random.
2. **Slow critical path.** `aws-lc-sys` cmake build (41.9 s) blocks rustls → reqwest → ascom-alpaca → every workspace crate. 55 test binaries account for 52 % of compile CPU time. Windows BDD spawn overhead is 5 s per cucumber scenario × 145 scenarios.
3. **TypeScript is coming** for the UI. Cargo + npm in CI means two dependency graphs, two cache stories, no shared action graph. Bazel unifies this under one remote cache and one action-graph-level change detection.

Bazel's remote cache is the structural fix for items 1 and 2 — the cache is content-addressed and unbounded (vs GHA's 10 GB ceiling that killed sccache). Action-graph change detection replaces cargo-rail's heuristics with ground truth. Cross-language support handles item 3 when `rules_js`/`rules_ts` arrive.

## Scope

**In-scope:**
- All 11 workspace crates build and test under Bazel.
- Remote cache wired up. Bootstrapped on the BuildBuddy free tier; now a Cloudflare Worker + R2 cache (see Decisions, 2026-05-24).
- New GHA workflow running Bazel in shadow mode; Cargo remains required until parity.
- BDD cucumber tests run under Bazel via custom `rust_test` wrappers.
- Documentation (`CLAUDE.md`, `docs/skills/pre-push.md`, `docs/skills/bazel-remote-cache.md`) updated.

**Out-of-scope (deferred or dropped):**
- Removing Cargo entirely. `Cargo.toml` remains the source of truth for dependency versions via `crate_universe`'s `from_cargo`. Developers can still run `cargo` locally for IDE/rust-analyzer support.
- **Leptos / `sentinel-app` WASM (dropped 2026-05-24).** Not used today; Phase 4 cancelled.
- **Release packaging (dropped 2026-05-24).** `release.yml` stays on `cargo-deb` / `cargo-generate-rpm`; we release far less often than we merge. Phase 6 cancelled.
- Miri under Bazel. rules_rust miri support is thin; keep the scheduled Cargo job.
- ConformU: the external tool install and the canonical nightly conformance gate stay on Cargo (`conformu.yml`). But the per-service `conformu_integration` tests are now **also runnable under Bazel** (audit follow-up) — they drive the ConformU CLI via `bdd_infra::run_conformu` (no longer the `ascom-alpaca/test` feature), tagged `conformu` and gated on `CONFORMU_PATH`: `CONFORMU_PATH=… bazel test --config=conformu //...`.
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

**Dependency resolution.** `MODULE.bazel` uses `crate.from_cargo(manifests = ["//:Cargo.toml"])` — only the workspace root manifest is listed; `crate_universe` follows the `members` field in `[workspace]` to discover the rest. Cargo.toml and Cargo.lock stay as single source of truth. Adding a dep is still `cargo add`, followed by `CARGO_BAZEL_REPIN=1 bazel mod tidy` to refresh the Bazel crate index. Adding a new workspace member does **not** require editing `MODULE.bazel`.

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
- [x] `bdd-infra` discovers binaries via the conventional `{PACKAGE_UPPER}_BINARY` env var or `target/debug/<pkg>`; callers pass only the package name. No `[package.metadata.bdd]` or `cargo run` fallback — missing binaries panic with a clear diagnostic.
- [x] Cross-platform: Bazel CI now runs on `ubuntu-latest`, `macos-latest`, and `windows-latest`. The `lld` linker flag is scoped to Linux via `.bazelrc` `build:linux`.

**Exit criteria met:** `bazel test --test_tag_filters=bdd //...` passes on Linux (5 targets, ~150 s wall on a warm cache — dominated by rp:bdd at ~150 s with 84 scenarios; the other four targets overlap in parallel and add negligible wall time).

### Phase 4 — `sentinel-app` WASM (DROPPED 2026-05-24)

Cancelled: Leptos is not used today, so `sentinel-app` stays out of the Bazel
graph. If a WASM UI returns, re-open this phase — the `wasm_bindgen` /
`@platforms//cpu:wasm32` / hydrate+ssr approach noted in earlier revisions is
the starting point.

### Phase 5 — Remote cache + CI (DONE; cache backend swapped 2026-05-24)
- [x] `.github/workflows/bazel.yml` — triggers on PR, push to main, and a nightly schedule (07:07 UTC), runs `bazel test //...` with remote cache.
- [x] Bootstrap backend: BuildBuddy free tier (read+write token on push/schedule, read-only token on PRs).
- [x] Shadow mode: job is **not required** for merges; runs alongside Cargo jobs for 2+ weeks of parity validation.
- [x] **Cache backend swapped to a Cloudflare Worker + R2 edge cache** (public-read / token-write). `.bazelrc` `--config=remote-cache` points at the Cloudflare hostname; `bazel.yml` attaches `Authorization: Bearer` only on push/schedule and sets `--remote_upload_local_results=false` on PRs. Reads are anonymous, so fork PRs get a warm cache too. Served from the edge (no origin uplink in the path), retention via an R2 lifecycle rule — replacing the BuildBuddy LRU cold-cache outliers. Code + deploy: [tools/bazel-cache-worker/](../../tools/bazel-cache-worker/README.md).
- [x] `.bazelrc` hostname set to `cache.rustyphoton.space` (zone `rustyphoton.space` verified on Cloudflare 2026-05-24).
- [ ] Add the `BAZEL_CACHE_WRITE_TOKEN` GHA secret and deploy the Worker + R2 ([tools/bazel-cache-worker/](../../tools/bazel-cache-worker/README.md)).
- [ ] Compare wall-clock and correctness against Cargo jobs weekly (≥1 week on the new cache).

**Exit criteria:** Bazel CI job green for 2 consecutive weeks with no flakes; wall-clock within ±20 % of Cargo or better.

### Phase 6 — Packaging (DROPPED 2026-05-24)

Cancelled: `release.yml` stays on `cargo-deb` / `cargo-generate-rpm`. Release
cadence is far lower than merge cadence, so the Bazel-primary goal targets the
per-PR build/test path only; packaging keeps running on Cargo indefinitely.

### Phase 7 — Cutover (later)

With Phase 4 (Leptos) and Phase 6 (packaging) dropped, cutover no longer waits
on them. Remaining prerequisites: the cache live + parity logged
(Phase 5), the Cargo-only gates (miri, sanitizers, `cargo-hack`,
`cargo-msrv`, coverage) kept on a Cargo nightly, and the `rust-project.json`
IDE decision (open question 4).

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
| Remote cache unavailable / cold | Low | Resolved: a Cloudflare Worker + R2 edge cache with retention via an R2 lifecycle rule. Bazel treats remote-cache errors as non-fatal (warns, builds locally), so a cache outage degrades to a cold build rather than a CI failure. |
| `aws-lc-sys` build fails on Windows under Bazel (MAX_PATH + bswap) | Hit | Four fixes: (1) shortened `from_cargo` name from `"crates"` to `"cr"` — the repo name appears twice in every build-script runfiles path, saving 8 chars; (2) `build_script_data_glob = ["**"]` annotation ensures all vendored C files are materialised in Bazel's runfiles; (3) `AWS_LC_SYS_NO_JITTER_ENTROPY=1` disables jitterentropy on Windows — its `tree_drbg_jitter_entropy.c` uses a deep `../../../../` relative `#include` whose un-normalised intermediate form (~280 chars) exceeds MSVC's MAX_PATH; (4) `AWS_LC_SYS_CFLAGS=/we4013` promotes MSVC's implicit-function-declaration warning to an error — the cc-crate builder's feature check for `__builtin_bswap*` wrongly passes because cl.exe in C89 mode treats GCC built-ins as implicit declarations (C4013, level 3) without emitting the warning at the default `/W1` level; `/we4013` makes the check fail correctly so aws-lc uses MSVC's `_byteswap_*` intrinsics. We use `AWS_LC_SYS_CFLAGS` (not plain `CFLAGS`) because rules_rust overrides `CFLAGS` in the build-script environment with its own MSVC flags; the crate-specific variant is read first by `get_crate_cflags()` and propagated to `CFLAGS_<target>` before the feature checks run. |

## Success metrics

Measured weekly in shadow mode, then post-cutover:

- **PR CI wall-clock p50 and p95.** Target: ≥30 % reduction vs current Cargo+cargo-rail baseline (~7–8 min p50).
- **Cache hit ratio.** Target: ≥80 % on PR builds touching fewer than 5 crates.
- **Flaky re-run rate.** Target: <2 % of jobs require a retry (vs current unexplained rebuilds).
- **Developer time to first build on a fresh clone.** Target: ≤3 min (remote cache hit) vs current ~8 min (cold `cargo build`).

## Known test gaps under Bazel

Captured after Phase 1 pilot. Only `bdd-infra`'s own cargo-integration tests
remain Bazel-skipped; the phd2-guider gap originally listed here is now closed.

- `//crates/bdd-infra:bdd-infra_unit_test` — 3 of 17 tests: `test_run_once_*` variants that shell out to `cargo build` to locate the `rp` binary when `RP_BINARY` isn't already set. They intentionally exercise the cargo-fallback path, so they stay tagged `requires-cargo`.
- `//services/phd2-guider:phd2-guider_unit_test` — **resolved.** The 8 `test_start_phd2_*` tests use a `MockProcessSpawner` (they never exec a real binary) and only needed an existing file for `get_executable_path`'s `.exists()` check; `dummy_executable_path()` now returns `std::env::current_exe()`, so they pass in the sandbox and the target is no longer tagged `requires-cargo`. The `test_integration` and `test_mock_server` suites also gained Bazel targets — `test_integration` discovers the `mock_phd2` / `phd2-guider` binaries via `MOCK_PHD2_BINARY` / `PHD2_GUIDER_BINARY` and its config fixtures via `TEST_SRCDIR` / `TEST_WORKSPACE`.

**Resolution pattern for cargo-coupled tests:** refactor to accept the sibling binary via a `<UPPER_SNAKE>_BINARY` env var (wired `$(rootpath ...)` in BUILD, with `option_env!("CARGO_BIN_EXE_*")` as the Cargo fallback so the file still compiles under Bazel), and resolve fixture directories via `TEST_SRCDIR` / `TEST_WORKSPACE` (falling back to `CARGO_MANIFEST_DIR` under Cargo — see `services/ppba-driver/tests/translations.rs`). `bdd-infra`'s 3 stay `requires-cargo` because they test the cargo-fallback machinery itself.

Not a migration blocker.

### Mockall mock variants (cross-crate)

Some workspace crates expose mockall-generated types so downstream
test code in *other* crates can mock them. Cargo handles this with a
`mock` feature gated on `dep:mockall` plus a dev-dependency
re-declaration:

```toml
[features]
mock = ["dep:mockall"]

[dev-dependencies]
some-crate = { workspace = true, features = ["mock"] }
```

Cargo unifies the dep declarations so test compilations see the mock
symbol while production binaries link a feature-free version. Bazel
has no equivalent — `crate_features` is a compile-time attribute of
the library output. The pattern:

1. Production-clean `rust_library` named `<crate>` with **no**
   `crate_features`.
2. Test-only `rust_library` variant named `<crate>_with_mock` with
   `crate_features = ["mock"]` and `testonly = True`. Same
   `crate_name` as the production target — they are never linked
   into the same binary. `testonly = True` makes Bazel reject any
   production `rust_library` / `rust_binary` that tries to depend
   on the variant, so the convention is enforced at build time.
3. Downstream `rust_library` / `rust_binary` targets depend on the
   production variant. (Not both: two crates with the same
   `crate_name` in one closure produces an `E0464 multiple
   candidates` link conflict.)
4. Downstream consumers whose unit tests reuse the library's sources
   via `rust_test(crate = ":<lib>")` need a **twin** `rust_library`
   target (`<lib>_with_mock`, also `testonly = True`) that swaps in
   the `_with_mock` variant of the mock-providing dep. rules_rust
   merges the parent crate's deps with the rust_test's deps, so
   swapping deps only on the `rust_test` is not sufficient — the
   parent must already be on the test-only dep. The twin shares all
   attributes with the production library except for the swapped
   dep and the `testonly` flag.
5. The mock-providing crate's own `rust_test` points at
   `crate = ":<crate>_with_mock"` so `cfg(test)` and
   `feature = "mock"` agree.

Today only `crates/rp-plate-solver` exposes mocks across crate
boundaries (`MockPlateSolveClient` for `services/rp:rp_unit_test`).
`crates/rp-tls` uses `#[cfg_attr(test, mockall::automock)]` —
single-crate scope, no cross-crate consumer, no `_with_mock` variant
needed. New crates that expose mockall mocks for downstream tests
follow the variant pattern.

### BDD conventions under Bazel (post Phase 3)

Each service's `tests/bdd.rs` is now a Bazel `rust_test` with:

- `use_libtest_harness = False` (cucumber has its own main).
- `BDD_PACKAGE_DIR = "services/<name>"` set in `env`; `bdd_main!` chdirs there at startup and absolutizes any `*_BINARY` env vars first so binary discovery still resolves relative to the runfiles root.
- `<SERVICE>_BINARY = "$(rootpath :<binary>)"` for self-spawn. Cross-spawn services set additional binaries: sentinel sets `FILEMONITOR_BINARY`, rp sets `CALIBRATOR_FLATS_BINARY`, and calibrator-flats sets `RP_BINARY` (the inverse of `rp:bdd`).
- `data` includes the service binary, `Cargo.toml`, `tests/features/**`, and any fixture JSON (`tests/config.json` etc.).
- ppba-driver and qhy-focuser have a second mock-feature binary target (`_mock` suffix) because Bazel treats `crate_features` as compile-time; the BDD test points at the mock binary.

Env var names follow the `{UPPER_SNAKE_PACKAGE}_BINARY` convention (e.g. `RP_BINARY`, `PPBA_DRIVER_BINARY`, `QHY_FOCUSER_BINARY`). `bdd-infra` derives the name from the package string passed to `ServiceHandle::start` — there is no per-service override.

All twelve service BDD suites now have Bazel `bdd` targets. The last two —
`plate-solver` and `calibrator-flats` — were wired after the initial Phase-3
batch; each adds a wrinkle worth noting:

- **plate-solver** spawns its service binary, which in turn shells out to the
  `mock_astap` stub (`src/bin/mock_astap.rs`), so its `bdd` target deps both
  `:plate-solver` and `:mock_astap` and sets `PLATE_SOLVER_BINARY` +
  `MOCK_ASTAP_BINARY`. It needs no OmniSim, so it runs under a plain
  `bazel test //services/plate-solver:bdd`. The `@requires-astap` scenarios
  self-gate on the `ASTAP_BINARY` env var (unset in PR jobs); `@unix` scenarios
  self-gate on `cfg(unix)`. Adding the target also meant splitting `src/main.rs`
  out of `plate-solver_lib` into a `plate-solver` `rust_binary` — the
  hand-written BUILD previously had only the lib + `mock_astap` (Cargo
  auto-discovers `src/main.rs` as the default binary, so there was no Cargo-side
  gap to notice).
- **calibrator-flats** is the inverse of `rp:bdd`: it cross-spawns rp
  (`RP_BINARY`) and OmniSim through the `bdd-infra_rp_harness` variant, plus its
  own binary (`CALIBRATOR_FLATS_BINARY`). Like every rp_harness target it needs
  OmniSim (`OMNISIM_PATH`, forwarded by `build:ci --test_env`) at runtime, so a
  local run requires OmniSim installed.

#### OmniSim is single-instance per host — serialize the two consumers (added 2026-05-27)

Exactly two BDD targets spawn OmniSim: `//services/rp:bdd` and
`//services/calibrator-flats:bdd` (the only two that depend on
`bdd-infra_rp_harness`). The other ten BDD suites do not touch OmniSim.

OmniSim enforces a **single instance per host**: `ASCOM.Alpaca.Simulators`'
`Program.cs` guards startup with a machine-global named `Mutex`
(`new Mutex(false, "Global\\{ApplicationGUID}")`) keyed on a fixed GUID — *not*
the port — so a second instance cannot start even on a different port; it hands
its arguments to the running instance over a named pipe and exits. (This is why
`OmniSimHandle::get_or_spawn` reuses a healthy instance on the fixed port 32323
rather than starting its own.)

Under Cargo CI this is invisible because each BDD service runs as its own job on
its own machine. Under Bazel all targets run as parallel actions on **one**
machine, so `rp:bdd` and `calibrator-flats:bdd` would race for the single
OmniSim: whichever spawns it first owns it (reaped by the process-wrapper sandbox
+ `PR_SET_PDEATHSIG` when that test action ends), the other reuses it over HTTP,
then loses it mid-run when the owner finishes — surfacing as a "connection
refused" cascade through the loud-reset `before` hook (#172) for every remaining
scenario. This is what failed the shadow `bazel test (BDD)` step on all three
OS, and (as a hang under instrumentation) the `bazel coverage` job.

Fix: both targets carry `tags = ["resources:omnisim:1"]`, and `.bazelrc` defines
the matching pool `test --local_resources=omnisim=1`. Bazel then never schedules
the two concurrently, while leaving them parallel to every non-OmniSim test —
strictly better than the `exclusive` tag (which would also serialize them against
the other ten BDD suites). The pool must be defined or the tagged tests fail
loudly (`Resource omnisim is not being tracked by the resource manager`), so the
race can never silently return by forgetting a flag. `--local_resources` is a
scheduling-only flag and does not enter action keys, so it does not perturb the
remote cache. **Any new OmniSim-spawning BDD target MUST carry the same tag.**

## Coverage under Bazel (shadow, added 2026-05-26)

A separate shadow-mode workflow, `.github/workflows/bazel-coverage.yml`, runs
`bazel coverage` on every PR (and push to main) alongside the canonical Cargo
coverage job (`test.yml`). It is **not required** for merge.

**Why it's worth doing under Bazel.** Coverage actions are content-addressed and
cacheable like any other action. `bazel coverage //...` builds/tests the whole
repo, but targets untouched by a PR are cache hits (local disk + remote R2), so
their `coverage.dat` is fetched rather than recomputed. Every PR therefore gets
a *complete* full-repo report while paying only for the changed targets — no
cargo-rail narrowing, and no dependence on Codecov flag carryforward to fill in
the untouched packages (we upload every package's flag every run, mostly from
cache). The coverage cache is a **separate action namespace** from `bazel.yml`'s
stable build/test (different rustc flags + nightly toolchain), so the coverage
workflow primes it on push-to-main with the write token.

**Parity requirements (the non-obvious parts):**

- **Pinned nightly toolchain + `--cfg=coverage_nightly`.** The `coverage(off)`
  attributes on every `#[cfg(test)] mod tests` block are gated on the
  nightly-only `coverage_attribute` feature. cargo-llvm-cov activates them by
  running on nightly and auto-setting `--cfg=coverage_nightly`; rules_rust does
  neither. So `MODULE.bazel` registers a pinned nightly alongside stable, and
  `.bazelrc`'s `--config=coverage` selects `channel=nightly` and adds
  `--cfg=coverage_nightly`. Without this, either the code fails to compile (the
  feature gate) or test modules pollute the numbers. The date is pinned (not
  rolling) so a nightly bump doesn't bust every coverage action's cache key.
- **Per-package flags.** Bazel emits one combined lcov;
  `tools/coverage/split_lcov.py` splits it per package (by `crates/<pkg>/` |
  `services/<pkg>/` source path — directory basename is the package name
  throughout this workspace) so the per-service Codecov flags survive.
- **Distinct `bazel-<pkg>` flags (shadow mode).** Uploads go to `bazel-<pkg>`,
  not `<pkg>`, so the canonical per-service badges stay Cargo-only until
  cutover. Codecov's project-wide total becomes the union of Cargo+Bazel during
  shadow mode (union can only raise coverage, never trip the 1 % drop gate);
  scope the `project` status to the Cargo flags in `.github/codecov.yml` if you
  want the total kept strictly Cargo-only.

**BDD is included; child-process coverage is the open risk.** Coverage runs the
BDD suite too — `--config=coverage` sets `--test_tag_filters=-requires-cargo`,
so only requires-cargo (cargo-shelling) tests are dropped — and the CI job
installs OmniSim for rp's scenarios. Under `bazel coverage //...` the spawned
service binaries ARE instrumented (first-party top-level targets matching the
instrumentation filter), so the ingredients for service-binary coverage are
present. What is NOT guaranteed is **collection**: cargo-llvm-cov captures
child-process coverage via a shared `LLVM_PROFILE_FILE` (with `%p`/`%m` patterns)
and a whole-target-dir `llvm-profdata merge`; Bazel's per-test-action model
reliably collects only the test process's own `.profraw`. Whether each
BDD-spawned child's `.profraw` lands in `COVERAGE_DIR` and gets merged depends on
the `LLVM_PROFILE_FILE` pattern rules_rust sets and whether its merge step globs
the whole directory.

**Validate in run #1** by diffing each `bazel-<service>` flag against `<service>`
(Cargo). If a service's Bazel coverage is materially lower, child-process
profraw is being dropped. The first runs confirmed exactly that — e.g.
`bazel-filemonitor` ≈ 61 % vs Cargo `filemonitor` ≈ 97 %, `bazel-rp` ≈ 73 % vs
`rp` ≈ 95 % — so the **contingency is now implemented**: `bdd-infra`'s spawn path
([`child_coverage_profile_var`] in `crates/bdd-infra/src/lib.rs`, applied in
`spawn_process` and `run_once`) sets
`LLVM_PROFILE_FILE=$COVERAGE_DIR/<pkg>-%p-%m.profraw` on each child `Command`,
but only when `COVERAGE_DIR` is set. That gate makes it Bazel-coverage-only:
plain `bazel test` and `cargo`/`cargo-llvm-cov` (which sets `LLVM_PROFILE_FILE`,
not `COVERAGE_DIR`) are untouched. `bdd_main!` absolutizes `COVERAGE_DIR` before
its chdir, alongside the `*_BINARY` vars, so the child's path resolves correctly.

**This is necessary but may not be sufficient — know what run #1 can and cannot
show.** Reading `rules_rust` 0.70.0's `util/collect_coverage/collect_coverage.rs`:
it globs *every* `*.profraw` in `COVERAGE_DIR` into the `llvm-profdata merge`
(so each child's counters ARE folded into the `.profdata`), but the subsequent
`llvm-cov export` is passed exactly one object — the test binary — and no
`-object` for the spawned service binaries. `llvm-cov` only emits coverage for
functions whose coverage-mapping lives in the objects on its command line.
Consequence: a spawned child's counters land in the profdata, but are exported
only for code whose mapping is *also* in the `rust_test` binary — i.e. library
crates the BDD target links (`rp:bdd` deps `:rp_lib`), not binary-only code
(`main.rs`, startup/shutdown). So this change can lift the library-level numbers
but will not, on its own, capture binary-only paths. **Run-#1 rubric:** re-diff
`bazel-<service>` vs `<service>`. If the gap closes, done. If `bazel-<service>`
stays near its pre-change value despite child `*.profraw` appearing in
`COVERAGE_DIR`, the cause is the single-`-object` export above (not a wiring
bug) — the fix is export-side: add the spawned binaries as `-object`s (a custom
post-merge `llvm-cov export`, or a `rules_rust` change), or fall back to
collecting those suites' coverage Cargo-style. Do **not** revert this wiring;
it is the prerequisite that puts the child profraw where any export-side fix
needs it.

**Run #1 result (commit `9e6deb9`, PR #340, coverage run 26675105103).** The run
completed cleanly — `//services/rp:bdd` finished in 368 s (66/66 pass, no hang;
the new `--test_timeout=900` and exit-3 tolerance stayed dormant) and split 27
package reports. The parity diff confirmed the prediction above: the
`bazel-<svc>` numbers were **unchanged** from before the child-coverage wiring.

| service | Bazel | Cargo | gap |
|---|---|---|---|
| rp | 72.64 | 94.77 | −22 |
| filemonitor | 61.18 | 95.78 | −35 |
| ppba-driver | 78.29 | 94.69 | −16 |
| sentinel | 72.02 | 93.86 | −22 |
| star-adventurer-gti | 33.23 | 95.29 | −62 |

`bazel-rp` (72.64) and `bazel-filemonitor` (61.18) are bit-identical to their
pre-wiring values, so the child `.profraw` is being collected but not exported —
exactly the single-`-object` limitation. `star-adventurer-gti` (−62) is the
starkest, its coverage being almost entirely BDD-child-driven. The wiring is
correct and regression-free; closing the gap is now an **export-side** task.

**Export-side root cause + proven fix (spike, locally validated).** The gap is
*not* a collection failure — under `VERBOSE_COVERAGE` the sandboxed
`collect_coverage` provably feeds all ~36 spawned-`filemonitor` child `.profraw`
into `llvm-profdata merge`, so the child counters ARE in the `.profdata`. The
loss is that `rules_rust` 0.70.0's `util/collect_coverage/collect_coverage.rs`
runs `llvm-cov export … -object <TEST_BINARY>` with **exactly one, hardcoded
object** and no env/attr/provider to add more (confirmed; upstream HEAD is
identical and the two relevant PRs — #3490 `COVERAGE_BINARY`, #3808 — only swap
the single object and are unmerged). The test binary has no covmap for
`filemonitor/src`, so the merged child counters have nothing to bind to.

Proven locally on `filemonitor:bdd` (instrumented `bdd` + `filemonitor`
binaries, one merged profdata):

| `llvm-cov export` objects | `services/filemonitor/src` |
|---|---|
| `-object <test>` (rules_rust today) | **0 / 0 — none** |
| `-object <test> -object <filemonitor>` | **155 / 166 = 93.4%** (`lib.rs` 92.8%, `main.rs` 100%) |

vs Cargo ~95.8%. The multi-object lcov is a strict superset (0 SF lost); the
child's counters bind because `filemonitor/src` is compiled into both binaries
with matching covmap hashes.

**Fix (implemented).** A minimal single-file patch of `collect_coverage.rs`,
pinned via `single_version_override(module_name="rules_rust", patch_strip=1,
patches=["//third_party/patches:collect_coverage_extra_objects.patch"])` in
`MODULE.bazel`, appends `-object <p>` to the `llvm-cov export` for each
`:`-separated entry of a new `RUST_COVERAGE_EXTRA_OBJECTS` env var (resolved with
the collector's own runfiles/`config_bin_dir` logic). Each `rust_test` that
spawns a binary sets that var to the `$(rootpath …)` of the binaries it spawns —
the same values already in its `*_BINARY` vars (mock variants included, e.g.
`star-adventurer-gti_mock`). The var is read only by `collect_coverage` under
`bazel coverage`, so it is inert for cargo/`cargo-llvm-cov` and plain
`bazel test`; `split_lcov.py` and the upload are unchanged.

Validated locally on three services (per-service `src`):

| service `src` | before | after |
|---|---|---|
| filemonitor | 0% | 92.8% |
| sentinel | (missing) | 79.6% |
| star-adventurer-gti | 33% | 78.2% |

Cost / guards: the vendored ~15-line patch must be re-rebased on rules_rust
upgrades (upstream has no hook — PRs #3490/#3808 unmerged, HEAD identical). A
non-applying patch already fails every `bazel` command at fetch time; the
`parity` CI job additionally asserts the patch is *effective* (the
`RUST_COVERAGE_EXTRA_OBJECTS` marker survives in the extracted collector), so a
silently-dropped patch is caught. **Known gap to confirm in CI:** a cross-package
spawn (e.g. `sentinel` → `filemonitor`) did not surface the cross-spawned
binary's `src` locally. This is NOT a resolver bug — the cross-spawned binary is
in the test's `data`, so `find_extra_object` resolves its runfiles path and the
`-object` is placed; the residual is a downstream covmap/profraw-binding detail,
not the BUILD wiring. It is supplementary anyway — each service's own `bdd`
covers its own `src`, so the per-service flags are unaffected. The residual
`bazel < cargo` deltas (e.g. star-adventurer 78% vs 95%) are the next item to
characterize against the full shadow run.

Badge reliability and run reliability (PR #340) are delivered; with this
export-side change the per-service Bazel coverage now approaches Cargo, the last
parity prerequisite before cutover.

**Badge reliability (carryforward).** The `bazel-<pkg>` badges read "unknown"
not from a wiring fault but because a Codecov *branch* badge resolves to the
branch HEAD, and the shadow job uploads nothing whenever its run is cancelled.
`.github/codecov.yml` now sets `flag_management.default_rules.carryforward:
true`, so a commit that doesn't re-upload a flag inherits its last-known value
instead of rendering "unknown". This makes carryforward explicit and
version-controlled for every flag (the Cargo badges already behaved this way —
the Cargo job narrows uploads to changed packages via cargo-rail yet all its
badges stay populated — but in-repo there was no declaration of it). It
decouples badge health from per-commit upload success.

Two caveats. (1) **Not retroactive.** Codecov applies carryforward while
*processing an upload*, so the currently-"unknown" badges do not populate the
instant this config lands — they fill in on the next `main` commit whose
`bazel-coverage` run uploads at least one report (HEAD's parent already carries
real bazel data to inherit from). For an immediate fix, re-run the cancelled
HEAD job or push a no-op `main` commit. (2) **Gating status.** The
`coverage.status.project` gate has no `flags:` filter, so it runs on the merged
report across all flags; with carryforward on, the partial `bazel-<pkg>` data is
now carried into that merge. This cannot regress the 1 % drop gate — Codecov
merges flags by union (a line is covered if *any* flag covers it) and `bazel-*`
covers a strict subset of the first-party lines Cargo already covers, so the
merged % is non-decreasing. If you want the gate kept strictly Cargo-only during
shadow mode, scope `status.project.default.flags` to the Cargo flags.

**The instrumented `rp:bdd` hang.** ~half of push-to-main coverage runs were
cancelled at the 60-min job wall: `//services/rp:bdd` intermittently hangs under
instrumentation while spawning OmniSim, and its `size = "large"` (900s) timeout
was observed *not* to fire under `bazel coverage`, so nothing killed it and the
run uploaded zero coverage. Two guards now bound the blast radius:
`.bazelrc` `coverage:coverage --test_timeout=900` caps any single coverage test
explicitly, and `bazel-coverage.yml` tolerates a "tests failed/timed out" exit
(Bazel exit 3) so the combined report — built by `--keep_going` from every
target that *did* pass — still splits and uploads. The underlying hang (an
unbounded Alpaca poll under instrumentation, per the OmniSim flake family in the
service docs) is the remaining reliability item; if the explicit `--test_timeout`
also proves not to fire under coverage, the fallback is to exclude
`//services/rp:bdd` + `//services/calibrator-flats:bdd` (the two OmniSim
spawners) from the coverage graph and collect their BDD coverage separately.

This supersedes the Phase 7 note that coverage stays a Cargo-only gate: coverage
now has a Bazel shadow path, to be validated in CI before any cutover.

## Open questions

1. **bzlmod vs WORKSPACE.** Starting with bzlmod. Fallback to WORKSPACE mode if `crate_universe` bzlmod issues block progress.
2. **Remote cache vendor.** RESOLVED (2026-05-24): a Cloudflare Worker + R2 edge cache, public-read / token-write. See Decisions and [tools/bazel-cache-worker/](../../tools/bazel-cache-worker/README.md).
3. **TypeScript addition.** Deferred until UI work actually starts. `rules_js` and `aspect_rules_ts` are bzlmod-first — will integrate cleanly then.
4. **rust-analyzer.** Does the team use cargo directly for IDE, or do we need `rust-project.json` generation from rules_rust? Decide after Phase 2.
