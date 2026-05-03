# Plan: sky-survey-camera mount-following + simulated pointing offset

**Date:** 2026-05-03
**Branch:** `worktree-trail-mount`
**Parent service:** [`docs/services/sky-survey-camera.md`](../services/sky-survey-camera.md) — retires the *Telescope-following mode* item under Future Work.
**Consumer plan:** [`docs/plans/image-evaluation-tools.md` Phase 6c-3](image-evaluation-tools.md#phase-6c-3--center_on_target-design--bdd--impl) — `center_on_target` BDD currently has to fake convergence via a synthetic `PlateSolveOps` adapter ("OmniSim does not model pointing"). This plan removes that caveat for the end-to-end path.

## Background

`sky-survey-camera` is the rusty-photon stack's deterministic CCD simulator: given a fixed optical system and an `(RA, Dec, rotation)`, it returns a SkyView cutout that matches what a real OTA at that pointing would see. Today the pointing is set externally — either from `pointing.initial_*` in config or via `POST /sky-survey/position` — and there is no link between the camera and any mount.

That means the centering loop (`slew → expose → plate-solve → sync_mount → repeat`) cannot be exercised end-to-end in simulation. ASCOM's OmniSim Telescope and OmniSim Camera both work in isolation, but OmniSim's camera doesn't render a sky and OmniSim's telescope doesn't render pointing onto any camera. The Phase 6c-3 plan acknowledges this and falls back to a synthetic adapter for happy-path convergence assertions — so the integration of `slew` + `capture` + `plate_solve` + `sync_mount` is never run as a single closed loop in CI.

This plan closes that gap by:

1. Letting `sky-survey-camera` snapshot its pointing from an attached ASCOM Telescope at exposure time (the deferred *Telescope-following mode*).
2. Adding a configurable angular offset between what the mount thinks it's pointing at and what the camera renders, so the centering loop has something non-trivial to converge to.
3. Adding a centering BDD scenario that wires OmniSim Telescope + sky-survey-camera (in following mode, with offset) + `plate-solver` + `rp` and asserts `center_on_target` actually converges.

## Goals

1. **Telescope-following mode in `sky-survey-camera`.** Optional `pointing.telescope` config block; when set, each `StartExposure` reads RA/Dec from the configured Alpaca Telescope instead of from the cached `PointingState`. Static-pointing mode (today's behaviour) remains the default.
2. **Pointing-offset injection.** A configurable angular offset, applied on top of the mount-reported position before the SkyView request. Models cone error / pointing-model residuals in a way OmniSim cannot. Static-pointing mode is unaffected.
3. **One closed-loop centering BDD scenario.** OmniSim Telescope + `sky-survey-camera` (following mode + injected offset) + `plate-solver` (mock_astap) + `rp.center_on_target`, asserting convergence to within tolerance in ≤ N iterations.
4. **No regression of v0 contracts.** Static pointing, `POST /sky-survey/position`, the C/P/E/S/A behavioral contracts, and the ConformU integration test all continue to pass unchanged when `pointing.telescope` is absent.

## Out of scope

- A new ASCOM **Telescope** simulator. OmniSim's Telescope is sufficient for BDD; no need to reinvent it.
- Per-axis non-linear pointing models (e.g. cone error as a function of altitude). A constant `(Δra_arcsec, Δdec_arcsec)` is enough for the centering test to be non-vacuous; richer error models are deferred.
- Tracking-rate simulation (sidereal drift while exposing). `StartExposure` snapshots the mount once at the start, same as today.
- Plate-solving the synthetic FITS for real. The centering BDD uses `mock_astap` per the established BDD pattern; real-solver coverage stays in the `@requires-astap` tier.
- Multi-mount support — sky-survey-camera follows at most one Telescope, mirroring `rp`'s singular-mount contract.

## Decisions resolved (during design)

These are the load-bearing choices. They belong in `docs/services/sky-survey-camera.md` once Phase 1 lands and should not be re-litigated.

### Pointing source is a static enum, chosen at construction

```rust
pub enum PointingSource {
    Static(SharedPointing),                  // v0 behaviour
    Telescope(TelescopeFollow),              // new
}
```

`TelescopeFollow` owns the `Arc<dyn Telescope>` plus the configured offset. The enum is selected once in `SkySurveyCamera::new` from the parsed `Config` and never changes for the life of the device — switching modes at runtime would require teaching `POST /sky-survey/position` to "fall back" or "override," which is a feature creep we don't need.

The exposure pipeline calls `pointing_source.snapshot().await -> PointingState` regardless of variant. `Static` reads the `RwLock`; `Telescope` reads the mount and adds the offset. Both return the same `PointingState` shape so the rest of the pipeline (`build_full_sensor_request`, cache lookup, FITS fetch) is unchanged.

### `POST /sky-survey/position` returns 409 in follow-mode

When `pointing.telescope` is set, `POST /sky-survey/position` cannot meaningfully update anything — the next exposure will overwrite whatever was written by reading the mount again. The honest answer is to refuse the write with `409 Conflict`. `GET /sky-survey/position` continues to work and returns the *most recent snapshotted* `PointingState` (mount RA/Dec + offset), so test harnesses can still observe the camera's view of the world.

This is a new behavioral contract (P8), not a regression: the v0 spec says `POST` is rejected with 409 only when disconnected. Adding a second 409 reason is consistent with the spirit of P6 ("write rejected when it cannot take effect").

### The offset is a constant `(arcsec, arcsec)` in topocentric RA/Dec

```json
"telescope": {
    "alpaca_url": "http://127.0.0.1:32323",
    "device_number": 0,
    "offset_ra_arcsec": 60.0,
    "offset_dec_arcsec": -45.0,
    "request_timeout": "2s",
    "auth": null
}
```

A constant in topocentric coordinates is what `sync_mount` corrects in one shot — exactly the centering test's premise. Modeling cone error as a function of altitude or a full pointing-model polynomial is realistic but obscures whether a *test failure* is in the centering algorithm or in the pointing model itself. The simple constant keeps the test diagnostic.

After `sync_mount`, the mount's reported RA/Dec shifts to match the solved center, so the offset's effect on the *next* SkyView request shrinks (mount reports `target + ε`, cutout is for `target + ε + offset`, but `ε ≈ 0` after sync, so cutout is for `target + offset` — which the next solve picks up cleanly). This is what makes the loop converge in a single iteration after sync, matching the 6c-3 contract's "sync on first iteration only" invariant.

### The Alpaca client uses `ascom-alpaca`'s `Client`, same as `rp`

`services/rp/src/equipment.rs` already wraps `ascom_alpaca::Client::get_devices()` + the `Telescope` trait. We reuse that pattern verbatim — `build_alpaca_client` lifted into a tiny helper or duplicated, whichever the reviewer prefers. The `client` feature on `ascom-alpaca` is *already* enabled in `services/sky-survey-camera/Cargo.toml` (line 24), so nothing new lands in `Cargo.lock` from this work item alone.

The mount connection is established **lazily on the first exposure** that needs it, not at `set_connected(true)`. Reasons:

- C3 says ASCOM Connect must not block on slow upstream handshakes. An early Telescope connect would re-introduce that flakiness for follow-mode users.
- Following mode genuinely doesn't need the mount until an exposure is actually requested. Lazy connect keeps the camera startable even when the mount is briefly unreachable.
- A failed Telescope read at exposure time becomes an `UNSPECIFIED_ERROR` (per S4's pattern: "endpoint unreachable → exposure fails, next attempt may retry"). Symmetric with how SkyView errors are surfaced.

### `request_timeout` is short and bounded by config

The Telescope read happens in the hot path of every `StartExposure`. We bound it (default 2 s) and surface it as `UNSPECIFIED_ERROR` on timeout, with a `warn!` log. ASCOM's standard read-only properties (`right_ascension`, `declination`) should respond in milliseconds against any sane mount; the bound exists to prevent a wedged mount from extending exposure latency by 30 s.

### No client-side caching of mount RA/Dec

Each exposure reads the mount fresh. Caching across exposures would mask `slew` events that other clients (rp itself) issued between exposures, defeating the whole point of follow-mode. The exposure rate is bounded by ASCOM's request cycle anyway, so the read pressure on the Telescope is negligible.

### BDD wires the same OmniSim that already runs for mount tests

`crates/bdd-infra/src/rp_harness/omnisim.rs` already spawns a singleton OmniSim shared by all scenarios. The centering scenario reuses that singleton — both `rp` and `sky-survey-camera` point at it, and OmniSim's Telescope state is shared between them. No new external simulator binary, no new harness plumbing beyond config wiring.

The `mount.feature` scenarios that mutate Telescope state (e.g. `park`) are already independent per-scenario via `set_connected(false)` cycles; the centering scenario plays nicely with those because OmniSim Telescope state is reset at scenario start by `rp`'s connect lifecycle.

## Configuration changes

### `sky-survey-camera` — new optional `pointing.telescope` block

```jsonc
{
  // ... unchanged: device, optics, survey, server ...
  "pointing": {
    "initial_ra_deg": 83.8221,
    "initial_dec_deg": -5.3911,
    "initial_rotation_deg": 0.0,
    "telescope": {                         // NEW, optional
      "alpaca_url": "http://127.0.0.1:32323",
      "device_number": 0,
      "offset_ra_arcsec": 0.0,
      "offset_dec_arcsec": 0.0,
      "request_timeout": "2s",
      "auth": null
    }
  }
}
```

Schema rules:

- `pointing.telescope` absent → static mode, today's behaviour.
- `pointing.telescope` present → follow mode. `initial_*` fields are still parsed but used only as a one-time fallback if the very first mount read fails (so the camera doesn't hard-error before `rp` has finished its own connect handshake).
- `offset_*_arcsec` default to 0, so following mode without an offset is the realistic-mount case (no error to converge from — that's a config choice, not a default behaviour).
- `request_timeout` is humantime, defaults `2s`, validated `> 0`.
- `auth` reuses `rp_auth::config::ClientAuthConfig` for symmetry with `rp`'s mount config.

### BDD harness — `RpHarness` wires sky-survey-camera

`crates/bdd-infra/src/rp_harness/config.rs` currently builds `rp` configs only. It needs a small extension so a scenario can spawn `sky-survey-camera` configured to follow the same OmniSim that `rp`'s mount points at. New fields and a builder helper, parallel to `with_mount`:

```rust
pub struct SkySurveyCameraConfig {
    pub follow_telescope: bool,
    pub offset_ra_arcsec: f64,
    pub offset_dec_arcsec: f64,
}
```

This stays in the harness; the production `sky-survey-camera` config schema is the canonical surface.

## New behavioral contracts

Added to `docs/services/sky-survey-camera.md` "Behavioral Contracts":

### Telescope follow mode

- **F1.** With `pointing.telescope` set, every `StartExposure` reads `right_ascension` and `declination` from the configured ASCOM Telescope and snapshots `PointingState { ra: mount_ra + offset_ra, dec: mount_dec + offset_dec, rotation: rotation_at_last_post_or_initial }`. Rotation is *not* sourced from the mount (Telescope has no rotation property in standard ASCOM); it comes from `pointing.initial_rotation_deg` or the last `POST /sky-survey/position` body before follow-mode took effect. (Scope note: rotation control is out of scope for this plan; future work could add a connected ASCOM Rotator.)
- **F2.** A failed Telescope read (timeout, transport error, ASCOM error) at exposure time returns `UNSPECIFIED_ERROR`, sets `last_error`, leaves `image_ready = false`. Logged at `warn!`. The next `StartExposure` retries the read fresh.
- **F3.** With `pointing.telescope` set and the mount unreachable, `set_connected(true)` still succeeds (parallel to C3 for SkyView). The next exposure surfaces the error via F2.
- **F4.** Mount RA/Dec are read fresh every exposure; no client-side cache.
- **F5.** The configured offset (`offset_ra_arcsec`, `offset_dec_arcsec`) is added to mount-reported RA/Dec before the SkyView request. RA wrap-around: `(mount_ra_deg + offset_ra_arcsec/3600).rem_euclid(360)`. Dec is clamped to `[-90, +90]` (an offset that pushes past the pole produces a clamped pointing and a `warn!`; this is a config error worth surfacing, not silently saturating).
- **F6.** `POST /sky-survey/position` while `pointing.telescope` is set returns `409 Conflict` with a body indicating the mode. `GET /sky-survey/position` returns the most recently snapshotted `PointingState` (mount + offset).

### `rp` — no new contracts in this plan

`center_on_target` is owned by Phase 6c-3. This plan delivers the simulator infrastructure that 6c-3's BDD will use; the `center_on_target` contract itself is unchanged.

## Phases

Each phase is its own PR. All four phases land on this branch (`worktree-trail-mount`) or successors; the centering BDD (Phase 4) is the integration milestone.

### Phase 1 — Service design doc update

Status: **done** — landed in `4e7691d`.

- [ ] Update `docs/services/sky-survey-camera.md`:
  - "Configuration" — add the `pointing.telescope` schema block with field-by-field description.
  - "Operation → Pointing State" — describe `PointingSource::{Static, Telescope}` and the snapshot semantics in follow mode.
  - "Behavioral Contracts" — add F1–F6 (above) and update P6 to mention the second 409 reason. Renumber P-contracts only if needed (additive is fine).
  - "Custom HTTP Endpoints" — add the 409-in-follow-mode row to the `POST` validation list.
  - "Future Work" — strike "Telescope-following mode" and replace with the more-targeted items left (rotation source, non-constant pointing models, sidereal drift during exposure).
- [ ] Add a new "Pointing Offset Simulation" subsection under Operation, explaining the offset's purpose (cone-error analog), units, sign convention, RA wrap, Dec clamp, and the diagnostic warn-log on clamp.
- [ ] No code yet.

**Exit criteria:** doc reviewed and merged. The contracts are now the spec for Phase 2.

### Phase 2 — `PointingSource` refactor + telescope-follow mode (no offset yet)

Status: **done** — landed in `6ce90ae`.

- [ ] `services/sky-survey-camera/src/pointing.rs` — introduce `PointingSource` enum. `Static(SharedPointing)` is the existing path, no behaviour change. `Telescope(TelescopeFollow)` is a new struct holding `Arc<dyn Telescope>` + its config; `snapshot()` reads RA/Dec from the mount, returns a `PointingState`. **Offset stays at zero in this phase** so the diff is bounded.
- [ ] `services/sky-survey-camera/src/config.rs` — extend `PointingConfig` with the optional `telescope: Option<TelescopeFollowConfig>`. Wire `humantime_serde` for `request_timeout`. Reuse `rp_auth::config::ClientAuthConfig` (already a workspace dep transitively via `rp`; if not, add the workspace dep — no new crates.io download).
- [ ] `services/sky-survey-camera/src/camera.rs` — replace `state.pointing: SharedPointing` with `state.pointing_source: PointingSource`. Change the snapshot call sites (`run_exposure_inner` line 212, the `routes.rs` GET handler) to `pointing_source.snapshot().await`. The `routes.rs` POST handler routes to the static variant or returns 409 (F6) on the telescope variant.
- [ ] `services/sky-survey-camera/src/lib.rs` — `SkySurveyCamera::new` constructs the right variant from config. Lazy-connect the Alpaca client: build the `Client` eagerly (cheap), but `set_connected(true)` on the Telescope only on the first `snapshot()` call (cached `OnceCell<Result<Arc<dyn Telescope>, _>>`). On error, log `warn!` and surface via F2.
- [ ] Unit tests for `PointingSource::Static` (unchanged behaviour) and `PointingSource::Telescope` with a mocked Telescope (the `ascom-alpaca` Telescope trait is `mockall`-compatible? — if not, an in-crate trait wrapper around `right_ascension`/`declination` is the standard fallback, mirroring `plate-solver`'s `AstapRunner` trait). Cover: happy path snapshot, transport error → typed error, timeout → typed error.

**Exit criteria:** `cargo build -p sky-survey-camera --all-features --all-targets` clean. Existing v0 BDD scenarios all pass unchanged. New unit tests for the follow-mode code path pass. `cargo rail run --profile commit -q` clean. No new crates.io entries in `Cargo.lock`.

### Phase 3 — Pointing offset injection + F5/F6 BDD

Status: **done** — landed in `9d4f518`. The mount stub stayed in-crate
(`tests/bdd/world.rs`) rather than landing in `bdd-infra`, since no
other service-level BDD currently needs an ASCOM Telescope mock. If a
second crate eventually needs one, the `MountStubBehavior` /
`spawn_mount_stub` shape lifts cleanly into `bdd-infra`.

- [ ] `services/sky-survey-camera/src/pointing.rs` — `TelescopeFollow::snapshot()` adds `offset_*_arcsec` to mount RA/Dec, applies RA `rem_euclid(360)`, clamps Dec to `[-90, +90]` (with `warn!` on clamp).
- [ ] `services/sky-survey-camera/src/config.rs` — `offset_ra_arcsec` / `offset_dec_arcsec` (default 0.0). No validation beyond `f64::is_finite` — large offsets are legal; the clamp handles wrap.
- [ ] Unit tests on `TelescopeFollow::snapshot()`: zero offset → mount value passes through; positive RA offset wraps at 360°; negative RA offset wraps at 0; Dec offset clamps at ±90 with warn-log expectation; non-finite offset rejected at config-load (not at snapshot).
- [ ] BDD: extend `services/sky-survey-camera/tests/features/pointing_api.feature` with F6 (`POST /sky-survey/position` returns 409 in follow-mode). The harness needs to spin up the camera with a follow-mode config; this is where the `bdd-infra` extension lands.
- [ ] BDD: new feature file `services/sky-survey-camera/tests/features/follow_mode.feature` covering F1 (snapshot reads mount), F2 (mount-read failure → S-style error), F5 (offset application + RA wrap + Dec clamp). The mount source can be a tiny in-test axum stub serving the two ASCOM endpoints we read — full OmniSim is overkill for unit-style follow-mode coverage and adds a binary discovery dependency to a service test that doesn't otherwise need it. Reserve OmniSim for Phase 4.
- [ ] `crates/bdd-infra/src/rp_harness/config.rs` — small extension to spawn `sky-survey-camera` from BDD with the follow-mode wiring. Or a dedicated launcher under `crates/bdd-infra/src/sky_survey_harness/` if the rp harness module wants to stay rp-only — call this in the PR.

**Exit criteria:** F1/F2/F5/F6 covered in BDD; `cargo test -p sky-survey-camera` green; existing v0 scenarios still pass; `cargo rail run --profile commit -q` clean.

### Phase 4 — Closed-loop centering BDD

Status: **parked, blocked on Phase 6c-3.** As of 2026-05-03 the
`center_on_target` MCP tool is not yet implemented in `rp` (no entry
in `services/rp/src/mcp.rs`, no `services/rp/src/imaging/tools/center_on_target.rs`).
Phases 1–3 deliver the simulator infrastructure 6c-3 will need;
this phase unblocks once 6c-3 lands. No code in this phase yet.

- [ ] New feature file `services/rp/tests/features/center_on_target_e2e.feature` (or appended to `center_on_target.feature` if 6c-3 already created it). One scenario:
  - **Given** OmniSim is running and exposes a Telescope at index 0
  - **And** `sky-survey-camera` is configured to follow that Telescope with `offset_ra_arcsec = 60` and `offset_dec_arcsec = -45`
  - **And** `plate-solver` is running with `mock_astap` returning a deterministic synthetic `.wcs` for the requested cutout (the mock needs to be told the cutout center; do this by parsing the `fits_path` argv and reading the FITS header SkyView wrote, *or* by extending `mock_astap` with a `MOCK_ASTAP_RESPONSE_FROM_FITS=true` mode that echoes the input frame's CRVAL1/CRVAL2)
  - **And** `rp` is configured with that mount, that camera, and that plate solver
  - **When** the BDD calls `center_on_target { ra: 83.8221, dec: -5.3911, tolerance_arcsec: 5, max_attempts: 3 }`
  - **Then** the loop converges in ≤ 2 iterations (sync corrects the offset on iteration 1; iteration 2 confirms residual within tolerance), `final_error_arcsec` ≤ 5, and the mount's reported RA/Dec is within tolerance of the target.
- [ ] Decide on `mock_astap` extension vs. a dedicated `center_on_target` plate-solver mock. The cheapest path is to teach `mock_astap` to read the `fits_path` argument and echo the FITS's `CRVAL1`/`CRVAL2` into the `.wcs` it writes — that way the mock "solves" whatever pointing the camera served, which is exactly what an honest plate solver would do for a SkyView cutout. Document the new mode in the plate-solver plan / service doc as a follow-up.
- [ ] Confirm the OmniSim Telescope responds to `slew_to_coordinates_async` and reflects the new RA/Dec via `right_ascension` / `declination` in the way `do_slew_blocking` expects. The `mount.feature` scenarios already exercise this, so it should hold.
- [ ] Tag the scenario `@e2e-centering` so it can be skipped when run-time is constrained, mirroring `@requires-astap`'s opt-in posture. The PR-required `cargo nextest` can include or exclude based on cost, decided when the scenario's wall-clock cost is measured.

**Exit criteria:** the centering scenario passes deterministically (same inputs → same convergence path) under both Cargo and Bazel test surfaces; the synthetic-adapter caveat in 6c-3's plan can be relaxed to "synthetic adapter for unit-style coverage; `@e2e-centering` for the closed loop"; `cargo rail run --profile commit -q` clean.

## Risks and open questions

### `mock_astap` round-tripping the cutout's WCS

Phase 4 hinges on the plate solver mock being able to "solve" the SkyView cutout by reading its embedded WCS — the cutout's FITS already has `CRVAL1`/`CRVAL2` set by SkyView (and by `mock::synthetic_fits` for tests that use that path). If `mock_astap` simply echoes those into its `.wcs` output, the loop converges. This *is* faithful to what a real solver would do on a SkyView cutout, since SkyView's WCS is the ground truth for that cutout. The only failure mode is if the synthetic-FITS helper ever stops setting CRVAL accurately — which would also break ConformU's stub backend, so we'd notice immediately.

If round-tripping the WCS is rejected on review (e.g. as "the mock isn't actually solving"), the fallback is to add a deterministic-noise mock mode that returns `CRVAL ± gaussian` to model real solver imprecision; the centering loop's tolerance is sized for that.

### Lazy mount-connect vs. ASCOM Connect timing

If the mount isn't connected when the camera reads it, the `Telescope` ASCOM read will return `NotConnected`. The lazy-connect path inside `TelescopeFollow::snapshot` calls `set_connected(true)` if it isn't already — but that introduces a side effect (the camera connecting the Telescope) that other clients (rp, NINA) might not expect. Risk: if rp later issues `set_connected(false)` to park, the camera's next exposure silently re-connects.

Resolution to flag in Phase 2 review: should the camera *only read* an already-connected Telescope (returning `UNSPECIFIED_ERROR` if not connected) and leave connect/disconnect to whoever owns the mount (`rp`)? That's the cleaner separation. The cost is a stricter ordering requirement in BDD: the centering scenario must connect via `rp` first, then start `sky-survey-camera`, or use a `@before` hook to issue `set_connected(true)` directly. This is the right tradeoff.

**Tentative decision**: read-only access; do not mutate the Telescope's connect state from the camera. Confirm in Phase 2 PR review.

### OmniSim Telescope behaviour under rapid slew/read sequences

If the centering loop reads RA/Dec on the OmniSim Telescope while a slew is still settling, the mount may return its in-flight position rather than the commanded one. `rp`'s `do_slew_blocking` already polls `slewing()` until idle, so by the time it returns, the mount is at the commanded position. The BDD scenario only reads the camera *after* `rp.slew` has returned, so this should be sound. If flakiness emerges in CI, the answer is a small post-slew settle (`MountConfig.settle_after_slew`) — already a first-class config knob.

### Cost of running the centering scenario in PR-required CI

The full chain (OmniSim + sky-survey-camera + plate-solver + rp + the orchestration loop) is heavier than any current single BDD scenario. Estimate: 5–10 s wall-clock on a warm runner, dominated by the OmniSim startup (already amortized as a singleton) and the SkyView fetch (cached after first run). If it lands above 15 s consistently, it goes behind `@e2e-centering` and runs only on the nightly tier — same posture as `@requires-astap`.

### Does sky-survey-camera need filter-wheel awareness for follow mode?

No — the v0 service is single-band, set at startup. A connected filter wheel changing what survey is queried is in Future Work and orthogonal to mount-following. The centering BDD uses one band.

## Testing posture

| Layer | Coverage |
|-------|----------|
| Unit (`PointingSource::Telescope`) | Mock Telescope (in-crate trait wrapper around the two ASCOM reads) — happy path, transport error, timeout, offset arithmetic, RA wrap, Dec clamp. |
| BDD (sky-survey-camera) | F1/F2/F5/F6 against a tiny in-test axum mount stub. Existing v0 scenarios continue to pass with `pointing.telescope = null`. |
| BDD (rp, e2e centering) | Phase 4 scenario — full chain, OmniSim Telescope, mock plate solver, real `rp.center_on_target` loop. |
| ConformU | Unchanged — runs in static mode, no follow-mode involvement. |

The unit and sky-survey-camera-BDD layers are self-contained per `docs/skills/testing.md` ("test the smallest amount of functionality possible") and stay green even if `rp` or `plate-solver` are broken. The centering BDD is the one scenario where all four services have to agree, and it lives in `rp`'s test suite because `rp` is the orchestrator.

## Future work

- **Telescope rotator support.** When a connected ASCOM Rotator becomes a thing, `TelescopeFollow` reads `position_angle` from it and feeds the camera's `rotation_deg`. Today's plan leaves `rotation` static.
- **Non-linear pointing models.** Az/alt-dependent offsets, polar misalignment, atmospheric refraction. Useful for stress-testing `center_on_target`'s robustness; not needed for a first integration test.
- **Sidereal drift during exposure.** Snapshot at start *and* end, interpolate. Today's pipeline snapshots only at start.
- **OmniSim PR upstream.** If the simple round-tripped-WCS plate-solver mock proves load-bearing for many tests, consider proposing OmniSim grow a "render sky onto camera" mode. Not on our critical path.
