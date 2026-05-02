# Plan: `rp-plate-solver` service

**Date:** 2026-05-02
**Branch:** `worktree-astap`
**ADR:** [005-plate-solver](../decisions/005-plate-solver.md)
**Parent plan:** [image-evaluation-tools.md Phase 6c-1](image-evaluation-tools.md#phase-6c-1--rp-plate-solver-rp-managed-service)

## Background

ADR-005 adopted ASTAP, executed as a subprocess by a new
`rp-plate-solver` rp-managed service, with the binary and index database
supplied by the operator (BYO). The ADR explicitly defers
implementation sequencing to a separate plan — this is that plan.

`rp-plate-solver` is the second rp-managed service in the workspace
(after `phd2-guider`) and the first one stood up entirely under the
design-first / test-first workflow established in
`docs/skills/development-workflow.md`. It exists so the upcoming
`plate_solve` built-in MCP tool in rp (Phase 6c-2) and the
`center_on_target` compound tool (Phase 6c-3) have a stable HTTP target
to call against.

## Goals

1. **MVP:** stand up `rp-plate-solver` as a Sentinel-supervised wrapper
   around ASTAP, exposing one HTTP solve endpoint plus a health probe,
   with all three failure domains (rp, wrapper, ASTAP child)
   independently bounded.
2. **Freeze the HTTP contract here** so Phase 6c-2 can build the rp-side
   client against a stub server while this service is still in flight.
3. **Retire ADR-005 open questions 1–6** (per-platform end-to-end solve
   passes, LGPL §4/§6 review under BYO, hint plumbing).
4. **Establish the rp-managed-service template** for any future
   external-process wrapper — module shape, trait-isolated subprocess
   surface, supervision contract, BDD test-double pattern.

## Decisions resolved (during design)

These belong in `docs/services/rp-plate-solver.md` once Phase 1 lands
and should not be re-litigated.

### Stability and supervision — the load-bearing rationale

`rp-plate-solver` is an **rp-managed service**, not a plugin, because
Tenet 1 ("robustness above all else") demands that ASTAP — a
single-maintainer LGPL Pascal binary that can SIGSEGV on pathological
star fields, wedge on a too-small `-r` search radius with bad hints, or
deadlock on flaky storage — cannot threaten `rp`'s liveness. The
discriminator from `rp.md` §"Component Categories" is the supervision
default:

| Category | Sentinel restart on hang/crash |
|----------|-------------------------------|
| rp-managed services | **default** |
| Plugins | opt-in ("may restart configurable plugins") |

For ASTAP's failure profile, default supervision is the right posture.
Plugin shape would force operators to opt into Sentinel coverage and
write the restart command themselves — a footgun for a component that
*will* hang in the field.

The architecture defines **three nested failure domains**, each
independently bounded by an explicit supervisor:

| Domain | What dies | Supervisor | Mechanism |
|--------|-----------|-----------|-----------|
| `rp` (gateway) | session-state owner | Sentinel | event-stream disconnect → operator-configured restart command |
| `rp-plate-solver` (wrapper) | one solver process; no session state | Sentinel | operator-configured restart command (e.g. `systemctl restart rp-plate-solver`) |
| `astap_cli` (child) | one solve attempt | the wrapper itself | per-request wall-clock deadline → SIGTERM → SIGKILL after 2 s grace |

Belt and suspenders: **rp's HTTP client to the wrapper has its own
outer timeout** (`plate_solver.timeout_secs` in rp config). Even if the
wrapper's internal timeout regresses, rp does not hang on a
`plate_solve` call.

### Stateless across requests

The wrapper holds no per-solve state. No solve cache, no warm process
pool, no shared mutable structures across requests. Restart is always
cheap and never costs more than the in-flight request. This is what
makes Sentinel's restart strategy safe: "kill it" is the recovery, not
"kill it and restore session state."

### Single-flight solve, queued at the wrapper

ASTAP is CPU-bound on a single core during a solve. Concurrency at the
wrapper would just thrash the core and double per-solve wall time. The
wrapper accepts overlapping HTTP requests but processes them serially
behind a `tokio::sync::Semaphore::new(1)`. Configurable via
`max_concurrency`, default 1. v1 ships with the default.

### Trait-isolated subprocess surface; `mock_astap` for BDD; real ASTAP only in Phase 6 e2e

Three layers of isolation, each with a distinct purpose.

**Layer 1 — `AstapRunner` trait + `mockall` for upstream-of-runner unit
tests.** The `Command::new("astap_cli")` boundary is wrapped in an
`AstapRunner` trait gated by `#[cfg_attr(test, mockall::automock)]`, per
ADR-004's mock-strategy rule for narrow service-boundary traits (≤ 10
methods). The trait lets the HTTP handler, single-flight semaphore, and
error-mapping logic be unit-tested without spawning anything.

