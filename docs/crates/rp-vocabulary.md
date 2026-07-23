# `rp-vocabulary` Crate Design

**Status:** proposed — designed on `feature/rp-targets-p1`, built in the
same PR; the decision record is
[ADR-019](../decisions/019-plan-data-vocabulary-and-validation.md) and the
plan entry is
[Decision 12](../plans/planetarium-target-import.md). This document is the
design detail behind that decision; the Rule-2 updates it triggers in
[`rp-targets.md`](rp-targets.md), [`rp-ephemeris.md`](rp-ephemeris.md),
`rp-catalog` (no dedicated doc), and
[`docs/services/rp.md`](../services/rp.md) land with the code.

A tiny, dependency-light leaf crate holding the **shared, validated
vocabulary** of `rp`'s imaging plan: the small domain value types —
`IcrsCoord`, `Binning`, `FrameType`, `Exposure` — that `rp`, the crates
it is built from (`rp-targets`, `rp-ephemeris`), and every surface that
talks to `rp` about plans must agree on. Each type is
*parse-don't-validate*: a value that exists is valid by construction, and
that one constructor is the single validator every surface shares.

This is a workspace library, not a service, and it holds **no logic** —
no store, no template engine, no ephemeris math, no protocol endpoints.
It holds *nouns*: the validated representations those layers exchange. It
is the plan-side analogue of what
[`rusty-photon-config`](../../crates/rusty-photon-config) is for driver
config — a shared contract crate, not a grab-bag of utilities.

## Why this crate exists

Two forces produced it, both surfaced while reviewing the P1 target
store.

**1. Validation was drifting across surfaces.** The same rule was written
more than once and — worse — skipped in places:

- ICRS coordinate bounds (`ra_hours ∈ [0,24)`, `dec_degrees ∈ [-90,90]`)
  existed twice in agreement (`rp::planner::primitives::validate_icrs`,
  and inline in `rp::planner::decision::parse_targets_from_value`) and
  were **missing entirely** on the store-write path (`add_target` /
  `update_target` accepted raw `f64`), so a store-backed target could
  hold coordinates the legacy config-array path rejects.
- `Binning`'s round-trip was split across three modules in two crates:
  `Display` in `rp-targets`, the `"AxB"` parse (`parse_binning`) in
  `rp::planner::goal_wire`, and a config→planner import reaching *up* into
  the planner from `rp::config::naming_template` to borrow it.
- Exposure `Duration` had two independent string encodings — humantime
  `"300s"` (`goal_wire`) written to the store as humantime-canonical
  `"5m"` by `AcquisitionGoal`'s `humantime_serde`, versus the
  filename-token `"300sec"` — with no single type owning either.

The fix is not "call the validator from more places" (that only defers
the next omission); it is to **make the invalid state unrepresentable**,
so a validator can no longer be forgotten. That requires the validated
value to be a *type*, and the type needs a home every layer can depend
on.

