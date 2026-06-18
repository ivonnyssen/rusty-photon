# Phase 2 §2.1 — Predictive slew deadline (implementation plan)

**Status: COMPLETE (archived 2026-06-14).** Delivered in PR #348. The
`SlewRateArcsecPerSec` config newtype, the `EventEnvelope::with_deadlines`
builder, the great-circle slew-deadline computation in `do_slew_blocking`,
the `deadline` parameter on `poll_slewing_until_idle`, and the
`MIN_SLEW_DEADLINE` (30 s) floor all shipped to `main` and are verified there
(`services/rp/src/config/mount.rs`, `services/rp/src/mcp/internals.rs`). The
slew path's hardcoded 300 s ceiling is gone (now the named
`SLEW_DEADLINE_FALLBACK`). Parent plan (still active):
[`predictive-deadlines-and-watchdog.md`](../predictive-deadlines-and-watchdog.md).

Execution plan for **§2.1** of
[`predictive-deadlines-and-watchdog.md`](../predictive-deadlines-and-watchdog.md):
replace the slew path's hardcoded 300 s ceiling with a deadline computed
per call from the slew's actual workload, and populate the
`predicted_duration_ms` / `max_duration_ms` envelope fields that Phase 1
reserved.

## Status

| Step | What | State |
|---|---|---|
| 0 | Design-doc updates (rp.md slew row, Event Envelope, Sentinel monitored-ops, config) | this PR |
| 1 | `SlewRateArcsecPerSec` config newtype + `MountConfig` field | this PR |
| 2 | `EventEnvelope::with_deadlines` builder | this PR |
| 3 | Deadline computation in `do_slew_blocking`; `deadline` param on `poll_slewing_until_idle` | this PR |
| 4 | Unit + BDD tests (incl. parameterising the §2.6 timeout test) | this PR |

**Scope: the slew family only.** Per the parent plan's "one PR per
operation family", park (§2.2, `do_park_blocking`'s own 300 s at
`internals.rs`), move_focuser (§2.3, the 120 s helper), exposure (§2.4),
and centering (§2.5) are **out of scope** and untouched.

## Goal

Every `slew` (the primitive tool **and** each corrective slew inside
`center_on_target`) carries a deadline derived from the great-circle
distance to the target and the mount's slew rate:

```text
predicted = great_circle_arcsec(current_pointing, target) / slew_rate_arcsec_per_sec
            + settle_after.as_secs_f64()
max       = max(predicted * 3, MIN_SLEW_DEADLINE)   // 30 s floor
```

`max` becomes the poll deadline (replacing the fixed 300 s); `predicted`
and `max` are emitted (rounded to ms) on the `slew_started` envelope.

## Decisions

- **D1 — Deadline at the primitive, not the workflow.** A single slew is
  predictable; a `center_on_target` *workflow* (capture + solve + slew,
  N times) is not. So we deadline the slew primitive and let
  `center_on_target`'s per-iteration corrective slews **inherit** it for
  free — they already call `do_slew_blocking`. No workflow-level centering
  deadline, and no widening of the `MountOps::slew_to` trait. (This also
  informs §2.5: prefer per-primitive deadlines over a whole-workflow
  centering ceiling.)
- **D2 — Self-compute from a pre-slew pointing read.** `do_slew_blocking`
  reads the mount's current RA/Dec *before* issuing the slew (the same
  `right_ascension()`/`declination()` accessors already used post-slew),
  so the distance is exact for whatever the caller targets — the centering
  loop needs no extra plumbing.
- **D3 — Pre-slew-read failure ⇒ fall back to 300 s.** A flaky pointing
  read must not fail an otherwise-valid slew. On read error the deadline
  degrades to the legacy 300 s ceiling and the deadline fields are omitted
  from the envelope; the slew proceeds.
