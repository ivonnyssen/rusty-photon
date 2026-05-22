# Plan: unified service lifecycle (issue #294)

**Status: DESIGN.** Crate scaffolded at
`crates/rusty-photon-service-lifecycle/` with `Shutdown`,
`ReloadSignal`, and `ServiceRunner` public API stubbed; runner method
bodies are `todo!()` pending Phase 1 implementation. Awaiting PR #289
(`chore: drop production panics across the workspace`) to merge into
`main`; this plan rebases on the post-#289 baseline.

**Date:** 2026-05-22
**Branch:** `worktree-issue-294`
**Issue:** [#294 — look over all services and simplify/unify the signal handler installation](https://github.com/ivonnyssen/rusty-photon/issues/294)
**Closest precedent:** [`docs/plans/shared-transport-extraction.md`](shared-transport-extraction.md) (multi-phase workspace-wide infra crate adopted by N services)
**Crate design doc:** [`docs/crates/rusty-photon-service-lifecycle.md`](../crates/rusty-photon-service-lifecycle.md)
**Skill doc (to be written in Phase 3):** `docs/skills/service-lifecycle.md`

## Outcomes

*(Filled in as phases ship.)*

- **Phase 0** (design + scaffolding): in progress. Crate skeleton
  landed on `worktree-issue-294`; runner method bodies are `todo!()`.
  Crate design doc at `docs/crates/rusty-photon-service-lifecycle.md`.
- **Phase 1** (crate implementation + filemonitor migration): pending.
- **Phase 2** (10 service migrations + phd2-guider SIGTERM fix): pending.
- **Phase 3** (skill doc + workspace docs polish): pending.

## Background

Every long-running binary in the workspace installs OS signal handlers
for graceful shutdown. Twelve services have drifted into roughly four
shapes for the same task:

1. **Pre-PR-289 `.expect()` panic-on-install pattern.** Eight call
   sites — `dsd-fp2` (twice, in both `main.rs` and `lib.rs`),
   `pa-falcon-rotator`, `ppba-driver`, `qhy-focuser`,
   `star-adventurer-gti`, `sky-survey-camera`, `rp`. The function is
   named `shutdown_signal()` in all of them and is structurally
   identical down to the bracing.
2. **Post-PR-289 no-panic pattern.** Three call sites — `sentinel`,
   `calibrator-flats`, `plate-solver`. Each adopts a longer
   `if let Err(e)` / `match` / `std::future::pending` shape that
   logs install failures via `tracing::warn!` instead of panicking.
   The three implementations differ among themselves (`plate-solver`
   races SIGTERM+SIGINT as a pair; `sentinel` drives a
   `CancellationToken` from a spawned task; `calibrator-flats` is
   closest to "standard").
3. **`phd2-guider`** — bug: inline `tokio::signal::ctrl_c()` only,
   no SIGTERM handler. `systemctl stop` / `kill -TERM` will not
   shut it down gracefully.
4. **`filemonitor`** — the most elaborate: SIGHUP-for-reload on Unix
   plus a full Windows Service Control Manager integration
   (`services/filemonitor/src/service.rs`, ~100 lines, the only
   `windows-service` crate adopter). Its `run_server_loop` accepts
   stop and reload signals as
   `FnMut() -> Pin<Box<dyn Future<Output = ()>>>` closures, which is
   awkward to test and reason about.

Net of PR #289: the divergence is actively *growing*. Three services
adopt a longer no-panic shape; eight remain on the panic shape; one
is buggy; one is special. The diversity is an artifact of building
these services over time, not a principled difference in what they
need.

The crate design itself —
[`docs/crates/rusty-photon-service-lifecycle.md`](../crates/rusty-photon-service-lifecycle.md)
— covers scope, public API, behavior, and usage examples. This plan
covers only the *migration* from the four current shapes to the
unified one.

## Goals

1. **One propagation primitive** workspace-wide:
   `tokio_util::sync::CancellationToken`. Today, raw one-shot futures
   coexist with sentinel's `CancellationToken` and filemonitor's
   `Notify`-and-closures.
2. **One no-panic signal-install path** (the PR #289 pattern) in one
   place, used by every service.
3. **Filemonitor's reload-on-SIGHUP and Windows Service mode work
   under the same abstraction** as everyone else — no special-case
   shape inside filemonitor.
4. **Adding SCM mode to a second service in the future** is a ~3-line
   change to that service's `main.rs` plus a `Cargo.toml` feature
   flip, not a copy of `service.rs`.
5. **Migration is reviewable**: one commit per service, behavior-
   preserving except for the phd2-guider SIGTERM bug fix.

## Non-goals (of the migration)

* **Installer packaging.** No msi, winget, `sc create` scripts,
  systemd unit files. Today only `filemonitor` ships an SCM story;
  this plan extracts the *runtime code* so adding SCM to another
  service is cheap, but produces no installation artifacts. Tracked
  separately whenever the second SCM adopter materializes.
* **Forcing SCM adoption.** The crate *supports* SCM via an opt-in
  cargo feature; only `filemonitor` enables it in Phase 1. Other
  services may opt in later on their own timelines.
* **Rewriting `BoundServer` / `ServerBuilder` across services.** The
  unified shutdown handle is passed *into* the existing
  `tokio::select!` shape (or to
  `axum::serve(...).with_graceful_shutdown(...)` for axum services).
  Server types keep their current API.

## Migration decisions resolved

These resolve open questions raised during design discussion. Crate
design decisions (one crate not two, builder not macro, runner owns
runtime, naming, no `tracing` init in runner) are documented in the
[crate design doc](../crates/rusty-photon-service-lifecycle.md);
migration-specific decisions live here.

* **Defer installer packaging.** Architectural payoff up front
  (unified `Shutdown` abstraction, SCM-as-source pattern), per-service
  SCM adoption + packaging deferred until needs arise. Avoids the
  "ocean-boil" of rebuilding the entire Windows lifecycle story
  across the workspace in one PR. Reversible: if SCM never spreads
  beyond filemonitor, no work is wasted.
* **One commit per service migration.** Each migration is small,
  behavior-preserving, and individually reviewable. Bundling them
  would obscure the per-service before/after.
* **phd2-guider SIGTERM bug fix lands as part of its migration
  commit**, not as a separate prior PR. The fix falls out of adopting
  the runner; isolating it would be paperwork without value.

## Phase 0 — design + scaffolding *(in progress)*

* Crate design doc at
  [`docs/crates/rusty-photon-service-lifecycle.md`](../crates/rusty-photon-service-lifecycle.md).
* This plan doc.
* Crate scaffold at `crates/rusty-photon-service-lifecycle/` with full
  public API surface, `//!` doc comments, and `todo!()` bodies on the
  runner methods. Compiles clean under both `--all-features` and
  default-features.
* Workspace `Cargo.toml` updated: added the crate to `members` and to
  `[workspace.dependencies]`.

## Phase 1 — crate implementation + filemonitor migration

Single PR. Proves the design end-to-end (both signal-driven console
mode and SCM mode) before any other service touches the crate.

### Crate implementation

* Implement `ServiceRunner::run` and `run_with_reload`:
  * Build a multi-thread tokio runtime.
  * Spawn the signal-watcher task; have it cancel a
    `CancellationToken` on first signal.
  * Spawn the reload-watcher task (SIGHUP / SCM ParamChange) if
    `with_reload`.
  * `block_on(run_fn(Shutdown::from_token(token)))`.
* Implement the SCM dispatch path behind `#[cfg(feature = "scm")]`.
  Stash config in a `OnceLock`; `ffi_service_main` re-enters the
  runtime build with SCM-driven token + reload signal.
* Crate unit tests — contract for `Shutdown`/`ReloadSignal`, the
  signal-install path (SIGTERM kicks the token), runner-invokes-
  closure-exactly-once. SCM dispatch is manually validated via
  filemonitor's existing Windows smoke test, not in CI. Detail in
  the crate design doc's [Testing
  section](../crates/rusty-photon-service-lifecycle.md#testing).

### Filemonitor migration

* Delete `services/filemonitor/src/service.rs`.
* Rewrite `main.rs` `run_with_reload` body to use the runner.
* Change `run_server_loop` signature from two pinned-future-
  returning closures to `(CancellationToken, ReloadSignal)`.
* Update `docs/services/filemonitor.md` to reference the shared
  crate for the SCM/reload lifecycle, removing the duplicated
  explanation.

### Risks

* **Runtime ownership flip.** Most services use `#[tokio::main]`; the
  runner builds its own runtime. Confirmed during inventory that
  filemonitor already uses `Runtime::new()?`, so the SCM-side flip is
  zero-risk. For other services in Phase 2, the migration moves their
  `main` from `#[tokio::main] async fn` to plain `fn` wrapping the
  runner — same effective shape, no behavioral change.
* **Closure `Send + 'static` bound.** Services must `move` `args`
  into the closure. Idiomatic; no new lifetime gymnastics. Documented
  in the crate's [Public API
  section](../crates/rusty-photon-service-lifecycle.md#public-api).
* **SCM regression in filemonitor.** The highest-risk migration
  because filemonitor is currently the only SCM adopter. Mitigated by
  preserving exact behavior (Stop→cancel, ParamChange→notify,
  Interrogate→NoError) and running filemonitor's existing Windows
  smoke test before merging.

## Phase 2 — remaining service migrations

One commit per service, ordered by complexity / risk (lowest first):

1. `dsd-fp2` — has the duplicate-function quirk; collapses to one
   call site, easiest validation.
2. `pa-falcon-rotator`
3. `ppba-driver`
4. `qhy-focuser`
5. `star-adventurer-gti`
6. `sky-survey-camera`
7. `rp`
8. `calibrator-flats` (axum-style)
9. `plate-solver` (axum-style; also drops the redundant SIGINT
   handler)
10. `sentinel` (drops the hand-rolled `tokio::spawn` + token plumbing;
    sources `shutdown.token()` directly into the engine)
11. `phd2-guider` (gains SIGTERM as a side fix — the only behavioral
    change in the entire migration; add a regression test asserting
    clean shutdown within bounded time on SIGTERM)

Each commit:

* Replaces `shutdown_signal()` (or equivalent) with
  `ServiceRunner::run`.
* Deletes the now-unused `use tokio::signal` / helper imports.
* Updates the service's `docs/services/<name>.md` if shutdown is
  documented there (most aren't).

## Phase 3 — workspace docs polish

* Add `docs/skills/service-lifecycle.md` capturing the standard
  pattern, the runner API, and a "how to enable SCM mode" section
  pointing at filemonitor as the worked example.
* Update `docs/skills/development-workflow.md` to reference the new
  skill doc when scaffolding a new service.
* Update `CLAUDE.md` if its service-creation guidance lists signal
  handling as boilerplate.

## Future work (explicitly deferred)

* **Per-service SCM adoption.** `sentinel` is the most plausible next
  adopter (unattended, long-running, no client coupling). When that
  happens, the change is `+ features = ["scm"]` in its `Cargo.toml`
  plus `.scm_mode(args.service)` in main. No new SCM code.
* **Installer packaging.** msi/winget/sc-create scripts; systemd unit
  files. Tracked separately once the second SCM adopter materializes
  (so the install story isn't one-off-shaped).
* **Shutdown timeout** — `ServiceRunner::with_shutdown_timeout(d)`.
  Adds "if user closure hasn't returned within `d` after cancellation,
  log error and exit anyway." Useful for catching hangs; not needed
  today.
* **Structured shutdown reason.** Today `Shutdown::cancelled()` is
  `() -> ()`. Could become
  `() -> ShutdownReason::{CtrlC, Sigterm, Sighup, ScmStop, ...}` for
  diagnostic logging. Additive, postpone.
* **systemd readiness notifications** (`sd_notify(READY=1)`). For
  services started under systemd `Type=notify`. Additive on the
  runner; postpone until there's an asker.

## Current state inventory (reference)

Reflects the codebase with PR #289 applied; one row per shutdown
call site.

| Service | Path | Pattern | Reload | SCM |
|---|---|---|---|---|
| `dsd-fp2` | `services/dsd-fp2/src/main.rs:44` | OLD (`.expect`) | — | — |
| `dsd-fp2` | `services/dsd-fp2/src/lib.rs:168` | OLD (`.expect`) — second copy | — | — |
| `pa-falcon-rotator` | `services/pa-falcon-rotator/src/main.rs:47` | OLD | — | — |
| `ppba-driver` | `services/ppba-driver/src/main.rs:168` | OLD | — | — |
| `qhy-focuser` | `services/qhy-focuser/src/main.rs:46` | OLD | — | — |
| `star-adventurer-gti` | `services/star-adventurer-gti/src/main.rs:52` | OLD | — | — |
| `sky-survey-camera` | `services/sky-survey-camera/src/lib.rs` | OLD | — | — |
| `rp` | `services/rp/src/lib.rs` | OLD | — | — |
| `phd2-guider` | `services/phd2-guider/src/main.rs:269` | BUGGY (Ctrl+C only) | — | — |
| `sentinel` | `services/sentinel/src/lib.rs:234` | NEW + `CancellationToken` | — | — |
| `calibrator-flats` | `services/calibrator-flats/src/lib.rs:99` | NEW + axum graceful | — | — |
| `plate-solver` | `services/plate-solver/src/lib.rs:122` | NEW + SIGTERM/SIGINT pair | — | — |
| `filemonitor` | `services/filemonitor/src/main.rs:51` (console) + `service.rs` (SCM) | OLD (Phase 1 deletes) | **SIGHUP** | **windows-service** |