**2. The plan API is becoming a published, multi-surface contract.**
`rp` is ~20 % of the eventual system. Grading, mosaic, and other tools —
plus more than one UI — will all read and write plans. The way
rusty-photon already solves "one validation, many surfaces" is the
`config.get`/`config.schema`/`config.apply` protocol in
`rusty-photon-config`: one type, a schema *generated from* that type
(`schema_for`), a `validate()` returning dotted-path field errors the UI
renders inline, and one authoritative server-side gate. Extending that
pattern to plan data (targets, goals, naming patterns — see
[Schema + validate protocol](#schema--validate-protocol-the-decision-1-role))
needs a crate that owns the *typed vocabulary + its schema + its
constructor validation*. That is this crate.

## Scope

In scope — validated domain value types and nothing else:

- **`IcrsCoord`** — a J2000/ICRS pointing, `try_new`-validated to
  `ra_hours ∈ [0,24)`, `dec_degrees ∈ [-90,90]`.
- **`Binning`** — the `(x, y)` frame-binning pair; `Display` as `"2x2"`,
  `FromStr` parsing `"AxB"`. The `(filter, binning, exposure)` quota-key
  dimension.
- **`FrameType`** — `Light | Dark | Flat | Bias`, the capture intent;
  `Display`/`FromStr`, plus `calibration_slug()` (the reserved
  `dark`/`flat`/`bias` bucket slug).
- **`Exposure`** — a per-frame duration newtype that **owns both** of its
  serializations: the humantime value/wire form (`"300s"`) and the
  whole-second filename token (`"300sec"`), so the two can never disagree
  by accident again.

Each type derives `Serialize`/`Deserialize` (validating on the way in),
and — behind the [`schema` feature](#feature-flags) — `JsonSchema` with
its constraint encoded, so the generated schema is a true projection of
the validator.

Out of scope — owned elsewhere, listed so the boundary is explicit:

- **The target store** ([`rp-targets`](rp-targets.md)) — redb, CRUD,
  migration. It *depends on* this crate for `Binning`/`FrameType`/
  `IcrsCoord`/`Exposure`; it is not folded in here.
- **Ephemeris math** ([`rp-ephemeris`](rp-ephemeris.md)) — `alt_az`,
  sidereal time, twilight. It depends on this crate for the `IcrsCoord`
  *value* and keeps the transforms *on* it.
- **The file-naming template engine** (`rp::config::naming_template`:
  `CompiledTemplate`, `TOKENS`, `parse_segments`, `check_unambiguous`) —
  stays in `rp`. Only the grammar's *value types* live here. See
  [Naming: the deferred slice](#naming-the-deferred-slice).
- **The schema + validate protocol endpoints** and the dotted
  `FieldError` mapping — live in `rp` (parameterized by these types).
- **ASCOM driver quantities** — camera `BinX`/`BinY`, mount
  `RightAscension`/`Declination`, the shutter `light: bool`. These are a
  *false cognate*, not this vocabulary. See
  [Not this crate: driver quantities](#not-this-crate-driver-quantities-false-cognates).
- **ADR-006 mount-local typed quantities** (`MechHa`/`Ra`/`Dec`, encoder
  ticks) — frame-safe *pointing math* inside the mount driver, a
  different concern from plan-data values. See
  [Relationship to ADR-006](#relationship-to-adr-006-and-the-bare-decimals-decision).

## Crate boundary and dependency graph

`rp-vocabulary` is a leaf with **zero first-party dependencies**, so no
dependency cycle is structurally possible. Both `rp-targets` and
`rp-ephemeris` — today independent sibling leaves — gain an edge *to* it,
never to each other.

```
                    rp-vocabulary  (leaf: serde, thiserror, [schema] schemars)
                    IcrsCoord::try_new   Binning: FromStr   FrameType   Exposure
                        ▲          ▲            ▲
        ┌───────────────┘          │            └───────────────┐
   rp-ephemeris              rp-targets                     rp-catalog
   (keeps erfars/tzf-rs;     (keeps redb; Target,           (ResolvedTarget
    transforms take          AcquisitionGoal hold            .coord: IcrsCoord;
    vocab::IcrsCoord)        vocab types)                    depends on vocab)
        ▲                          ▲                             ▲
        └──────────────────────────┼─────────────────────────────┘
                              services/rp
        Target & PlannerTarget hold IcrsCoord; depends on rp-vocabulary
        with `features = ["schema"]`; owns the naming engine + the
        schema/validate protocol endpoints.
```

The reason the value must live *below* both consumers is
[decision 3](#the-newtype-field-migration-decision-3): for `Target`
(in `rp-targets`) **and** `PlannerTarget` (in `rp`) to hold a validated
`IcrsCoord` field, the type has to sit under both — and putting it in
`rp-ephemeris` instead would force `rp-targets → rp-ephemeris`, dragging
the `erfars` C library and the `tzf-rs` timezone database into a plain
data store. A dependency-light vocabulary leaf avoids that entirely.

## The types

### `IcrsCoord` — validated pointing

```rust
/// A J2000 mean equator/equinox (ICRS) pointing, valid by construction.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(try_from = "IcrsCoordWire", into = "IcrsCoordWire")]
pub struct IcrsCoord {
    ra_hours: f64,     // private: the only way to a value is try_new
    dec_degrees: f64,
}

impl IcrsCoord {
    /// `ra_hours ∈ [0, 24)`, `dec_degrees ∈ [-90, 90]`.
    pub fn try_new(ra_hours: f64, dec_degrees: f64) -> Result<Self, CoordError>;
    pub fn ra_hours(&self) -> f64;
    pub fn dec_degrees(&self) -> f64;
}

pub enum CoordError { RaOutOfRange { ra_hours: f64 }, DecOutOfRange { dec_degrees: f64 } }
```

Fields are **private** — that is what makes the invalid state
unrepresentable (as `TargetSlug` already does with its inner `String`).
The wire/on-disk form is unchanged: `#[serde(try_from = "IcrsCoordWire")]`
where `IcrsCoordWire { ra_hours, dec_degrees }` is the flat two-key shape,
and the `TryFrom` runs `try_new` — so deserializing a bad row or bad JSON
*fails* rather than smuggling an out-of-range value past the constructor.
Read sites migrate from `.ra_hours` to `.ra_hours()`.

### `Binning` — completed round-trip

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, /* serde, [schema] */)]
#[display("{x}x{y}")]
pub struct Binning { pub x: u8, pub y: u8 }

impl FromStr for Binning { /* "AxB" → Binning; errors on non-"AxB" or non-u8 */ }
```

`Binning`'s fields stay public: any `u8 × u8` is shape-valid, so there is
no bound to protect and no bypass to close — the fix is simply to put the
`FromStr` **next to** the `Display` it inverts, retiring
`goal_wire::parse_binning` and the config→planner import that borrowed it.
(The naming-token grammar `\d+x\d+` in `rp` is laxer than `u8`; that
mismatch is pre-existing and tracked as a follow-up, not fixed here.)

### `FrameType` — capture intent

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, /* serde, [schema] */)]
pub enum FrameType { Light, Dark, Flat, Bias }

impl FrameType {
    /// The reserved `{target}` bucket for a calibration frame with no
    /// explicit target: lowercased type. `None` for `Light`.
    pub fn calibration_slug(&self) -> Option<&'static str>;
}
impl Display for FrameType   { /* "Light" | "Dark" | "Flat" | "Bias" */ }
impl FromStr  for FrameType  { /* exact, case-sensitive inverse of Display */ }
```

This is where `FrameType` finally belongs. It was buried in
`rp::config::naming_template` yet consumed as a cross-cutting domain type
(the `{frame_type}` token, the `capture` MCP param, the exposure sidecar
document). Earlier analysis weighed "leave it in a top-level `rp` module"
against "push it into `rp-targets`," and rejected the latter only to keep
`schemars` out of the store leaf. The vocabulary crate dissolves that
trade-off: `FrameType` lives here with a **feature-gated** `JsonSchema`,
`rp-targets` depends on this crate *without* the `schema` feature (so it
stays schemars-free), and `rp` turns the feature on.

### `Exposure` — one type, both serializations

```rust
pub struct Exposure(std::time::Duration);

impl Exposure {
    pub fn try_new(d: Duration) -> Result<Self, ExposureError>; // rejects zero
    pub fn as_duration(&self) -> Duration;

    /// The whole-second filename token, e.g. "300sec". `Err` if the
    /// duration is not a whole number of seconds (filenames have no
    /// sub-second representation).
    pub fn to_filename_token(&self) -> Result<String, ExposureError>;
    pub fn from_filename_token(s: &str) -> Result<Self, ExposureError>;
}
// Serialize/Deserialize emit/accept the value form "300s" (seconds-exact,
// byte-stable round-trip) — NOT humantime-canonical "5m".
```

`Exposure` makes the *two* encodings explicit and co-located: the value
form used in config and store (`"300s"`), and the whole-second filename
token (`"300sec"`). Because the store is new in this PR there is no
deployed data to migrate, so we simply pick the seconds-exact value form
as canonical and the store/wire disagreement (`"5m"` vs `"300s"`) never
ships. `AcquisitionGoal.exposure` changes from a `humantime_serde`
`Duration` to `Exposure`.

## Relationship to the `rp-targets.md` "bare decimals" decision

[`rp-targets.md`](rp-targets.md#coordinates-plain-decimal-not-typed-quantities)
documents a deliberate decision to store `ra_hours`/`dec_degrees` as bare
`f64`. That is the decision this design touches; it is **consistent with
it**, and the reconciliation is worth recording so it travels with the
code.

That section leans on
[ADR-006](../decisions/006-typed-physical-quantities-for-mount-pointing.md)
for support, so it is worth being precise about what ADR-006 does and does
not say here — it is *not* a constraint this crate has to work around:

- **ADR-006 has no jurisdiction over plan coordinates, and doesn't claim
  any.** It is explicitly scoped to the Star Adventurer mount's pointing
  math (Non-goals: *"Rewriting other services … workspace-wide adoption is
  a later, separate question"*; Resolved Q3: *"Workspace-wide adoption —
  out of scope here"*). Its `MechHa`/`Ra`/`Dec` types exist to make
  mechanical-vs-celestial *frame* mix-ups compile errors inside that
  driver. `IcrsCoord` carries no frame distinction; it is a *plan-data
  value* with a range check — a different type for a different layer.
- **If anything, ADR-006 is the precedent *for* this pattern.** Its Goal 2
  is *"construct-time / deserialize-time invariants (parse-don't-validate)"*,
  and its `FlipRangeHours` config newtype is a private-field `f64` with
  `#[serde(try_from = "f64", into = "f64")]` and a range-checking `TryFrom`
  — exactly `IcrsCoord`'s shape. The `rp-targets.md` "bare decimals" choice
  was an *alignment* argument (match `ResolvedTarget`/`IcrsCoord`, which
  were bare `f64`), not a prohibition on validating newtypes.
- **The "bare decimals" goals are preserved, and alignment *improves*.**
  That decision had two aims: don't adopt the ADR-006 mount types for
  plan data (honored — `IcrsCoord` is not one), and stay aligned with
  `rp_catalog::ResolvedTarget` and `rp_ephemeris::IcrsCoord`. Today those
  are **three parallel bare-`f64` coordinate representations** that merely
  happen to match. Making `IcrsCoord` the one shared value type replaces
  three look-alikes with a single type — alignment goes *up*, not down.
- **On-disk and wire shapes are unchanged.** `IcrsCoord` serializes as
  the flat `{ra_hours, dec_degrees}` pair. The store still holds bare
  decimals on disk (the part of the decision about the *serialized* form
  is untouched); only the *in-memory* representation gains validation.

### `rp-catalog` adopts `IcrsCoord` (settled — ADR-019)

`rp_catalog::ResolvedTarget` also adopts `IcrsCoord`
(`ResolvedTarget.coord: IcrsCoord`) and `rp-catalog` gains a dependency on
`rp-vocabulary`. This is the fullest de-drift: **one** coordinate type end
to end — catalog → store → planner → ephemeris — replacing the three
parallel bare-`f64` representations that previously only *happened* to
agree. The alternative considered and rejected was bridging with a
`From`/`try_new` at the `rp` boundary (a smaller diff, but it would leave
one bare-`f64` rep in the workspace). `rp-catalog` stays a light leaf — it
gains only the `rp-vocabulary` edge, no other new dependency.

## The newtype-field migration (decision 3)

The coordinate drift is prevented structurally by changing the *field
type*, not by adding more calls to a validator:

- `rp_targets::Target` — `ra_hours: f64, dec_degrees: f64` → `coord: IcrsCoord`.
- `rp::planner::decision::PlannerTarget` — same.

With no raw-`f64` coordinate field left, every construction — today's
`add_target`/`update_target`, `parse_targets_from_value`, and any future
write path — is *forced* through `IcrsCoord::try_new` by the compiler. The
`update_target` line that did `target.ra_hours = v` no longer type-checks;
it becomes `target.coord = IcrsCoord::try_new(...)?`. Serde keeps the flat
`{ra_hours, dec_degrees}` shape (above), so neither the redb rows nor the
MCP JSON change. This is not a backwards-compatibility exercise — the
store and tools are new in this PR with no shipped caller — it is doing
the change *while it is still free*, before the wire ossifies into a
contract.

## Schema + validate protocol (the decision-1 role)

`rp-vocabulary` is the *enabler* of a plan-data analogue of the
`config-actions` protocol; it does not own the protocol. The division:

- **This crate provides**: the validated constructors (the single
  validator, returning typed `thiserror` errors) and — behind the
  `schema` feature — the `JsonSchema` derives with constraints encoded
  (`ra_hours` min/max, `Binning`/`FrameType`/`Exposure` shapes), so the
  published schema cannot drift from the validator.
- **`rp` provides**: the `schema`/`validate` endpoints for targets, goals,
  and naming patterns, and the mapping from a constructor `Err` to a
  dotted-path `FieldError { path, message }` (e.g.
  `{ path: "ra_hours", message: "must be in [0, 24)" }`) — the same shape
  `config-actions` returns, so a UI renders the error next to the field.
- **Surfaces consume**: Rust linkers (`rp`, future tools) get identical
  validation by construction; non-linking surfaces (the ui-htmx BFF, a
  browser form, a future bridge) render forms from the published schema
  and defer to `rp`'s constructor as the authoritative gate. This keeps
  ui-htmx a thin renderer (the "UI contains zero application logic" tenet)
  exactly as it already renders driver-config forms from `config.schema`.

This cross-crate decision is recorded in
[ADR-019](../decisions/019-plan-data-vocabulary-and-validation.md); this
crate doc is the design detail behind it.

## Naming: the deferred slice

The file-naming template *engine* stays in `rp` — it is session-layer glue
(regex + chrono + `SessionConfig`) with exactly one linker and none on the
horizon (ui-htmx passes patterns through as opaque strings; a future
planetarium-bridge creates targets over MCP and never renders filenames).
What *could* migrate here later is the thin reusable slice a
pattern-editor surface would want — a `NamingPattern` newtype wrapping the
token grammar and `validate_pattern` — leaving `CompiledTemplate` (render)
in `rp`. That is **not** in this PR; the boundary is staked so it can move
cleanly when a real second consumer (a pattern editor) exists. Until then,
`Binning`/`FrameType`/`Exposure` moving here already lets the engine drop
its config→planner import.

## Not this crate: driver quantities (false cognates)

The ASCOM camera/mount drivers are **not** consumers and their look-alike
quantities are **not** this vocabulary:

- Camera **binning** (`bin: u32`, `BinX`/`BinY`) is a *symmetric hardware
  factor* validated against the sensor's `supported_bins`, with
  `can_asymmetric_bin()` hardcoded `false` — a driver/ASCOM contract, `u8`
  at Alpaca and `u32` at the SDK FFI. `Binning{x,y}` is an
  asymmetric-capable *plan-data quota key* with no hardware validation.
  They share only the word.
- `start_exposure(duration, light: bool)` is a 2-way shutter boolean, not
  the 4-way `FrameType` intent — there is no Flat/Bias concept at the
  driver layer.
- Mount **pointing** (`RightAscension`/`Declination`) is received by a
  telescope driver as f64 over the ASCOM trait and immediately wrapped in
  that driver's *own* frame-safe types (ADR-006's `Ra`/`Dec`/`MechHa`/…),
  which model mechanical-safety frame distinctions `IcrsCoord` does not —
  and the driver re-validates against its mechanical envelope regardless,
  because a safety-critical device cannot trust upstream validation. The
  established mount/rotator drivers link **no** plan-domain crate (they
  even wrap `erfars` directly rather than share `rp-ephemeris`); a driver
  linking `rp-vocabulary` would invert that (driver → rp-domain) for a
  transient boundary wrapper.

No camera/mount service links `rp-vocabulary`; the translation from plan
`Binning`/`IcrsCoord` to Alpaca `BinX`/`BinY` and `RightAscension`/
`Declination` is `rp`'s mcp-client seam. This is why the crate is
`rp-*` (astro-**domain** vocabulary) and not `rusty-photon-*` (which is
reserved for domain-**neutral** plumbing every service including the
drivers links — transport, TLS, config, lifecycle, i18n). If a *third*
pointing device ever earns it (ADR-006's own trigger — the rotator and
Star Adventurer are two), the coordinate/angle sharing that belongs
*across drivers* is a neutral `rusty-photon-units` foundation their
frame-safe types build on — never `IcrsCoord` reaching down into a driver.

## Feature flags

```toml
[features]
default = []
schema  = ["dep:schemars"]   # JsonSchema derives on every type
```

`schema` is off by default so a consumer that only needs the DTOs +
validation (e.g. `rp-targets`, or a future thin client) does not inherit
`schemars`. `rp` enables it for the schema/validate protocol. This is the
one lever that keeps the store leaf as light as it is today while still
letting `rp` project the vocabulary onto the wire.

## Dependencies

| Crate | Purpose |
|---|---|
| `serde` / `serde_json` | value (de)serialization, validating on input |
| `thiserror` | the constructor error enums (`CoordError`, `ExposureError`, …) |
| `derive_more` | `Display`/`FromStr` derives (`Binning`, `FrameType`) |
| `humantime` / `humantime-serde` | `Exposure` value form |
| `schemars` *(optional, `schema`)* | `JsonSchema` for the wire projection |

No new crates.io dependency is introduced — every one is already in the
workspace. No `MODULE.bazel.lock` repin is required for the crate's deps;
adding a new workspace member still needs the standard `bazel mod tidy`
refresh per [Rule 10](../workspace.md#bazel-primary-ci-gate).

## Module layout

```
crates/rp-vocabulary/src/
├── lib.rs        # crate root + re-exports; #![deny(unsafe_code)]
├── coord.rs      # IcrsCoord, CoordError, IcrsCoordWire
├── binning.rs    # Binning + FromStr
├── frame_type.rs # FrameType + calibration_slug
└── exposure.rs   # Exposure + value/filename codecs, ExposureError
```

Crate-root attributes match the sibling crates
(`#![cfg_attr(coverage_nightly, feature(coverage_attribute))]`,
`#![deny(unsafe_code)]`).

## Testing

Each type's round-trip is pinned in-crate, and — critically — the
existing round-trip tests **move here with the code they cover**, so the
relocation cannot silently regress the render/parse contract:

- `IcrsCoord` — `try_new` accepts in-range / rejects each out-of-range
  bound; `serde` round-trips through the flat wire and *rejects* an
  out-of-range wire value on deserialize.
- `Binning` — `Display`/`FromStr` round-trip; `FromStr` rejects non-`AxB`
  and non-`u8`.
- `FrameType` — `Display`/`FromStr` round-trip every variant (the moved
  `frame_type_round_trips_every_variant` test); `calibration_slug`.
- `Exposure` — value round-trip (`"300s"` in/out, byte-stable); filename
  token round-trip; whole-second rejection for the filename form.

Tests use `.unwrap()`/`.unwrap_err()` per
[testing.md](../skills/testing.md), scoped by the usual `#[allow(...)]` on
the test module.

## In this PR vs deferred

**In this PR:** the crate and its four types with validation +
feature-gated schema; `Binning`/`FrameType`/`Exposure` moved off
`rp-targets`/`rp`; `IcrsCoord` moved off `rp-ephemeris` (which now depends
here); `rp-catalog`'s `ResolvedTarget` adopting `IcrsCoord`; the
newtype-field migration of `Target` and `PlannerTarget`; the
coordinate-validation gap on the store-write path closed by construction;
the round-trip tests moved with their types.

**Deferred:** the `NamingPattern` grammar slice (stays in `rp` until a
pattern editor exists); the full `schema`/`validate`/`apply` *endpoints*
on `rp` (the protocol machinery — this crate only supplies the typed
vocabulary they speak).

## Decision rationale (alternatives considered)

- **`rp-vocabulary` over `rp-primitives` / `rp-common-types`.**
  "Primitives" reads as *language* primitives (int/bool), underselling
  that these are validated domain value types. "Common-types" is
  semantically empty and invites the crate to become a workspace junk
  drawer — the opposite of a disciplined, published contract. "Vocabulary"
  names the *purpose* (the shared nouns many interfaces exchange to
  interoperate — the term-of-art "vocabulary types"), which is itself the
  gate against scope creep: a random helper isn't vocabulary.
- **`rp-*` over `rusty-photon-*`.** The workspace convention is
  `rusty-photon-*` for domain-*neutral* plumbing every service links
  (drivers included), `rp-*` for astrophotography-*domain* crates. These
  are domain nouns the drivers deliberately do **not** link (false
  cognates). Precedent: `rp-mcp-client` — a shared "talk to `rp`" contract
  linked by multiple *services* (`calibrator-flats`, `session-runner`) —
  is `rp-*`, so a shared plan-vocabulary linked by `rp` and future clients
  is the same category.
- **One vocabulary crate over several (`rp-primitives` + `rp-naming`).**
  Every value type has exactly one linking consumer today (`rp`); the
  naming *engine* has no second consumer on the roadmap. Minting multiple
  crate nodes now to encode a future the roadmap doesn't contain adds
  BUILD files and lockfile churn for no consumer they uniquely serve. One
  leaf, with the naming slice's boundary staked for later.
- **Consolidate into existing leaves vs a new crate at all.** A pure
  "no new crate" consolidation (put `Binning`/`Exposure` in `rp-targets`,
  `IcrsCoord` in `rp-ephemeris`) fixes today's drift but cannot give
  `Target` *and* `PlannerTarget` a shared validated coordinate field
  without an inverted `rp-targets → rp-ephemeris` (ERFA) edge — and it
  scatters the plan vocabulary that the multi-surface schema/validate
  protocol wants in one place. The leaf crate is what makes the
  newtype-field guarantee (decision 3) and the protocol (decision 1) both
  buildable.
```