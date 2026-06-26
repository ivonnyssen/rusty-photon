# Phase 2 §2.2 + §2.3 — Predictive park & move_focuser deadlines (implementation plan)

**Status: COMPLETE (archived 2026-06-14).** Delivered in PR #352. The
`FocuserStepsPerSec` config newtype, `compute_park_deadline` /
`compute_focuser_deadline`, the `deadline` parameters threaded through the
two inner poll helpers, and the `park_started` / `move_focuser_started`
deadline stamps all shipped to `main` and are verified there
(`services/rp/src/config/focuser.rs`, `services/rp/src/mcp/internals.rs`).
Park keeps its no-auto-abort contract; the hardcoded 300 s / 120 s ceilings
are gone (now the named `PARK_DEADLINE_FALLBACK` / `FOCUSER_DEADLINE_FALLBACK`).
Parent plan:
[`predictive-deadlines-and-watchdog.md`](predictive-deadlines-and-watchdog.md).

Execution plan for **§2.2 (park)** and **§2.3 (move_focuser)** of
[`predictive-deadlines-and-watchdog.md`](predictive-deadlines-and-watchdog.md):
replace the two remaining hardcoded blocking-poll ceilings — `park`'s 300 s
and `move_focuser`'s 120 s — with deadlines sized per call, and populate the
`predicted_duration_ms` / `max_duration_ms` envelope fields that Phase 1
reserved. Follows the slew template established by §2.1
([`predictive-deadlines-phase2-slew.md`](predictive-deadlines-phase2-slew.md),
PR #348).

## Status

| Step | What | State |
|---|---|---|
| 0 | Design-doc updates (rp.md park/move_focuser rows, Event Envelope, config, Sentinel monitored-ops, keep-alive note) | this PR |
| 1 | `FocuserStepsPerSec` config newtype + `FocuserConfig` field | this PR |
| 2 | `compute_park_deadline` + `compute_focuser_deadline`; `deadline` params on the two inner poll helpers | this PR |
| 3 | Stamp `park_started` / `move_focuser_started` envelopes with deadlines | this PR |
| 4 | Unit tests (deadline math, parameterised §2.6 timeout tests) + BDD (`operation_events.feature`) | this PR |

**Scope: the park and move_focuser families only.** Exposure (§2.4) and
centering (§2.5) are **out of scope** and untouched — see the parent plan and
the §"Why exposure & centering are deferred" note below.

## Goal

```text
# park (§2.2)
predicted = PARK_WORST_CASE_TRAVERSE / mount.slew_rate_arcsec_per_sec + settle
max       = max(predicted × 2, MIN_PARK_DEADLINE)          # 60 s floor

# move_focuser (§2.3)
predicted = |target_position − current_position| / focuser.steps_per_sec
max       = max(predicted × 2, MIN_FOCUSER_DEADLINE)       # 5 s floor
```

`max` becomes the poll deadline (replacing the fixed 300 s / 120 s);
`predicted` and `max` are emitted (rounded to ms) on the `*_started` envelope.

## Decisions

- **D1 — Park has no distance to scale from; size it off a worst-case
  traverse.** The generic Alpaca `Telescope` trait exposes no park-position
  getter, so rp cannot compute a great-circle distance to the park position
  the way `slew` does. The deadline is therefore the worst-case full-axis
  traverse — `PARK_WORST_CASE_TRAVERSE_DEG = 180°`, the maximum angular
  separation between any two points on the sphere — at the configured slew
  rate. This still tightens the deadline relative to the old flat 300 s for
  any rig that sets a realistic `slew_rate_arcsec_per_sec`, and it is the
  honest upper bound rp can know without reading the park coordinates.
- **D2 — Park reuses `mount.slew_rate_arcsec_per_sec`; no new config field.**
  Park and slew share the mount's one slew rate. `settle_after_slew` is the
  settle term. So §2.2 adds *zero* config surface.
- **D3 — Park keeps its no-auto-abort contract.** rp.md line 749 / the
  `do_park_blocking` doc-comment: "a partially-completed park is closer to
  safe than an aborted one." The new deadline only changes *when* `park`
  surfaces the timeout error; it still does **not** call `abort_slew`. The
  corrective-action ladder (Phase 5) owns that decision.
- **D4 — Park headroom is ×2, not slew's ×3.** Slew's ×3 sits over a
  *measured* small distance (absorbing accel ramps + rate optimism). Park's
  `predicted` is already a worst-case 180° traverse, so a smaller ×2 headroom
  is right — ×3 over worst-case would re-approach the old 300 s and defeat the
  point. `MIN_PARK_DEADLINE = 60 s` (more generous than slew's 30 s floor —
  park traverses to a fixed mechanical position that can be a long way off,
  and OmniSim's BDD park is a from-rest physical traverse).
- **D5 — `focuser.steps_per_sec` as a validating newtype.** Same
  parse-don't-validate pattern as `SlewRateArcsecPerSec` (D6 of §2.1):
  `serde(try_from = "f64")`, rejects non-finite / ≤ 0, named in the error,
  fails at config load. The Alpaca `Focuser` trait has no step-*rate* property
  (`MaxIncrement`/`MaxStep` are step *counts*, not rates), so config is the
  only source.
- **D6 — `DEFAULT_FOCUSER_STEPS_PER_SEC = 500`.** Deliberately conservative
  (slow) — about half a typical EAF/Q-Focuser rate — so `predicted`
  over-estimates the move duration and the deadline won't false-abort a
  healthy move (mirrors §2.1's deliberately-slow 2°/s slew default). At
  500 steps/s a typical autofocus move (a few thousand steps) gets a
  single-digit-second deadline vs. the old flat 120 s, while a rare near-full
  travel gets *more* headroom than 120 s (which the old flat ceiling would
  have false-aborted). Tune up per-rig for a tighter bound.
- **D7 — Self-compute from a pre-move position read.** `do_move_focuser_blocking`
  reads the focuser's current `position()` before issuing the move (the same
  accessor used post-move), so `|target − current|` is exact. Mirrors §2.1's
  D2 pre-slew pointing read.
- **D8 — Read-failure ⇒ fall back to the legacy ceiling.** A flaky pre-move
  position read (or an unresolvable focuser) must not fail an otherwise-valid
  move: the focuser deadline degrades to `FOCUSER_DEADLINE_FALLBACK = 120 s`
  and the envelope deadline fields are omitted; the move proceeds (the inner
  helper still produces the authoritative result/error). Park's analogous
  fallback is `PARK_DEADLINE_FALLBACK = 300 s` when no mount is configured.
- **D9 — Builders, not signature changes.** Reuse §2.1's chainable
  `EventEnvelope::with_deadlines`; the tool-layer `park_inner` /
  `move_focuser_inner` signatures are unchanged (the deadline is self-computed
  inside the `do_*_blocking` wrapper).

## Code anchors (verified)

- `services/rp/src/config/focuser.rs` — add `FocuserStepsPerSec` + field.
- `services/rp/src/mcp/internals.rs`
  - new consts: `MIN_PARK_DEADLINE` (60 s), `PARK_DEADLINE_FALLBACK` (300 s),
    `PARK_WORST_CASE_TRAVERSE_DEG` (180), `PARK_DEADLINE_HEADROOM` (2);
    `MIN_FOCUSER_DEADLINE` (5 s), `FOCUSER_DEADLINE_FALLBACK` (120 s),
    `FOCUSER_DEADLINE_HEADROOM` (2).
  - `compute_park_deadline` (sync; no pointing read) +
    `compute_focuser_deadline` (reads current position).
  - `do_park_blocking` / `do_park_blocking_inner` — compute + thread deadline,
    stamp `park_started`; inner's `total_budget` becomes the `deadline` param.
  - `do_move_focuser_blocking` / `do_move_focuser_blocking_inner` — same shape.
- `services/rp/src/mcp/tests.rs` — `focuser_registry` literal gains
  `steps_per_sec: Default::default()`; relax `assert_end_mirrors_start` to
  exempt park/move_focuser starts; assert deadlines on the two triple tests;
  add §2.6 `elapsed <` assertions to the two timeout tests.
- `services/rp/tests/features/operation_events.feature` + steps — flip the
  park scenario to "carries the deadline fields"; add a move_focuser scenario.

## Why exposure & centering are deferred

- **§2.4 exposure** is envelope-stamping, not enforcement: rp.md §2.4 says rp
  itself does not enforce the exposure deadline (the camera driver does), so it
  is a different shape (a `camera.readout_time_estimate` config + envelope
  stamp on `exposure_started`, leaving `do_capture`'s grace as the rp backstop).
- **§2.5 centering** inherits §2.1's per-slew deadline through `do_slew_blocking`
  already (slew §2.1 Decision D1). Whether to add an *outer* `centering_*`
  deadline is the parent plan's open question; it is its own decision and PR.

Both belong in a separate PR so this one stays "replace the two hardcoded
blocking-poll ceilings."

## Acceptance

- `do_park_blocking`'s `Duration::from_secs(300)` and
  `do_move_focuser_blocking`'s `Duration::from_secs(120)` are gone (replaced by
  the computed `deadline` param; the fallbacks are explicit named constants).
- `park_started` / `move_focuser_started` carry
  `predicted_duration_ms`/`max_duration_ms`; a stuck mount/focuser fails in its
  computed window, not at 300 s / 120 s (§2.6 timeout tests assert `elapsed`
  lands below the old ceiling under `start_paused` virtual time).
- rp.md documents both formulas, the floors, and the
  `focuser.steps_per_sec` config (CLAUDE.md rule 2).
- All gates green; existing webhook/BDD delivery unaffected.
