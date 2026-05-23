# ADR-006: Typed Physical Quantities for the Star Adventurer GTi Pointing Math

## Status

Proposed (2026-05-23).

Origin: the review thread on the issue #259 tracking-guard work. Wiring
`MountConfig::validate()` (which closed a pre-existing gap where
`FlipPolicy::validate()` was dead code) surfaced the deeper question —
every physical quantity in the driver is a bare `f64`, so units and
reference frames are enforced only by field-name suffixes, doc comments,
and runtime validation, never by the type system. This ADR proposes the
type-level fix and sequences it as a **separate initiative** from the
shipped #259 work, which stays self-contained.

## Context

The Star Adventurer GTi driver does a lot of pointing arithmetic, and
all of it runs on bare primitives:

- `coordinates.rs` exposes 20 public functions, nearly all taking or
  returning bare `f64` "hours" or "degrees" (53 `f64` mentions in the
  file). `mech_ha` alone appears ~196 times across `src/` as a bare
  `f64`.
- Config carries the same shape: `flip_range_hours`,
  `binding_zone_min_hours`/`binding_zone_max_hours`,
  `tracking_guard_margin_hours`, `dec_min_degrees`/`dec_max_degrees`,
  `site_latitude_deg`/`site_longitude_deg` are all `f64`.
- There are **no newtypes anywhere in the workspace** (a `grep` for
  `struct X(f64|i32|u32)` across `crates/` and `services/` returns
  nothing) and **no units crate** (`uom`/`dimensioned`). This ADR would
  set the first such convention.

### The bug class this is meant to prevent

Two things masquerade as "hours" but are different physical frames:

- **Mechanical HA** (`mech_HA`) — the encoder's view of where the polar
  axis points, folded to `[−12, +12)`. This is what the CW exclusion
  zone, the slew-path checks, and the new tracking guard reason about.
- **Celestial HA** (`HA = LST − RA`), plus the related `RA`, `LST`, and
  `Dec` quantities.

Because they're all `f64`, the compiler accepts any mix of them. Today
the code is correct, but the correctness is unchecked. Two adjacent
lines in `coordinates.rs` make the hazard concrete:

```rust
// mechanical_ha_to_ra: LST minus *mechanical* HA  (frame: mechanical)
(lst_hours - mech_ha).rem_euclid(24.0)
// ra_to_mechanical_ha: LST minus RA = celestial HA (frame: celestial)
fold_to_signed((lst_hours - ra_hours).rem_euclid(24.0), 24.0)
```

Both are `f64 - f64`. Swap `mech_ha` and `ra_hours`, or feed a celestial
HA into a function expecting `mech_HA`, and it compiles and runs — and on
this mount a frame error in the wrong place drives the counterweights
toward the tripod. The flip math (`mech_ha + 12.0`) and the
ticks↔hours↔degrees conversions (`* 24.0 / cpr`, `/ 15.0`) are the same
story: unit- and frame-laden operations expressed as untyped float
arithmetic.

