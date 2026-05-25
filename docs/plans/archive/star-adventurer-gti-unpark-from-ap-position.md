# Star Adventurer GTi: required `unpark_from_ap_position` + recovery actions

## Status: COMPLETE (archived 2026-05-24) — implemented in PR #301 (issue #249)

All six phases below landed under issue #249. Design captured in
[`docs/services/star-adventurer-gti.md` §Unpark from AP position](../../services/star-adventurer-gti.md#unpark-from-ap-position).
Conversation log lives on issue #248 (the design tracker); issue #249
tracked the implementation.

## Motivation

The driver currently makes the operator's start-up position assumption
*implicit* — `home_pose: null` is a valid config and means "trust
whatever raw encoder the firmware reports," which on fresh power-up is
`(0, 0)` regardless of where the OTA physically is. The driver then
runs coordinate math against that zero as if it corresponded to a real
celestial pointing. This is the largest correctness gap in the driver:
every position the driver reports rests on an operator assumption that
no API surface ever asks them to commit to.

The fix is to require the assumption explicitly. Every install
declares `mount.unpark_from_ap_position`; the ship default is
`ap_park_0` = "current position, I will plate-solve" (safe, requires
operator effort to recover position); operators with a permanent setup
opt into a named park (`ap_park_1..ap_park_5`) to get the auto-seed
flow.

Three runtime Actions round out the surface so operators don't have
to edit config files mid-session: `SetUnparkFromApPosition` (persist a
new value, applies on next fresh-power-up), `SetPreferredApPark`
(park target for `Park()`), and `UnparkFromApPosition` (recovery
operation that runs the safe-reset-then-seed sequence).

This plan is not a bug fix — the driver works today for installs that
configure `home_pose` correctly. It's the structural change that turns
an implicit, easy-to-get-wrong assumption into an explicit, hard-to-
get-wrong contract.

## Scope summary

- **Config schema change**: rename `home_pose` → `unpark_from_ap_position`,
  add `ApPark0` enum variant, make the field required with ship default
  `ap_park_0`. Add `preferred_ap_park` field (optional, default
  `ap_park_3`).
- **Three custom ASCOM Actions**: `SetUnparkFromApPosition`,
  `SetPreferredApPark`, `UnparkFromApPosition`. Wired through
  `Action(name, parameters)` dispatch, advertised via `SupportedActions`.
- **`ResetMountEncoders` internal helper**: stop both axes → wait
  for idle → write `:E1` / `:E2` → clear driver state. Invoked by
  `UnparkFromApPosition(ap_park_N≥1)` and (degenerate case, stops are
  no-ops) by the fresh-power-up auto-seed.
- **Rename `seed_home_pose_after_connect` → `seed_after_connect`**.
  Skip when configured pose is `ap_park_0`. Otherwise unchanged.
- **`Park()` uses `preferred_ap_park` as the target** when no explicit
  `park_ra_ticks` / `park_dec_ticks` override is set in config.
- **No migration** — driver isn't released; we just rename.

## Pre-implementation: open questions to resolve

None blocking. The design conversation resolved every open thread:

- Eager vs lazy `SetUnparkFromApPosition`: **lazy** (persist + apply on
  next fresh-power-up). Operators wanting an "immediate apply" path use
  `UnparkFromApPosition(park)`.
- `Unpark()` semantics: **unchanged** (just clears `AtPark` flag). The
  recovery flow is the explicit `UnparkFromApPosition(park)` Action.
- Confirmation gate after fresh-power-up seed: **deferred** to a future
  config flag, to be added if hardware sessions show operator
  discipline ("if you power-cycle after a crash, physically re-park
  first") isn't enough.

## Phase A — Config schema + `ApPark0` variant

**Files**

- `services/star-adventurer-gti/src/config.rs`
- `services/star-adventurer-gti/src/lib.rs` (re-exports if needed)
- BDD test configs in `services/star-adventurer-gti/tests/features/*.feature`
  (whichever scenarios set `home_pose`)

**Changes**

1. Rename the existing `HomePose` enum → `ApPark`. Add an `ApPark0`
   variant with `#[serde(rename = "ap_park_0")]`. Update all
   `#[serde(rename = "ap_park_N")]` annotations.
2. Update `ApPark::codebase_mech_ha` / `codebase_dec_encoder` to
   return some sentinel (or be inapplicable) for `ApPark0`. Probably
   cleanest: change return types to `Option<f64>` and have `ApPark0`
   return `None` — callers must handle the "no seed values" case
   anyway. Document the contract.
3. Rename `MountConfig::home_pose: Option<HomePose>` →
   `MountConfig::unpark_from_ap_position: ApPark` (required, no
   `Option`). Use `#[serde(default = "default_unpark_from_ap_position")]`
   with the default returning `ApPark::ApPark0` so existing configs
   without the field load cleanly.
4. Add `MountConfig::preferred_ap_park: ApPark` (default
   `ApPark::ApPark3`). Same defaulted-required pattern. Reject
   `ApPark0` via a custom serde validation (it's not a slew target).

**Tests**

- Config deserialization unit tests in `config.rs`: round-trip each
  `ApPark` variant, default applied when field missing, error when
  `preferred_ap_park: "ap_park_0"`.
- `home_pose` → `unpark_from_ap_position` is a hard rename — any test
  config that referenced `home_pose` fails loudly until updated. That's
  intentional; no migration shim.

## Phase B — `ResetMountEncoders` helper

**Files**

- `services/star-adventurer-gti/src/mount_device.rs`

**Changes**

Add a private async helper:

```rust
async fn reset_mount_encoders(
    &self,
    ra_target_ticks: i32,
    dec_target_ticks: i32,
) -> ASCOMResult<()> {
    // 1. Stop both axes via existing stop_axis_and_wait helper.
    self.stop_axis_and_wait(Axis::Ra, AXIS_STOP_TIMEOUT).await?;
    self.stop_axis_and_wait(Axis::Dec, AXIS_STOP_TIMEOUT).await?;
    // 2. Write the encoder seed values.
    self.transport.send(Command::SetPosition { axis: Axis::Ra, ticks: ra_target_ticks }).await?;
    self.transport.send(Command::SetPosition { axis: Axis::Dec, ticks: dec_target_ticks }).await?;
    // 3. Clear driver-internal slew / target / tracking-flag state.
    let mut state = self.state.write().await;
    state.slew_in_progress = false;
    state.target_ra_hours = None;
    state.target_dec_degrees = None;
    state.tracking_requested = false;
    Ok(())
}
```

**Tests**

- Mock-transport test asserting the wire ordering: two `:K`s, polling
  for idle, then two `:E`s with the configured tick values.
- Test that in-memory state is cleared (slew_in_progress, target_*,
  tracking_requested).
- Test failure path: if `stop_axis_and_wait` returns an error, the
  encoder write is *not* attempted (motion still in flight).

## Phase C — Rename + adapt `seed_home_pose_after_connect`

**Files**

- `services/star-adventurer-gti/src/mount_device.rs`
- Existing tests for `seed_home_pose_after_connect`

**Changes**

1. Rename `seed_home_pose_after_connect` → `seed_after_connect`.
2. Short-circuit: if `unpark_from_ap_position == ApPark::ApPark0`,
   return immediately. No log lines, no encoder writes.
3. Otherwise: compute seed ticks from `ApPark::codebase_*` helpers
   (which now return `Option<f64>` — `Some(_)` is guaranteed for any
   non-`ApPark0` variant), call `reset_mount_encoders` internally.
   The stop steps are no-ops on a fresh-power-up mount (motors idle)
   but make the function correct regardless of when it runs.
4. Update the `info!()` log line text: `seeded firmware encoder for
   home_pose` → `seeded firmware encoder for unpark_from_ap_position`.
   Pre-seed snapshot log line is unchanged.

**Tests**

- Existing `seed_home_pose_after_connect` tests get renamed and updated
  for the new field name.
- New test: `ap_park_0` configured + fresh-power-up encoder → no `:E`
  writes, no `info!()` log lines beyond the standard connect path.
- New test: `ap_park_3` configured + non-fresh encoder → seed skipped
  (existing tolerance-based behaviour preserved).

## Phase D — Custom Action dispatch

**Files**

- `services/star-adventurer-gti/src/mount_device.rs`

**Changes**

1. Implement `async fn supported_actions(&self) -> ASCOMResult<Vec<String>>`
   in the `ITelescope` impl. Returns `vec!["SetUnparkFromApPosition",
   "SetPreferredApPark", "UnparkFromApPosition"]`.
2. Implement `async fn action(&self, action_name: &str, parameters: &str)
   -> ASCOMResult<String>` with a `match` on action name dispatching
   to the three handlers. Unknown names return
   `ASCOMErrorCode::ACTION_NOT_IMPLEMENTED`.
3. Each handler:
   - **`SetUnparkFromApPosition(park)`**: parse `park` parameter as
     `ApPark`. Update in-memory `MountConfig::unpark_from_ap_position`.
     Persist to config file using the same atomic-rename pattern as
     `SetPark` (commit `c8260c1` — `services/star-adventurer-gti/src/config.rs`'s
     `write_back_park_ticks` is the template).
   - **`SetPreferredApPark(park)`**: parse `park`. Reject `ap_park_0`
     with `ASCOMErrorCode::INVALID_VALUE`. Update in-memory + persist.
   - **`UnparkFromApPosition(park)`**: parse `park`. Refuses if not
     parked (parameter validation), if disconnected (`NOT_CONNECTED`),
     or if slewing (`INVALID_OPERATION`). For `ap_park_0`, semantically
     equivalent to standard `Unpark()` — just clears `AtPark`. For
     `ap_park_1..ap_park_5`, computes the park's seed ticks, calls
     `reset_mount_encoders(ra_ticks, dec_ticks)`, then clears `AtPark`.

**Tests**

- Per action: parameter parsing (valid + invalid park names), refusal
  conditions, success path.
- `UnparkFromApPosition(ap_park_0)` ≡ `Unpark()` end state.
- `UnparkFromApPosition(ap_park_3)` writes the expected encoder
  values via the mock transport and clears `AtPark`.
- `SetUnparkFromApPosition(ap_park_3)` updates in-memory config and
  the on-disk file (use `tempfile` for the config path).
- `SupportedActions` returns the three names.

## Phase E — `Park()` uses `preferred_ap_park`

**Files**

- `services/star-adventurer-gti/src/mount_device.rs`

**Changes**

`Park()` currently slews to the in-memory `park_ra_ticks` /
`park_dec_ticks`. Change the resolution order:

1. If `park_ra_ticks` AND `park_dec_ticks` are both set in config
   (raw-encoder override), use them. Today's behaviour for backwards
   compatibility within the codebase.
2. Otherwise, compute target ticks from `preferred_ap_park` using
   the `ApPark::codebase_*` helpers + current site latitude.
3. (Falls back to live snapshot only if neither path produces a
   target — degenerate case for `preferred_ap_park = ap_park_0`
   which should already be rejected at config-deserialize time, but
   keep the fallback for defense.)

**Tests**

- Existing park tests pass against the new resolution order (those
  that explicitly set `park_ra_ticks` keep working — they hit path 1).
- New test: config with only `preferred_ap_park = ap_park_3` → `Park()`
  slews to the Park 3 encoder pair.

## Phase F — Documentation polish + BDD scenarios

**Files**

- `docs/services/star-adventurer-gti.md` (already done in the design
  pass; verify cross-references are accurate after the rename lands)
- `services/star-adventurer-gti/tests/features/` (new feature file)
- `services/star-adventurer-gti/tests/bdd/steps/` (new step modules)

**Changes**

1. Sweep `mount_device.rs` doc comments for stale `home_pose`
   references; rename to `unpark_from_ap_position`.
2. Update BDD feature files that reference `home_pose` config.
3. Add a new BDD feature `unpark_from_ap_position.feature` with
   scenarios for:
   - Fresh power-up + `ap_park_0` → no seed, encoder unchanged.
   - Fresh power-up + `ap_park_3` → encoder seeded to Park 3 values.
   - `UnparkFromApPosition(ap_park_3)` from any encoder state →
     reset-then-seed sequence runs.
   - `UnparkFromApPosition(ap_park_0)` ≡ `Unpark()`.
   - `SetUnparkFromApPosition(ap_park_2)` persists and applies on
     subsequent fresh-power-up.

## Pass criterion

- `cargo rail run --profile commit -q` passes (full unit + integration
  suite).
- `cargo fmt --check` clean.
- New BDD scenarios green.
- ConformU report unchanged from the pre-change baseline (the new
  Actions are vendor extensions; ConformU's standard test set doesn't
  cover them and shouldn't regress on anything else).
- Hardware re-validation deferred to next physical session; the change
  is purely structural for in-band behaviour and shouldn't alter any
  slew or sync semantics for an operator who configures
  `unpark_from_ap_position` to match what they had as `home_pose`.

## Risks and open questions for implementation time

- **Atomic file rewrites with multiple Actions writing config.** The
  `SetPark` precedent handles a single field; two new Actions also
  write back. Need to confirm the rewrite touches only the targeted
  keys and doesn't accidentally rewrite the whole file (which would
  clobber operator-edited fields the driver doesn't know about). The
  `write_back_park_ticks` pattern reads-modifies-writes the parsed
  JSON; that should work for any field if generalised carefully.
- **`Action` parameter encoding.** ASCOM `Action` parameters are
  passed as a single `&str`. Standard practice for vendor actions is
  either a single token (e.g. `"ap_park_3"`) or a structured payload
  (JSON, comma-separated, etc.). Single-token is simpler and covers
  all three actions here.
- **Backwards compatibility within the *codebase*.** Existing tests,
  BDD scenarios, log messages, and doc references all need a sweep.
  The compiler catches the type-level renames; tests catch the
  serde-rename; doc / log / scenario sweeps need explicit grep
  passes.
- **Confirmation gate** for the `Unpark()`-after-fresh-seed danger:
  deferred per the design. If a hardware session demonstrates the
  failure mode (operator power-cycled after a crash without
  re-parking, then unparked and slewed into something), we'll need to
  revisit and likely add a `require_unpark_confirmation: bool` config
  flag plus a `ConfirmHomePoseObserved` Action.