**Layer 2 — `mock_astap` `[[bin]]` for BDD.** Following the `mock_phd2`
precedent in `services/phd2-guider/src/bin/mock_phd2.rs`, the wrapper
ships a small in-tree binary that mimics the ASTAP CLI surface
(reads `-f <path>`, writes a sidecar `.wcs`, exits) with **named
behavior modes** selected via `MOCK_ASTAP_MODE`:

| Mode | Behavior | Drives BDD scenario |
|------|----------|---------------------|
| `normal` (default) | Read `-f` arg, write a canned `.wcs` next to it, exit 0 | Happy path |
| `exit_failure` | Print to stderr, exit 1 (no `.wcs` written) | `solve_failed` |
| `hang` | Sleep indefinitely; respond cleanly to SIGTERM | `solve_timeout` (terminated) |
| `ignore_sigterm` | Install a SIGTERM-ignore handler, then sleep | `solve_timeout` (killed) — exercises the SIGKILL escalation through the full HTTP-to-process pipeline |
| `malformed_wcs` | Write a `.wcs` missing `CRVAL2`, exit 0 | Wrapper's `.wcs` parser must reject cleanly |
| `no_wcs` | Exit 0 without writing any `.wcs` | Wrapper must not return success when the sidecar is missing |

`MOCK_ASTAP_ARGV_OUT=<path>` (any mode) writes the received argv to the
named file, so steps that need to assert end-to-end argv flow can do so
without touching the trait. The argv-mapping *contract* still gets its
focused coverage at unit-test level on `runner/astap.rs` (build
`Command`, inspect `args()` slice, no spawn) — `MOCK_ASTAP_ARGV_OUT` is
for scenarios that want to assert the full request → subprocess pipeline
preserves what the runner builds.

`mock_astap` is not feature-gated — same as `mock_phd2`, it's a regular
`[[bin]]` that always builds. BDD discovers it via
`env!("CARGO_BIN_EXE_mock_astap")` (cleaner than the path-walking
`mock_phd2` does, because BDD lives in the same crate).

This pattern unifies what would otherwise be three separate test
artifacts (shell scripts for happy-path / failure / hang, plus a
separate `sigterm_trap` `[[bin]]`) into one self-documenting Rust
binary whose modes are the test contract.