This is exactly the failure mode this driver already takes seriously
mechanically (issue #252's zone widening, #259's tracking guard); the
type system can make a whole category of it unrepresentable.

### Goals

1. Make unit errors (hours vs degrees vs ticks) and frame errors
   (mechanical vs celestial HA) **compile errors**.
2. Move config validation from a runtime `validate()` call to
   **construct-time / deserialize-time** invariants (parse-don't-validate).
3. Encode the domain conversions (fold-to-`[−12,12)`, `HA→mech` `+12`
   fold, tick↔angle) as **named, type-checked operations** rather than
   inline float math.

### Non-goals

- Rewriting other services. They use bare `f64` too, but the bug class
  bites hardest in this mount's pointing math; workspace-wide adoption is
  a later, separate question.
- Adopting a general dimensional-analysis framework (`uom`) — see
  Options.
- Changing any observable behaviour. This is a representation change with
  the existing tests as the oracle.

## Options Considered

### Option 1 — Status quo + runtime `validate()` (do nothing more)

Keep bare `f64`; rely on the `MountConfig::validate()` just added and on
test coverage. **Rejected** as the end state: it leaves the frame bug
class entirely unguarded (validation checks ranges, not units/frames),
and it's the situation that prompted the question. Retained only as the
fallback if the migration is judged too risky.

### Option 2 — Config-boundary newtypes only ("Scope A")

Newtype just the config fields with validating constructors + serde
`try_from`, unwrapping to `f64` for all math. **Good but insufficient
alone**: it delivers parse-don't-validate for config (a real win, and it
retires `validate()`), but the moment a value enters `coordinates.rs` it
becomes an untyped `f64` again, so the frame bug class survives. Adopted
here as **a phase of**, not an alternative to, the full change.

### Option 3 — Semantic domain types across the pointing math (CHOSEN)

Introduce a small set of types that carry both unit and frame, threaded
through `coordinates.rs`, `slew.rs`, `inherent.rs`, the tracking guard,
and config. Distinct types for `MechHa` vs `HourAngle` (celestial) vs
`Ra` vs `Lst` make the frame errors unrepresentable; `Degrees`,
`Hours`, `Ticks`/`Cpr` carry units. See [Decision](#decision).

### Option 4 — A dimensional-analysis crate (`uom`)

`uom` gives compile-time unit checking for SI-style quantities.
**Rejected**: heavyweight generic machinery and serde-integration
friction for what is, here, a handful of bespoke astronomical frames
(`uom` models units, not "mechanical vs celestial hour angle"). The
frame distinction — the part that actually bites us — isn't something
`uom` expresses. It would also be the workspace's first big external
type dependency for marginal benefit.

### Option 5 — Phantom-typed `Angle<Unit, Frame>`

One generic `Angle<U, F>` with `PhantomData` markers for unit and frame.
**Elegant, deferred**: it minimises the number of concrete types but
pushes complexity into the type signatures and the conversion impls, and
it's harder to give each frame its own domain methods
(`Lst::hour_angle_of(ra)`). Start with concrete newtypes (Option 3); a
later consolidation to a phantom-typed core is possible without changing
call sites if the concrete types are kept thin.

## Decision

Adopt **Option 3**: a `units` module of concrete newtypes, migrated in
regression-safe phases, with no external units dependency.

### Type design (intent, not final API)

Unit-carrying base types:

```rust
/// Angle in hours (1 h = 15°). No frame meaning on its own.
pub struct Hours(f64);
/// Angle in degrees.
pub struct Degrees(f64);
/// Encoder counts and counts-per-revolution.
pub struct Ticks(i32);
pub struct Cpr(u32);
```

Frame-distinct hour-angle types — the part that kills the bug class:

```rust
/// Mechanical hour angle: encoder view of the polar axis, folded to
/// [-12, +12) h. The quantity the CW exclusion zone and tracking guard
/// reason about.
pub struct MechHa(Hours);
/// Celestial hour angle of a target: LST - RA, folded to [-12, +12) h.
pub struct HourAngle(Hours);
pub struct Ra(Hours);
pub struct Lst(Hours);
```

The domain conversions become named, type-checked operations — the inline
float math of today turned into methods that only accept the right frames:

```rust
impl Lst    { pub fn hour_angle_of(self, ra: Ra) -> HourAngle; }   // LST - RA, folded
impl HourAngle { pub fn to_mech(self, side: PierSide) -> MechHa; } // +12 fold on the flipped side
impl Ticks  { pub fn to_mech_ha(self, cpr: Cpr) -> MechHa; }
impl MechHa { pub fn to_ticks(self, cpr: Cpr) -> Ticks; }
impl MechHa { pub fn in_zone(self, zone: CwExclusionZone) -> bool; }
```

`fold_ha` / `fold_to_signed` / `rem_euclid(24.0)` collapse into a single
`Hours::fold_ha()` (and the fold becomes idempotent-by-construction for
the frame types). `derive_more` (already a dependency) supplies
`Display`, `From`, and the arithmetic boilerplate where it's safe; the
`feature` list grows from `["debug"]` to include `from`/`into`/`display`
(and `add`/`sub` only where unrestricted arithmetic is actually correct
— notably **not** across frames).

### Config newtypes subsume `validate()`

Config fields become validating newtypes; serde validates on
deserialize, so an out-of-range value fails at `serde_json::from_str`
with the field name attached — strictly better than the hand-rolled
`MountConfig::validate()`, which is then retired:

```rust
#[derive(Serialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct FlipRangeHours(Hours);

impl TryFrom<f64> for FlipRangeHours {
    type Error = String;
    fn try_from(v: f64) -> Result<Self, String> { /* (0, 0.95] */ }
}
```

The CW zone — two fields with a cross-field `min ≥ max = disabled` rule —
becomes one `CwExclusionZone` type (`Active { min, max } | Disabled`),
so the "disabled" sentinel is modelled rather than conventional.

### No external units dependency

Use `derive_more` only. Reject `uom` (Option 4).

## Migration plan (regression-safe, phased; one commit per stage in a single PR)

`coordinates.rs` is hardware-validated (Phase 4/6, lat 32.7°N) and
safety-critical. The migration is structured so the existing tests are
the oracle at every step and the suite stays green throughout. The whole
refactor lands as **one PR with one commit per stage** below — each
commit keeps the suite green, so the PR can be reviewed (and bisected)
stage by stage rather than split across multiple PRs.

- **Phase 1 — `src/units.rs` + tests, no production change.** Land the
  types with exhaustive unit tests and property tests (tick↔angle
  round-trips, `fold_ha` idempotence, `HA→mech→HA` round-trips). Pure
  addition; nothing calls them yet.
- **Phase 2 — migrate `coordinates.rs` internals.** Convert function
  bodies to the typed operations while keeping the existing public `f64`
  signatures as thin wrappers (wrap on entry, unwrap on exit). The
  current `coordinates::tests` pin behaviour and must stay green
  unchanged — they are the regression net.
- **Phase 3 — propagate types outward + config.** Flip the public
  signatures to typed; migrate `slew.rs`, `tracking_guard.rs`,
  `inherent.rs`, and the `MountConfig` fields (the Scope A newtypes);
  retire `MountConfig::validate()` in favour of construct-time
  validation. The ASCOM boundary (`ascom-alpaca`'s `f64` RA/Dec API) and
  the wire codec stay `f64`; the device layer wraps/unwraps there, and
  that boundary is documented.
- **Phase 4 — remove transitional shims.** Delete the f64 wrappers; the
  pointing API is fully typed.

BDD scenarios and ConformU are unchanged and serve as end-to-end
regression checks across all phases.

## Consequences

- **Eliminates the frame bug class** in pointing math and makes config
  invalid-states unrepresentable — the two things this initiative exists
  for.
- **Workspace precedent.** First newtypes in the repo. The `units`
  module is written to be reusable, but adoption is scoped to
  `star-adventurer-gti` here; other services are untouched. A future ADR
  can promote it workspace-wide if it earns its keep.
- **Friction at boundaries.** Every f64 site gains a wrap/unwrap or a
  method call, and three boundaries stay `f64` by necessity — serde
  input, the Sky-Watcher wire codec, and the `ascom-alpaca` trait API
  (RA in hours, Dec in degrees). These are localised and documented;
  the interior is typed.
- **Migration risk is the main cost** — it rewrites validated pointing
  code. Mitigated by the phased plan, the existing test oracle, and
  keeping each stage a self-contained, green commit that is
  independently reviewable within the one PR.
- **No new external dependency.** `derive_more` feature flags grow; no
  `uom`.
- **`#259` is unaffected.** The shipped tracking guard and
  `MountConfig::validate()` continue to work; Phase 3 later swaps
  `validate()` for typed construction without changing behaviour.

## Open questions

1. **Frame granularity.** Start with concrete `MechHa`/`HourAngle`/
   `Ra`/`Lst` (this ADR) vs a phantom-typed `Angle<Unit, Frame>`
   (Option 5). Proposal: concrete first, revisit consolidation after
   Phase 3 shows the real conversion surface.
2. **Land Scope A first?** The config newtypes are low-risk and
   immediately retire `validate()`. Option to ship them as Phase 1.5
   (before the `coordinates.rs` migration) for an early win. Recommended.
3. **Workspace-wide adoption.** Out of scope here; revisit once the
   `units` module has proven itself in this service.

## References

- Issue #259 review thread — the validation work that surfaced this.
- `services/star-adventurer-gti/src/coordinates.rs` — the math being
  typed (the `lst - mech_ha` vs `lst - ra` frame hazard).
- `services/star-adventurer-gti/src/config.rs` — `MountConfig::validate`
  / `FlipPolicy::validate` (the runtime checks construct-time validation
  would subsume).
- `docs/services/star-adventurer-gti.md` §"Safety envelope" /
  §"Tracking-time safety guard" — why frame correctness is a hardware
  concern on this mount.
- `docs/decisions/004-testing-strategy-for-http-client-error-paths.md` —
  prior workspace-convention ADR; format reference.