- **D4 — `MIN_SLEW_DEADLINE = 30 s`** floor, and the `× 3` headroom over
  `predicted` absorbs acceleration ramps and rate over-optimism. The floor
  value is set by the **OmniSim BDD simulator**, not a real mount: OmniSim
  slews at 20°/s with a fixed deceleration tail (`TelescopeHardware.cs`:
  `maximumSlewRate=20`, `TIMER_INTERVAL=0.1`, `SlewSettleTime=0`), and
  resets its *physical* axes to a startup position before each scenario
  while `sync_mount` moves only the *reported* coordinates — so the first
  `center_on_target` slew physically traverses home→target and takes up to
  ~12 s (a 180° axis traverse: ~88 fast ticks + decel), even though the
  reported distance rp sizes the deadline from is tiny. An initial 5 s floor
  deadlocked every `center_on_target` BDD scenario on all platforms (the
  prior hardcoded 300 s ceiling had always hidden OmniSim's slew time).
  30 s is ~2.5× OmniSim's ~12 s worst case — margin for a contended CI
  runner dropping timer ticks (the goto-slew advances a fixed angle per
  tick, so a stalled timer stretches wall-clock time). A real mount's tiny
  slew is far quicker, so the floor is slack in production yet still
  surfaces a wedged slew ~10× sooner than 300 s (and before rmcp's 300 s
  keep-alive). See the OmniSim source (sibling repo
  `ASCOM.Alpaca.Simulators/TelescopeSimulator`).
- **D5 — Reuse `haversine_arcsec`** (`imaging/tools/center_on_target.rs`),
  already wrap-safe and unit-tested. RA is in **hours** at the
  `do_slew_blocking` boundary (validated `[0, 24)` in `slew_inner`), so
  both RA args are `× 15` before the call, matching `center_on_target`.
  `rp-ephemeris`'s `angular_separation_degrees` is `pub(crate)`, returns
  degrees, and lacks wrap tests — not used.
- **D6 — `slew_rate_arcsec_per_sec` as a validating newtype**
  (`serde(try_from = "f64")`, rejects non-finite / ≤ 0), the project's
  parse-don't-validate pattern (star-adventurer-gti `config.rs` is the
  template). Default **7200 arcsec/s = 2°/s** — deliberately slower than
  real GoTo mounts (3–4°/s) so `predicted` over-estimates and the deadline
  won't false-abort; tune per-rig for a tighter bound. The generic Alpaca
  `Telescope` trait exposes no GoTo-rate property, so this config value is
  the only source.
- **D7 — `EventEnvelope::with_deadlines(predicted_ms, max_ms)`** chainable
  builder, so `started()`'s signature is unchanged for the not-yet-converted
  operations (which keep both fields `None`/omitted).

## Code anchors (verified)

- `services/rp/src/config/mount.rs` — `MountConfig`; add the newtype + field.
- `services/rp/src/events.rs` — `EventEnvelope` (the two `Option<u64>`
  fields already exist with `skip_serializing_if`); add `with_deadlines`.
- `services/rp/src/mcp/internals.rs`
  - `do_slew_blocking` / `do_slew_blocking_inner` — compute + thread the
    deadline; stamp the `slew_started` envelope.
  - `poll_slewing_until_idle` — replace `Duration::from_secs(300)` with a
    new `deadline: Duration` parameter (`do_park_blocking`'s separate 300 s
    is §2.2, untouched).
  - new `MIN_SLEW_DEADLINE` const.
- `services/rp/src/imaging/mod.rs` — `pub use` `haversine_arcsec`.
- `services/rp/src/mcp/built_in/mount.rs` / `center_on_target.rs` — no
  signature change (the deadline is self-computed inside `do_slew_blocking`).

## Tests

- **Unit** (`mcp/tests.rs`, `tokio::test(start_paused = true)`):
  - Parameterise `test_slew_timeout_returns_error_after_abort` (§2.6) so a
    far/near target drives a small `max` and the stuck mount fails within
    that window — proving the deadline scaled off 300 s.
  - distance → deadline math (current vs target → expected predicted/max),
    the `MIN_SLEW_DEADLINE` floor (current == target), and that the slew
    progress `total` equals the computed `max`.
  - Off the broadcast seam: `slew_started` now carries
    `predicted_duration_ms`/`max_duration_ms` (present, `max ≥ predicted`,
    `max ≥ floor`); relax the shared `assert_end_mirrors_start` so only the
    **end** envelope asserts the fields absent (keeps park/unpark/capture/
    plate_solve triple tests green).
  - `MountConfig` parse test for `slew_rate_arcsec_per_sec` (valid + a
    rejected non-positive value).
- **BDD** (`tests/features/operation_events.feature` + steps): the
  `slew_started` scenario asserts the deadline fields are **present** (new
  `carries deadline fields` step); the existing `reserves … as absent`
  step stays for the not-yet-converted operations. `ReceivedEvent` already
  parses both fields — no harness change.

## Acceptance

- The slew path's `Duration::from_secs(300)` is gone (replaced by the
  parameter; the fallback 300 s is an explicit named constant).
- `slew_started` carries `predicted_duration_ms`/`max_duration_ms`; a tiny
  slew floors at 30 s and a stuck mount fails in its computed window, not
  300 s.
- rp.md documents the formula, the `MIN_SLEW_DEADLINE` default, and the
  `mount.slew_rate_arcsec_per_sec` config (CLAUDE.md rule 2).
- All gates green; existing webhook/BDD delivery unaffected.