**Layer 3 — Real ASTAP via `install-astap`, only in Phase 6.** The
cross-platform end-to-end workflow installs real ASTAP on each target
platform and runs a single solve through the full wrapper to validate
that the CLI surface, hint-flag mapping, and `.wcs` parser haven't
silently diverged from upstream. ASTAP upstream regressions
themselves are caught by the existing `install-astap` smoke workflow
(per ADR-005's `cache-key-suffix: github.run_id` posture); Phase 6
catches *integration* regressions specifically.

**Local developer experience:** running `cargo test -p rp-plate-solver
--test bdd` requires nothing beyond `cargo build -p rp-plate-solver
--all-targets` (which builds `mock_astap` automatically). No ASTAP
install needed for routine work. Same DX bar as phd2-guider with
`mock_phd2`.

### BYO ASTAP per ADR-005, validated at startup

The wrapper's config requires `astap_binary_path` and
`astap_db_directory`. Both are validated at startup:

- `astap_binary_path` must exist, be a regular file, and be executable
  by the current user.
- `astap_db_directory` must exist and be a directory.

Validation failures produce a structured error that names the field
and links to the README's per-platform install instructions, then exit
non-zero (so Sentinel's restart loop surfaces the misconfiguration
rather than masking it).

### Solver-swap path kept architecturally available, not built in v1

The `AstapRunner` trait is the swap-out point: a future `SolveFieldRunner`
implementation lets operators flip to astrometry.net's `solve-field`
via configuration, no rebuild required. v1 ships only the ASTAP
implementation. The trait shape is informed by both solvers' shared
contract (FITS-in / WCS-out subprocess) so the swap is mechanical.

### Real-ASTAP coverage: cadence and gating

Real ASTAP runs **nightly, on all three target OSes** as a small
tagged smoke that backstops the mock-based BDD. PR jobs install
nothing and run only the `mock_astap`-backed scenarios; nightly
installs ASTAP via `install-astap` on each runner and lets the
`@requires-astap` scenarios fire.

| Concern | Where it's caught |
|---------|-------------------|
| Wrapper contract correctness (every error code, every supervision arm, every `.wcs` edge case) | mock-backed BDD on every affected PR |
| Mock vs. real divergence — does real ASTAP still behave the way `mock_astap` claims? | Phase 6 nightly cross-platform smoke (~2 scenarios per OS) |
| Per-platform integration — does the wrapper actually work on macOS / Windows? | Same Phase 6 nightly job — runs on `ubuntu-latest`, `macos-latest`, `windows-latest` |
| ASTAP upstream regressions independent of our code | The existing `install-astap` smoke workflow (already in repo) |

A single nightly cross-platform smoke is enough: the scenarios are
narrow (happy path + one error), the install-astap action handles
per-OS bytes, and nightly cadence is fast enough to catch divergence
before it pollutes a release. No separate weekly tier.

**Gating mechanism — `@requires-astap` cucumber tag.** The pattern
mirrors `@wip` (already in `bdd.rs` per `docs/skills/testing.md` §2.7).
The runner's `filter_run` skips any scenario tagged `@requires-astap`
unless the `ASTAP_BINARY` env var is set:

```rust
.filter_run("tests/features", |feat, _rule, sc| {
    let is_wip = feat.tags.iter().any(|t| t == "@wip")
        || sc.tags.iter().any(|t| t == "@wip");
    let needs_astap = feat.tags.iter().any(|t| t == "@requires-astap")
        || sc.tags.iter().any(|t| t == "@requires-astap");
    let astap_available = std::env::var("ASTAP_BINARY").is_ok();
    !is_wip && (!needs_astap || astap_available)
})
```

PR `required` jobs never set `ASTAP_BINARY` → tagged scenarios silently
skip with no error noise. The nightly job sets it via
`install-astap` → tagged scenarios fire. The same pattern generalizes
to any future service with a real-binary backstop.

### HTTP, not MCP

The wrapper exposes a narrow REST surface over HTTP, not an MCP
server. Justification:

- Exactly one operation. Wrapping rmcp's session/discovery machinery
  around a single endpoint is overkill.
- The caller (`rp`) is the only consumer ever planned; rp owns the
  `plate_solve` MCP tool name, not the wrapper.
- Health probing is trivial in REST and more involved in MCP.

### No `wcs` section persistence in this service

The wrapper returns the WCS solution in the HTTP response and stops
there. Writing a `wcs` section onto the exposure document is rp's job
(in Phase 6c-2's `plate_solve` MCP tool), because rp owns the
exposure-document store.

## HTTP contract (frozen)

This contract is fixed in this plan so Phase 6c-2 can mock it
immediately. Changes after the freeze require a follow-up PR with
explicit consumer notice.

### `POST /api/v1/solve`

Request body (JSON):

```json
{
  "fits_path": "/data/lights/M31/M31_L_300s_001.fits",
  "ra_hint": 10.6847,
  "dec_hint": 41.2689,
  "fov_hint_deg": 1.5,
  "search_radius_deg": 5.0,
  "timeout": "30s"
}
```

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `fits_path` | absolute path string | yes | Must be readable by the wrapper process. Path-based input matches ADR-005's "rp and the plate solver share a filesystem" contract — no pixel bytes over HTTP. |
| `ra_hint` | f64 (decimal hours, 0–24) | no | Maps to ASTAP `-ra`. |
| `dec_hint` | f64 (decimal degrees, −90–90) | no | Maps to ASTAP `-spd` after south-pole-distance conversion. |
| `fov_hint_deg` | f64 (degrees, image height) | no | Maps to ASTAP `-fov`. |
| `search_radius_deg` | f64 (degrees) | no | Maps to ASTAP `-r`. Defaults to ASTAP's own default when omitted. |
| `timeout` | humantime string | no | Wrapper's wall-clock deadline for the ASTAP child. Defaults to `30s`. Bounded by wrapper-config max. |

Success response (`200 OK`, JSON):

```json
{
  "ra_center": 10.6848,
  "dec_center": 41.2690,
  "pixel_scale_arcsec": 1.05,
  "rotation_deg": 12.3,
  "solver": "astap-cli-2026.04.28"
}
```

| Field | Notes |
|-------|-------|
| `ra_center` | Decimal degrees (0–360). Sourced from `.wcs` `CRVAL1`. |
| `dec_center` | Decimal degrees (−90–90). Sourced from `.wcs` `CRVAL2`. |
| `pixel_scale_arcsec` | Computed from `CDELT1` (absolute value, arcsec/pixel). |
| `rotation_deg` | Sourced from `.wcs` `CROTA2`. |
| `solver` | Banner string parsed at wrapper startup, cached. |

Error response (non-2xx, JSON):

```json
{
  "error": "solve_failed",
  "message": "ASTAP exited 1: insufficient stars detected",
  "details": { "exit_code": 1, "stderr_tail": "..." }
}
```

Error codes (frozen):

| Code | HTTP status | When |
|------|-------------|------|
| `invalid_request` | 400 | Schema-invalid body, unparseable `timeout`, `fits_path` not absolute. |
| `fits_not_found` | 404 | `fits_path` does not exist or is not readable. |
| `solve_failed` | 422 | ASTAP returned non-zero or did not produce a `.wcs`. |
| `solve_timeout` | 504 | Wrapper's wall-clock deadline expired before ASTAP returned. |
| `internal` | 500 | Unexpected wrapper failure (broken pipe, parser bug, etc.). |

### `GET /health`

Returns `200 OK` with `{"status": "ok"}` when the wrapper has passed
startup config validation and the ASTAP binary is still executable on
disk. `503` otherwise. Sentinel's "Responsive but stuck" check at
`rp.md` line 2143 uses this endpoint.

## MVP scope

### In scope for v1

- Single endpoint `POST /api/v1/solve` and `GET /health`.
- ASTAP runner only.
- Single-flight solves (configurable max concurrency, default 1).
- Per-request timeout with SIGTERM → SIGKILL escalation.
- Startup config validation.
- Sentinel-friendly: prints `bound_addr=` to stdout (per
  `bdd-infra` convention), responds to SIGTERM cleanly, exits non-zero
  on misconfiguration.

### Out of scope for v1 (deferred or never)

- `solve-field` (astrometry.net) runner — architecturally swappable, not
  built.
- Background solving by subscribing to `exposure_complete` events. The
  ADR mentions this as a possibility; v1 is request/response only.
- A solve cache or warm process pool — explicitly excluded by the
  stateless-across-requests decision.
- An MCP server surface — REST only.
- Pixel transport over HTTP — file-path only, per ADR-005.

## Module structure

```
services/rp-plate-solver/
  Cargo.toml
  BUILD.bazel
  README.md                     # Per-platform install pointers, ops guide
  src/
    main.rs                     # CLI entry (clap), prints bound_addr=
    lib.rs                      # ServerBuilder + BoundServer, public API
    config.rs                   # Config + load_config + startup validation
    error.rs                    # AppError (thiserror), HTTP error mapping
    api.rs                      # axum routes + request/response types
    runner/
      mod.rs                    # AstapRunner trait + #[automock]
      astap.rs                  # Real impl: builds Command, parses .wcs
      wcs.rs                    # .wcs file parser (CRVAL1/2, CDELT1/2, CROTA2)
    supervision.rs              # Per-request timeout + signal escalation
  tests/
    bdd.rs                      # Entry point (uses bdd_main!)
    bdd/
      world.rs                  # PlateSolverWorld + service handle + script paths
      steps/
        mod.rs
        config_steps.rs
        solve_steps.rs
        supervision_steps.rs
        health_steps.rs
    features/
      configuration.feature
      solve_request.feature
      subprocess_supervision.feature
      health.feature
    fixtures/
      m31_known.fits            # Small (~2 MB) known-solution FITS — used
                                # by the @requires-astap nightly smoke
                                # (Phase 6) and by Phase 7's cross-platform
                                # e2e workflow. mock_astap-backed BDD
                                # scenarios don't read pixel content.
      degenerate_no_stars.fits  # Small all-zeros FITS for the real
                                # solve_failed scenario in the
                                # @requires-astap smoke
      sample.wcs                # Canned .wcs that mock_astap's "normal"
                                # mode copies into place
  src/bin/
    mock_astap.rs               # [[bin]] test double mimicking ASTAP's CLI
                                # surface with named MOCK_ASTAP_MODE modes
                                # (normal | exit_failure | hang |
                                # ignore_sigterm | malformed_wcs | no_wcs).
                                # Discovered by BDD via
                                # env!("CARGO_BIN_EXE_mock_astap"). Not
                                # feature-gated — always built. Pattern
                                # mirrors services/phd2-guider/src/bin/mock_phd2.rs.
```

The closest workspace reference is `services/phd2-guider/`. Note the
shape borrows: `config.rs`, `error.rs`, `lib.rs` (`ServerBuilder`), and
the `bdd_main!` BDD entry. It diverges where the domain differs:
`runner/` replaces `process.rs`/`connection.rs`/`rpc.rs` because the
external-program contract is fundamentally different (one short-lived
subprocess per request vs. one long-lived TCP peer).

## Phases

Each phase is its own PR.

### Phase 1 — Service design doc

Status: **not started.**

- [ ] `docs/services/rp-plate-solver.md` (new). Sections: Overview,
      Architecture, Behavioral contracts (per HTTP endpoint, per error
      code, per supervision domain), Configuration, Subprocess test
      doubles, MVP scope, Out of scope.
- [ ] Update `docs/services/rp.md` §"Plate Solver" to point at the new
      service doc and to drop the "decision pending" note (now retired
      by ADR-005).
- [ ] Update `docs/workspace.md` if the services index lives there.

**Exit criteria:** new doc reviewed and merged. No code yet.

### Phase 2 — Crate scaffolding + `AstapRunner` trait + `.wcs` parser

Status: **not started.**

- [ ] New workspace member `services/rp-plate-solver` registered in root
      `Cargo.toml`. Standard crate metadata (workspace inheritance for
      version, edition, rust-version, lints).
- [ ] `BUILD.bazel` for the new crate; `CARGO_BAZEL_REPIN=1 bazel mod
      tidy` if any new crates.io deps land here.
- [ ] `config.rs` — `Config` with `astap_binary_path`,
      `astap_db_directory`, `bind_address`, `port`, `max_concurrency`,
      `default_solve_timeout`, `max_solve_timeout`. `load_config(path)`
      + startup validation. Unit-tested.
- [ ] `error.rs` — `AppError` enum + `ErrorResponse` shape + axum
      `IntoResponse` mapping to the frozen error code table.
- [ ] `runner/mod.rs` — `AstapRunner` trait with one method:
      `async fn solve(&self, request: SolveRequest) -> Result<SolveOutcome, RunnerError>`.
      `#[cfg_attr(test, mockall::automock)]`. The trait abstracts the
      subprocess; `SolveRequest` carries the parsed config + hints,
      `SolveOutcome` carries the parsed `.wcs` values + solver banner.
- [ ] `runner/astap.rs` — real implementation. Builds the `Command`
      argument vector (mapping HTTP fields to ASTAP flags per the
      contract table), spawns under the supervision module, reads the
      sidecar `.wcs`, returns `SolveOutcome`. **Argv-mapping unit
      tests** here: given a `SolveRequest` with each combination of
      hint flags set/unset, assert the resulting `Command` argv slice
      matches expected. No spawning. This is the only unit-level
      coverage of the hint-flag pass-through contract; BDD asserts
      end-to-end behavior, not argv shape.
- [ ] `runner/wcs.rs` — `.wcs` file parser. Pure function over a
      string slice. Returns the four fields + the solver banner from
      the file's `COMMENT` line, or a parse error naming the missing
      key. Unit-tested exhaustively (every required key absent, every
      key present, malformed numeric, unexpected key order).
- [ ] `supervision.rs` — `spawn_with_deadline()` helper. Spawns a
      `tokio::process::Command`, races `child.wait()` against
      `tokio::time::sleep(deadline)`. On deadline expiry, sends
      SIGTERM, waits 2 s, sends SIGKILL. Returns a typed outcome
      (`Exited(status)` / `TimedOutTerminated` / `TimedOutKilled`).
      Unit tests use `mock_astap` (built in this same phase, see
      below):
      - **Exited** — `MOCK_ASTAP_MODE=normal` with a generous deadline.
      - **TimedOutTerminated** — `MOCK_ASTAP_MODE=hang` with a 100 ms
        deadline; mock_astap responds to SIGTERM cleanly.
      - **TimedOutKilled** — `MOCK_ASTAP_MODE=ignore_sigterm` with a
        100 ms deadline; mock_astap traps SIGTERM, must be SIGKILLed
        after the 2 s grace.

      Resolved via `env!("CARGO_BIN_EXE_mock_astap")`. Same binary
      drives both supervision unit tests and BDD scenarios — one
      artifact, one set of named modes, one place to extend when new
      failure modes need coverage.
- [ ] `src/bin/mock_astap.rs` — the `[[bin]]` itself. Implements the
      six `MOCK_ASTAP_MODE` modes from the design decision above. On
      Windows, `ignore_sigterm` uses `SetConsoleCtrlHandler` to
      ignore `CTRL_BREAK_EVENT` (the closest equivalent the
      supervision module's Windows signal path will exercise);
      document the platform-specific signal semantics in the binary's
      source. Argv-out side-channel implemented via
      `MOCK_ASTAP_ARGV_OUT`. Registered as a `[[bin]]` in
      `Cargo.toml`; not feature-gated.

**Exit criteria:** `cargo build -p rp-plate-solver --all-features
--all-targets` clean. Unit tests for `config.rs`, `error.rs`,
`runner/wcs.rs`, `supervision.rs` pass. The crate produces a `lib.rs`
re-export surface but no `main.rs` yet (Phase 4 wires the binary).

### Phase 3 — BDD scenarios (with `@wip`)

Status: **not started.**

All four feature files run against **`mock_astap`**. Each scenario
configures `MOCK_ASTAP_MODE` for the wrapper's `astap_binary_path`
config so the wrapper spawns a mock that produces the failure mode the
scenario targets. Real ASTAP is not in the loop here; that's Phase 6.

- [ ] `tests/features/configuration.feature` — startup validation:
      missing binary path → exit non-zero with field-naming error;
      binary path not executable → ditto; database directory missing
      → ditto; happy-path validation accepts `mock_astap` as a valid
      binary path. Target ~6 scenarios.
- [ ] `tests/features/solve_request.feature` — happy path
      (`MOCK_ASTAP_MODE=normal` returns the four expected fields from
      the canned `.wcs`); each error code from the frozen table
      (`solve_failed` via `exit_failure`, `solve_timeout` via `hang`,
      `fits_not_found` and `invalid_request` via the wrapper's
      pre-spawn rejection); the two `.wcs`-edge-case modes
      (`malformed_wcs` → `solve_failed` with parser-detail message;
      `no_wcs` → `solve_failed` with sidecar-missing message). End-to-end
      argv-flow assertion via `MOCK_ASTAP_ARGV_OUT` for one scenario
      per hint flag. Target ~10 scenarios.
- [ ] `tests/features/subprocess_supervision.feature` — `solve_timeout`
      (terminated) via `MOCK_ASTAP_MODE=hang` with `timeout: 100ms`;
      `solve_timeout` (killed) via `MOCK_ASTAP_MODE=ignore_sigterm`
      with the same deadline (the SIGKILL-escalation path now lives
      in BDD, not punted to a unit test); single-flight serialization
      (two concurrent `hang`-mode requests; assert second observed
      start time is after first observed end time, via mock_astap
      timestamp side-channel writes). Target ~5 scenarios.
- [ ] `tests/features/health.feature` — `/health` returns `200` after
      startup validation; returns `503` if the configured binary path
      is removed between startup and the probe (use a temp-dir copy
      of `mock_astap`, then delete the copy). Target ~3 scenarios.
- [ ] `tests/features/real_astap_smoke.feature` — tagged
      `@requires-astap` at the feature level, so PR jobs skip it
      automatically and the nightly job (Phase 6) runs it. Target ~2
      scenarios:
      1. **Happy path** — real solve through the wrapper using
         `tests/fixtures/m31_known.fits`; assert RA/Dec within
         tolerance of the known solution. This is the "is the mock
         honest" assertion: if real ASTAP starts producing a `.wcs`
         shape `mock_astap` doesn't, this scenario catches it before
         the next Phase 7 weekly cron run.
      2. **Real `solve_failed`** — real ASTAP on a degenerate FITS
         (committed alongside `m31_known.fits`); assert
         `solve_failed` with non-empty stderr embedded.
- [ ] `bdd.rs` — `filter_run` extends the existing `@wip` filter to
      also filter `@requires-astap` based on `ASTAP_BINARY`
      presence (snippet in the design decision section above).
- [ ] `tests/fixtures/degenerate_no_stars.fits` — small all-zeros
      FITS for the real `solve_failed` scenario. Re-uses the existing
      `m31_known.fits` for the happy path so we don't ship two
      multi-MB fixtures.
- [ ] All four feature files tagged `@wip`. `bdd.rs` uses
      `filter_run` to skip `@wip` so the workspace BDD suite stays
      green until Phase 4 lands. Convention per
      `docs/skills/testing.md` §2.7.
- [ ] `tests/bdd/world.rs` — `PlateSolverWorld` with
      `service_handle: Option<ServiceHandle>`, `last_response:
      Option<reqwest::Response>`, `temp_dir: Option<TempDir>`. Resolves
      the `mock_astap` path from `env!("CARGO_BIN_EXE_mock_astap")` at
      compile time — no env vars to set, no path walking.
- [ ] `tests/bdd/steps/*.rs` — step definitions. Reuse the
      `bdd-infra` `ServiceHandle` for the wrapper spawn/stop. No new
      feature flag on `bdd-infra` is needed — the wrapper does not
      host an MCP server, so `rp-harness` is irrelevant.
- [ ] **No CI install changes for BDD**: the BDD suite is fully
      self-contained because `mock_astap` is built by `cargo build
      --all-targets -p rp-plate-solver`. cargo-rail's existing
      change-detection logic handles the rest. The `install-astap`
      action stays scoped to its existing smoke workflow and to
      Phase 6's cross-platform e2e workflow.

**Exit criteria:** `cargo test -p rp-plate-solver --all-features --test
bdd` compiles and runs cleanly with `@wip` filtering all scenarios out
(0 scenarios run, 0 failures). The compiled BDD harness is the gate
that the next phase's implementation has to satisfy.

### Phase 4 — HTTP server + supervision impl + remove `@wip`

Status: **not started.**

- [ ] `api.rs` — axum router with `POST /api/v1/solve` and
      `GET /health`. Request body deserialization via `serde_json`,
      response serialization to the frozen schema, error mapping per
      the frozen table. Single-flight semaphore wired here.
- [ ] `lib.rs` — `ServerBuilder`/`BoundServer` two-phase API
      mirroring `phd2-guider`'s shape (avoids port-TOCTOU race called
      out in `docs/skills/development-workflow.md` §"Phase 4
      Stabilization").
- [ ] `main.rs` — CLI (clap), reads `--config`, prints
      `bound_addr=<host>:<port>` to stdout (per `bdd-infra` convention
      so `parse_bound_port` works), installs SIGTERM handler for
      graceful shutdown.
- [ ] Remove `@wip` from all four feature files in the same commit
      that lands the implementation.

**Exit criteria:** all BDD scenarios pass. `cargo rail run --profile
commit -q` clean. `cargo fmt` clean.

### Phase 5 — Sentinel integration

Status: **not started.**

- [ ] Document the per-service Sentinel restart command in
      `services/rp-plate-solver/README.md`. Linux/systemd:
      `systemctl restart rp-plate-solver`. Mention macOS launchd /
      Windows service-manager equivalents.
- [ ] Confirm Sentinel's existing per-service restart-command config
      surface (per `rp.md` §"Sentinel Watchdog Integration") accepts a
      service entry for `rp-plate-solver` without code changes. If it
      does, this phase is docs-only; if it does not, the gap is
      called out in a follow-up issue.
- [ ] Sentinel's "Responsive but stuck" check uses `GET /health` on
      this service. The check happens via Sentinel's existing health
      probe machinery; no per-service code in `rp-plate-solver`.

**Exit criteria:** an operator can configure Sentinel to restart
`rp-plate-solver` on hang/crash by following the README. No regression
in Sentinel's existing tests.

### Phase 6 — Nightly cross-platform real-ASTAP smoke (retires ADR-005 OQ 1–4)

Status: **not started.**

The `@requires-astap`-tagged smoke scenarios from Phase 3 are wired
to run nightly on all three target OSes. This is the only place real
ASTAP runs against the real wrapper; the existing `install-astap`
smoke workflow catches ASTAP-upstream regressions independently of
our code.

The scenarios already exist after Phase 3 — this phase is CI plumbing
plus the macOS / Windows-ARM64 specifics ADR-005 calls out.

- [ ] `.github/workflows/rp-plate-solver-smoke.yml` — `schedule:
      cron` nightly trigger plus `workflow_dispatch` for manual runs.
      Matrix: `ubuntu-latest`, `macos-latest`, `windows-latest`.
      Each leg installs ASTAP + D05 via
      `.github/actions/install-astap` with
      `download-database: true`, exports `ASTAP_BINARY` and
      `ASTAP_DB_DIR`, then runs
      `cargo test -p rp-plate-solver --all-features --test bdd`.
      The `@requires-astap` filter in `bdd.rs` lets the tagged
      scenarios fire because `ASTAP_BINARY` is set; the rest of the
      BDD suite runs against `mock_astap` as usual.
- [ ] Capture median solve time per platform in the workflow log;
      assert the upper bound stays inside the "few seconds with
      hint" budget the ADR commits to. Failing this assertion is a
      regression signal, not a flake — investigate before landing.
- [ ] macOS leg: the workflow does the
      `xattr -d com.apple.quarantine` step explicitly so we know
      whether it suffices vs. requiring Developer ID re-signing.
- [ ] Windows ARM64: no GitHub-hosted runner. Close the ADR's open
      question by either a one-off manual-machine pass or by
      removing the `Windows-ARM64` row from `install-astap`'s per-OS
      table — pick one in this phase.
- [ ] On failure, the job opens (or updates) a tracking issue named
      `nightly: rp-plate-solver real-ASTAP smoke failing on <os>` so
      divergence is visible without depending on someone reading
      scheduled-workflow logs. Same pattern as `scheduled.yml`'s
      `update` job behavior on dependency-update breakage.
- [ ] Update ADR-005's open-questions section to mark items 1–4
      retired, citing this workflow.

**Exit criteria:** workflow green on all three runners on its first
nightly fire after merge. ADR-005 OQ 1–4 marked retired in the same
PR.

### Phase 7 — Hint-plumbing verification (retires ADR-005 OQ 6)

Status: **not started.**

- [ ] Confirm via `services/rp/src/equipment/mount.rs` (Phase 6c-prep
      lands this) that the mount wrapper exposes RA/Dec with enough
      accuracy to seed `-ra` / `-spd` and that mount pointing
      uncertainty is bounded enough to seed `-r`. If pointing
      uncertainty is not currently observable from the Alpaca
      surface, document the gap and the operator-supplied default
      that fills it.
- [ ] Add a hinted-vs-blind perf comparison to Phase 6's nightly
      smoke workflow: run the happy-path scenario twice (hints
      supplied, hints omitted) and capture the solve-time delta in
      the workflow log. Observation, not contract enforcement —
      surfaces drift in the speed advantage the ADR promises without
      failing the build.
- [ ] Update ADR-005 open-question 6 to retired with a pointer to
      the workflow run that demonstrated it.

**Exit criteria:** ADR-005 OQ 6 marked retired.

### Phase 8 — LGPL §4/§6 review under BYO (retires ADR-005 OQ 5)

Status: **not started.**

- [ ] Add a short legal-review note under
      `docs/decisions/005-plate-solver.md` §"License Treatment"
      converting the working assumption into a closed item: confirm
      that subprocess execution of an operator-installed binary
      engages neither §4 (Conveying Verbatim Copies) nor §6 (Combined
      Works); confirm that the `install-astap` GH action's per-run
      fresh download does not constitute conveyance; confirm the
      GH-Actions cache layer is scoped narrowly enough to count as
      ephemeral build infrastructure.
- [ ] If any of those three sub-items returns the opposite finding,
      open a follow-up issue capturing the remediation (e.g., move the
      install-astap action's cache scope to per-PR, or drop caching
      entirely).

**Exit criteria:** ADR-005 OQ 5 marked retired or a remediation
issue is open.

## Sequencing notes

**All eight phases land before `rp-plate-solver` is considered
complete.** Phases 5–8 are not "harden later" work — they retire the
six open questions ADR-005 explicitly defers to this plan, and Phase 5
makes the supervision tenet (the load-bearing rationale for choosing
rp-managed-service over plugin) actually hold in production. None of
them is optional.

Within that fixed total scope, the phases have two distinct
dependency structures:

**Strict order, blocks downstream rp work:**

- Phases 1 → 2 → 3 → 4 land sequentially. This is the critical path
  for unblocking 6c-2 and 6c-3. Each phase is its own PR.

**Independent of each other, all required, can interleave with rp work:**

- Phases 5, 6, 7, 8 land in any order after Phase 4. None of them
  blocks the others; none of them blocks 6c-2 or 6c-3 from starting.
  They do block declaring `rp-plate-solver` complete.

**Cross-plan unblocking** (consumers of this plan's output):

- **6c-2 (rp-side `plate_solve` MCP tool) can begin as soon as Phase
  1 of this plan lands** — the HTTP contract is frozen there, and
  6c-2's BDD uses an in-test axum stub returning canned WCS payloads
  (per PR #124). 6c-2 does not block on this plan's Phases 2–8.
- **6c-3 (`center_on_target` compound tool) unblocks once 6c-2's
  `PlateSolveOps` adapter exists**, not on this plan directly.
- **6c-prep (telescope primitives) is independent** of this entire
  chain and can land in parallel with anything here.

## References

- ADR-005 — `docs/decisions/005-plate-solver.md`
- Image-evaluation-tools plan, Phase 6c-1 — `docs/plans/image-evaluation-tools.md`
- rp design doc, §"Component Categories" / §"Plate Solver" /
  §"Sentinel Watchdog Integration" — `docs/services/rp.md`
- ADR-004 — `docs/decisions/004-testing-strategy-for-http-client-error-paths.md`
  (mock-strategy rule for the `AstapRunner` trait)
- Reference service shape — `services/phd2-guider/`
- Mock-binary precedent — `services/phd2-guider/src/bin/mock_phd2.rs`
  (named `MOCK_PHD2_MODE` modes, `[[bin]]` always built, used for
  integration tests without depending on real PHD2)
- BDD harness conventions — `docs/skills/testing.md`,
  `crates/bdd-infra`
- Install action — `.github/actions/install-astap/action.yml`
