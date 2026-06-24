# `rp-targets` Crate Design

A [`redb`](https://crates.io/crates/redb)-backed store for the imaging
**plan**: the operator's target list, the per-sub-spec acquisition
quotas, and the per-target overrides for grading thresholds and
scheduling constraints. Pure storage behind one mockable trait — no
filesystem scanning, no ephemeris, no policy.

This is a workspace library, not a service. The `rp` orchestrator
consumes it directly. The store holds the *plan*; the *actuals*
(how many frames exist, which are good) are derived by `rp` from the
filesystem and the per-frame sidecars — they are deliberately **not**
in this crate. See [`docs/services/rp.md`](../services/rp.md) for how the
planner and MCP tools project this store onto the external surface and
compute progress; this doc covers the crate's own design.

## Scope

In scope — the crate is a typed CRUD repository over a single `redb`
file:

- **Targets** — a named pointing with denormalized coordinates,
  priority, an active flag, and optional per-target overrides.
- **Acquisition goals** — the desired frame count per
  `(filter, binning, exposure)` sub-spec, owned by a target.
- **Override storage** — per-target grading thresholds and scheduling
  constraints (the global defaults live in `rp` config; this crate
  stores only the deltas).
- **Lookups** — get/list/delete by slug; the planner scans the small
  set and orders in Rust.
- **Schema migration** — a `schema_version` key plus serde-tolerant
  value structs, so the on-disk format can evolve.

Out of scope — owned elsewhere, called out so the boundary is explicit
(see [Crate boundary](#crate-boundary-pure-plan-repository)):

- **Progress derivation** (filename scan + sidecar grading) — `rp`'s
  planner. The crate never touches `data_directory`.
- **The file-naming template engine** (render + parse) — `rp` session
  layer. See [rp Integration](#rp-integration-outside-this-crate).
- **Ephemeris evaluation** (is the target up / is the moon too close) —
  [`rp-ephemeris`](rp-ephemeris.md), driven by `rp`'s planner.
- **The grading plugin** that measures per-frame metrics — a separate
  `rp` plugin; this crate only stores the *thresholds* its verdict is
  computed against.
- **The catalog** — [`rp-catalog`](../../crates/rp-catalog) stays the
  embedded read-only source of truth. Targets **denormalize** the
  resolved coordinates; no catalog rows are stored here.
- **Frames, sidecars, sessions, and the event log** — FITS + sidecar
  JSON files and the in-RAM event ring buffer, all unchanged.

## Crate boundary (pure plan repository)

`rp-targets` follows the workspace convention that crates are
single-purpose: `rp-ephemeris` is pure math, `rp-catalog` is pure
lookup, `rp-fits` is pure I/O. `rp-targets` is pure plan storage.

The division of labour with the consumer:

```
                    ┌──────────────────────── rp (services/rp) ───────────────────────┐
                    │  planner/decision.rs                                            │
   rp-catalog ──────┼─►  resolve name → coords (at add-time)                          │
                    │       │                                                         │
                    │       ▼                                                         │
  ┌─ rp-targets ─┐  │   TargetStore  (this crate: stored plan)                        │
  │ TargetStore  │◄─┼───  upsert / get / list / delete / set-goals                    │
  │ Redb impl    │  │       │                                                         │
  └──────────────┘  │       ▼                                                         │
                    │   compose with:                                                 │
   rp-ephemeris ────┼─►   alt/az, moon separation, meridian  (eligibility)            │
   filesystem ──────┼─►   scan <data_directory> → total; read sidecars → good/rejected│
                    │       │                                                         │
                    │       ▼   "pick next target" / "progress for target X"          │
                    └─────────────────────────────────────────────────────────────────┘
```

The crate is testable in isolation (no filesystem, no clock, no
network), and a mock `TargetStore` lets `rp`'s planner be tested without
a real database.

## Data Model

Three small types. Acquisition goals are **embedded** in the target
value rather than living in a second table: a target and its handful of
goals are always read and written together, the set is bounded
(single-digit rows per target), and embedding makes "load a target with
its quotas" a single atomic value read. The whole store is a few tens of
targets, so there are no secondary indexes — listing scans and sorts in
Rust.

```rust
/// A planned pointing plus its acquisition goals.
pub struct Target {
    /// Immutable identity and on-disk/filename token (e.g. "m33",
    /// "ngc7000-east", "comet-12p"). Lower-cased, filename-safe.
    pub slug: TargetSlug,
    /// Operator-facing name; freely editable without breaking the
    /// identity or existing on-disk frames (e.g. "M33 — Triangulum").
    pub display_name: String,

    // --- Pointing (denormalized; plain decimal ICRS, see note below) ---
    pub ra_hours: f64,
    pub dec_degrees: f64,

    // --- Catalog provenance (None for non-catalog targets) ---
    /// Canonical catalog name this was resolved from, e.g. "NGC 224".
    pub catalog_ref: Option<String>,
    /// Denormalized at add-time from `rp_catalog::ResolvedTarget`.
    pub object_type: Option<String>,
    pub magnitude: Option<f64>,
    pub size_arcmin: Option<f64>,

    // --- Planning ---
    pub priority: i32,
    pub active: bool,
    pub goals: Vec<AcquisitionGoal>,

    // --- Per-target overrides (None ⇒ use rp-config global default) ---
    pub scheduling: Option<SchedulingConstraints>,
    pub grading: Option<GradingThresholds>,

    pub notes: Option<String>,
    /// RFC3339; set by rp at the call boundary (the crate takes the
    /// timestamp as a parameter — it does not read the clock).
    pub created_at: String,
    pub updated_at: String,
}

/// Desired frame count for one acquisition sub-spec. The
/// `(filter, binning, exposure)` triple is exactly the quota key from
/// the filename scheme (frame type is always Light for goals; gain is
/// not a sub-spec dimension — it is a fixed per-setup camera setting).
pub struct AcquisitionGoal {
    pub filter: String,          // "Ha", "L", "R", ...
    pub binning: Binning,        // renders as "1x1"
    #[serde(with = "humantime_serde")]
    pub exposure: std::time::Duration,
    pub desired_count: u32,
}

pub struct Binning { pub x: u8, pub y: u8 }

/// Per-target scheduling constraints. Each `None` field falls back to
/// the rp-config global default. *Stored here; evaluated by rp's
/// planner via rp-ephemeris.*
pub struct SchedulingConstraints {
    pub min_altitude_degrees: Option<f64>,
    pub min_moon_separation_degrees: Option<f64>,
    pub max_moon_illumination_fraction: Option<f64>,
    /// Max |hour angle| from the meridian, in hours, the target may be
    /// imaged at (e.g. 2.0 ⇒ within ±2 h of transit). None ⇒ no window.
    pub meridian_window_hours: Option<f64>,
}

/// Per-target grading thresholds. The grading plugin owns the *meaning*
/// of these; this crate only stores the overriding values. Each `None`
/// falls back to the rp-config global default.
pub struct GradingThresholds {
    pub max_hfr_pixels: Option<f64>,
    pub min_star_count: Option<u32>,
    pub max_eccentricity: Option<f64>,
    pub min_snr: Option<f64>,
}
```

### Identity: the slug

`TargetSlug` is a parse-don't-validate newtype (see
[development-workflow.md](../skills/development-workflow.md#parse-dont-validate-for-config)):
constructed via `TargetSlug::new(&str)`, it lower-cases, **strips all
whitespace** (mirroring `rp-catalog`'s name normalization, so
`"NGC 7000"` → `ngc7000`), and rejects anything still outside `[a-z0-9-]`
(so it is always a safe directory and filename token). The slug is **immutable** once created — it is the
on-disk acquisition identity (the `{target}` token in every frame's path
and name), so changing it would orphan existing frames. Renames change
`display_name`, never the slug. Slug collisions on add are the caller's
(`rp`'s) responsibility to resolve before `upsert` (see
[Slug allocation](#slug-allocation-add-time) — e.g. `ngc7000` →
`ngc7000-2`); `upsert` of an existing slug is an in-place update, never
a silent second row.

This mirrors `rp-catalog`, which already keys objects by a normalized
name rather than a surrogate id. UUIDs in this codebase identify
*transient operational artifacts* (exposure documents, operations,
events); a target is a durable plan entity, so it is name-keyed.

### Coordinates: plain decimal, not typed quantities

`ra_hours` / `dec_degrees` are bare `f64` in decimal ICRS, matching
`rp_catalog::ResolvedTarget` and `rp_ephemeris::IcrsCoord`. Per
[ADR-006](../decisions/006-typed-physical-quantities-for-mount-pointing.md),
the typed-quantity newtypes (`MechHa`/`Ra`/`Dec`, encoder ticks) are
**mount-local** — they exist to make frame/unit mix-ups in pointing math
into compile errors. A target row is plan data, not pointing math; it
flows straight into `IcrsCoord` when the planner needs a position. Bare
decimals here keep the store aligned with the catalog it is populated
from.

## The `TargetStore` trait (the seam)

One async trait — the single seam between the plan store and `rp`.
`redb` is a synchronous engine; the shipped impl does each operation's
work on the Tokio blocking pool (`spawn_blocking`), exactly as
`rp-fits`/`persistence::document` already wrap blocking sidecar I/O. The
async surface keeps the consumer ergonomic inside `rp`'s async planner,
and a mock impl needs no blocking pool at all.

```text
upsert_target(target)            -> Result<(), TargetStoreError>
get_target(slug)                 -> Result<Option<Target>, TargetStoreError>
list_targets()                   -> Result<Vec<Target>, TargetStoreError>
delete_target(slug)              -> Result<bool, TargetStoreError>   // false = absent
set_goals(slug, Vec<AcquisitionGoal>) -> Result<(), TargetStoreError> // replace the set
```

`list_targets` returns every row; the planner filters (`active`) and
orders (`priority`, then least-progress) in Rust — the row count is tens,
so a scan-and-sort is cheaper and simpler than maintaining an index.
There is intentionally **no** `record_exposure`/counter-mutation method:
actuals are derived from the filesystem, never written here
(see [rp Integration](#rp-integration-outside-this-crate)).

**Return contract.** `get_target` and `list_targets` return
fully-populated `Target` values *including* their embedded `goals`, so a
single `list_targets` call answers all-target progress with no N+1 fetch.
`list_targets` is sorted by slug (deterministic; the planner re-sorts by
its own policy). `delete_target` returns `false` for an absent slug;
`set_goals` on an absent slug returns `TargetStoreError::NotFound`, and
rejects a goal set that contains duplicate `(filter, binning, exposure)`
keys or a zero `desired_count`/`exposure`.

**Upsert precedence.** `upsert_target` writes the whole value (including
`goals`) atomically. On upsert of an existing slug the stored
`created_at` is preserved (the impl reads the prior row and keeps its
`created_at`); `updated_at`, `display_name`, coordinates, overrides, and
`goals` take the supplied values. `set_goals` is the goals-only fast path
(it leaves the rest of the row untouched); `upsert_target` and
`set_goals` are the only writers of `goals`.

Errors are a `thiserror` enum:

```rust
pub enum TargetStoreError {
    Open(redb::DatabaseError),
    Txn(redb::TransactionError),
    Table(redb::TableError),
    Storage(redb::StorageError),
    Commit(redb::CommitError),
    Encode(serde_json::Error),
    /// The redb file-format generation is older than this build's redb;
    /// run the documented one-time `Database::upgrade()`.
    RedbUpgradeRequired,
    /// On-disk schema_version is newer than this build understands.
    UnsupportedSchemaVersion { found: u32, supported: u32 },
    /// A goals-only operation referenced a slug with no stored target.
    NotFound { slug: String },
}
```

## `RedbTargetStore` (raw-redb implementation)

### On-disk layout

Two tables in one `redb` database file:

```text
targets : TableDefinition<&str, &[u8]>   // slug → serde_json(Target)
meta    : TableDefinition<&str, &[u8]>   // "schema_version" → u32 (LE bytes)
```

Values are encoded with `serde_json`. JSON (vs a compact binary codec
like `postcard`) is deliberate: it is already a workspace dependency, the
data volume is trivial, and the values stay dumpable/inspectable — a
`rp targets export` style tool is a plain read + `to_writer`. The tiny
size cost is irrelevant at tens of targets.

A single `redb::Database` is opened once at `rp` startup, wrapped in an
`Arc`, and shared. Each trait method clones the `Arc` into a
`spawn_blocking` closure that runs one `redb` transaction:

- **Reads** (`get`/`list`) use `begin_read()` → `open_table` → `get`/range.
- **Writes** (`upsert`/`delete`/`set_goals`) use `begin_write()` →
  `open_table` → `insert`/`remove` → `commit()`.

`redb` is fully ACID — a write is durably committed (with `fsync`) or not
at all, and a crash mid-commit leaves the previous committed state
intact. This **matches the crash-safety bar** the rest of the system
holds via atomic-write-by-rename + `fsync` (the `rp-fits` atomic helper
and rp's `persistence::document` sidecar writes), so the plan store needs
no extra durability machinery of its own.

`redb` guarantees backward compatibility *within* an on-disk file-format
generation, but the generation has changed across major releases
(v1→v2→v3): opening an older file with a newer `redb` returns
`DatabaseError::UpgradeRequired` and needs a one-time
`Database::upgrade()`. This is **distinct from** the crate's own
`schema_version` (which versions the *value* shape, not redb's file
layout). `redb` is therefore pinned to a known major in
`crates/rp-targets/Cargo.toml`, and `RedbTargetStore::open` surfaces a
redb-format bump as a dedicated `TargetStoreError::RedbUpgradeRequired`
rather than burying it in the opaque `Open` variant — so a format upgrade
is an explicit, logged step, not a silent failure.

### Schema migration

On open, read `meta["schema_version"]`:

- **absent** → fresh database; write `CURRENT_SCHEMA_VERSION`.
- **== current** → proceed.
- **< current** → run the ordered migration steps `vN → vN+1` inside a
  single write transaction, then bump the version. Additive,
  non-breaking field changes need no step at all: value structs
  `#[serde(default)]` their new fields and tolerate unknown ones, so an
  old value deserializes into the new `Target` directly. A migration
  step is only authored for a breaking re-shape (rename, split, type
  change), as a `Target_vN → Target` transform.
- **> current** → `UnsupportedSchemaVersion` (refuse to run against a
  database written by a newer build, rather than silently dropping
  fields).

### File location

Configurable via `targets.db_path`, defaulting to
`<session.data_directory>/targets.redb` so the plan travels with the
frames it describes and a single directory copy backs up both. Backup is
"copy the one file" — `redb` is a single-file store.

## rp Integration (outside this crate)

Everything in this section lives in `services/rp`, not in `rp-targets`.
It is documented here because it is the context that makes the crate
useful, and because it is what `docs/services/rp.md` must absorb in the
matching Rule-2 update. The authoritative home for these contracts is
`rp.md`; this is the summary.

### Slug allocation (add-time)

`rp` derives and resolves the slug before calling `upsert_target`:

1. Base = `TargetSlug::new(catalog_ref.unwrap_or(display_name))` (a
   catalog add bases on `"NGC 7000"` → `ngc7000`; a custom add bases on
   the operator's name).
2. Probe `get_target(base)`. **Absent** → use `base`.
3. **Present and the same object** (same `catalog_ref`, or coordinates
   within a small tolerance) → treat as an in-place edit: reuse the slug
   and `upsert` (the rename / re-add path).
4. **Present and a different object** → allocate the lowest unused
   `"{base}-{n}"` for `n` from 2 (`ngc7000-2`, `ngc7000-3`, …), taking
   the first free suffix. By the pigeonhole principle a free suffix is
   guaranteed within `list_targets().len() + 1` probes, so the search
   always terminates — no arbitrary cap or exhaustion error is needed.

Contract: adding NGC 7000 twice with different framing yields `ngc7000`
and `ngc7000-2`; re-adding the same object updates it in place. This is
rp policy — the crate only enforces that `upsert` of an existing slug is
an in-place overwrite, never a duplicate row.

### File-naming template (render + parse)

`rp` turns the reserved `session.file_naming_pattern` (rp.md:285-287,
example at rp.md:2990) from a render-only field into a **round-trippable**
template, plus a new `session.directory_pattern`. This **supersedes** the
originally-reserved token set (a breaking redefinition, not an
extension): `{duration}`→`{exposure}` and `{sequence}`→`{frame_number}`,
and the `:04`-style width specifier in the rp.md:2990 example is dropped
in favour of fixed-width rendering per token (below). The Rule-2 rp.md
update must edit rp.md:285-287 and rp.md:2990 to match; for backward
compatibility the parser accepts `{duration}` and `{sequence}` as
deprecated aliases of `{exposure}` and `{frame_number}`. Tokens use the
`{token}` brace syntax. The default reproduces the agreed scheme:

```
directory_pattern    = "{target}/{night_date}/{frame_type}"
file_naming_pattern  = "{target}_{filter}_{binning}_{frame_number}_{exposure}_fpos_{filter_position}_{sensor_temp}_{uuid8}"
```

Rendering example (note the lowercase `{target}` slug — the renderer
emits the slug verbatim and the parser's `[a-z0-9-]+` shape requires it;
the impl carries a `parse(render(x)) == x` round-trip assertion):
`m33/2026-06-02/Light/m33_Ha_1x1_0002_120sec_fpos_680_-20C_a1b2c3d4.fits`

Each token has a **typed shape** so the template compiles to an anchored
regex with named captures — never a naive `split('_')`, which the
`fpos_{filter_position}` literal-plus-value segment would break:

| Token | Shape (regex) | Source |
|---|---|---|
| `{target}` | `[a-z0-9-]+` | target slug |
| `{filter}` | `[A-Za-z0-9]+` | filter name |
| `{binning}` | `\d+x\d+` | `Binning` |
| `{frame_number}` | `\d+` | per-spec sequence, rendered zero-padded to width 4 (`0002`) |
| `{exposure}` | `\d+sec` | whole-second `Duration`, rendered `format!("{}sec", d.as_secs())` |
| `{filter_position}` | `\d+` | wheel slot |
| `{sensor_temp}` | `-?\d+C` | measured at capture |
| `{night_date}` | `\d{4}-\d{2}-\d{2}` | observing-night date |
| `{frame_type}` | `Light\|Dark\|Flat\|Bias` | capture intent |
| `{uuid8}` | `[0-9a-f]{8}` | exposure-document id (sidecar link) |

Token encodings are filename-specific and **distinct from** the
config/value encodings: `{exposure}` renders
`format!("{}sec", exposure.as_secs())` (so goal exposures are constrained
to whole seconds — sub-second/non-integer exposures are unsupported in
filenames, independent of the `humantime_serde` *value* encoding, which
uses `120s`/`500ms`), and `{frame_number}` renders zero-padded to width
4. `{frame_type}` names all capture intents, but only `FrameType=Light`
frames bucket against `AcquisitionGoal` quotas (Dark/Flat/Bias live under
their own dirs).

**Config-load validation (parse-don't-validate).** The pattern is parsed
and checked at startup; a bad pattern fails the load, not a session.
Rejection rules: the pattern must contain every token needed to derive
the quota key (`{target}`, `{filter}`, `{binning}`, `{exposure}`) and a
per-frame uniqueness token (`{uuid8}` or `{frame_number}`). It must
compile to an unambiguous anchored regex: between any two variable-width
tokens there must be a literal separator whose characters are excluded
from both the left token's trailing charset and the right token's leading
charset — `_` qualifies because it appears in no token charset, which is
exactly why the default pattern is unambiguous and never falls back to
`split('_')`. A pattern placing two such tokens adjacent (e.g.
`{frame_number}{exposure}`, or `{target}` immediately before
`{night_date}`, whose hyphens/digits the `[a-z0-9-]+` slug would swallow)
is rejected. Unknown tokens are rejected with the offending token named.

### Progress derivation (the "actuals")

`rp` computes progress on demand; nothing is stored:

1. **Total per sub-spec** — scan `<data_directory>/<slug>/<night>/Light/`,
   parse each filename via the template, bucket by
   `(filter, binning, exposure)`. Cheap: `readdir` + regex, no file
   opens. Filenames that don't match the compiled template are skipped
   (`debug!`-logged with the path) — they count toward neither total nor
   any sub-spec and never fail the scan. An absent or empty slug
   directory yields `total = 0` for every sub-spec, so each goal reports
   `0/desired_count` — an uncaptured filter is 0 %, not an error.
2. **Good vs rejected** — for each frame, read its sidecar's grading
   section (metrics written once by the grading plugin), apply the
   **effective** thresholds (`target.grading` field-wise over the config
   default), and classify. The verdict is dynamic: changing a threshold
   re-partitions good/rejected with nothing renamed or moved. The
   grading plugin may cache `(verdict, thresholds_version)` in the
   sidecar to avoid re-evaluating unchanged frames — a cache only,
   recomputed whenever the effective thresholds change, so the verdict is
   never authoritative on disk and stays fully reversible (consistent
   with the no-fixed-verdict rule).
3. **Progress** — compare good-count to `AcquisitionGoal.desired_count`
   per sub-spec.

**Night-date rollover.** `{night_date}` is the date the *observing night*
began — it rolls at local noon, so a frame captured at 01:30 belongs to
the night that started the previous evening. `rp` computes
`night_date = (local_civil_datetime − 12h).date()`, where
`local_civil_datetime` is the capture UTC instant converted through the
site's IANA timezone (DST-aware) — the same `rp_ephemeris::Site` the
planner already holds resolves that timezone from lat/long via `tzf-rs`.
The crate is not involved.

**Rejected-frame representation.** None on disk. Frames are never moved
or renamed for rejection (the verdict is reversible). When handing off to
PixInsight, `rp` materializes the *current* good set (e.g. a generated
list or a copy/symlink folder) — or PixInsight's own SubframeSelector
culls. This hand-off mechanism is deferred and out of scope for the MVP.

### `record_exposure` and progress tools

Because actuals are filesystem-derived, the design-doc-but-unbuilt
`record_exposure(target, filter)` tool (rp.md:830) no longer increments a
stored counter — capture already wrote the frame. It collapses to a no-op
or a progress-cache-invalidation hook. `get_session_progress`
(rp.md:831) and `get_target_status.progress` (today `null`, rp.md:828)
are computed from the store (goals) + the derivation above (actuals).

**Progress shape supersedes the filter-only map.** rp.md:2769-2772
documents progress keyed by filter alone
(`{"Luminance": {completed, goal}}`), which would collapse two goals that
share a filter (e.g. Ha@120s and Ha@300s). Because an `AcquisitionGoal`
is keyed by the full `(filter, binning, exposure)` triple, the progress
shape becomes, per target, a list of
`{filter, binning, exposure, good, total, desired}`. The Rule-2 rp.md
update must replace the filter-only shape accordingly.

### Constraint evaluation

The planner reads `target.scheduling` (falling back field-wise to the
config defaults) and evaluates it with `rp-ephemeris`: `alt_az` ≥
`min_altitude_degrees`, `moon_separation` ≥ `min_moon_separation_degrees`,
moon illumination ≤ `max_moon_illumination_fraction`, and |hour angle
from `transit`| ≤ `meridian_window_hours`. Storage of these fields is
MVP; *enforcement* in selection can be wired in incrementally (store
first, gate later) without a schema change.

## Configuration

New/extended `rp` config (durations are humantime strings per the
[workspace Duration convention](../workspace.md#duration-units); angles
are bare decimal degrees):

```jsonc
{
  "session": {
    "data_directory": "/data/lights",
    "directory_pattern": "{target}/{night_date}/{frame_type}",
    "file_naming_pattern": "{target}_{filter}_{binning}_{frame_number}_{exposure}_fpos_{filter_position}_{sensor_temp}_{uuid8}"
  },
  "targets": {
    "db_path": "/data/lights/targets.redb",      // default: <data_directory>/targets.redb
    "default_scheduling": {
      "min_altitude_degrees": 20.0,
      "min_moon_separation_degrees": 30.0,
      "max_moon_illumination_fraction": 1.0,     // 1.0 ⇒ no moon-brightness limit
      "meridian_window_hours": null              // null ⇒ no meridian window
    },
    "default_grading": {
      "max_hfr_pixels": null,                    // setup-dependent; opt-in
      "min_star_count": 20,
      "max_eccentricity": 0.6,
      "min_snr": null
    }
  }
}
```

`targets.default_*` are the global defaults a `Target`'s `None` override
fields fall back to. The grading defaults are owned by the grading
plugin's contract; they are shown here as the override target.

## MVP scope

**In MVP (this crate):** the `Target` + `AcquisitionGoal` model, the
`TargetSlug` newtype, the `TargetStore` trait, `RedbTargetStore`
(two-table layout, transaction-per-op, ACID), the `schema_version`
migration scaffold, and override storage for scheduling + grading. An
in-memory test double for consumer tests.

**In MVP (rp-side):** target CRUD MCP/REST tools (resolve via
`rp-catalog` → derive slug → `upsert`), the round-trippable naming
template with config-load validation, progress derivation (total from
filenames, good/rejected from sidecar metrics + effective thresholds),
and `get_session_progress` / `get_target_status.progress`.

**Deferred:** ephemeris-gated constraint *enforcement* in target
selection (the constraint fields are stored in MVP, gated incrementally —
note that least-progress *ordering* per rp.md's planner bullet 3 needs
only the in-MVP progress derivation and is therefore in scope, whereas
moon/meridian/altitude *gating* needs ephemeris and is deferred);
seasonal/date scheduling windows; seeding the catalog into the DB for
indexed type/magnitude/cone-search browse; alternative naming grammars
beyond the validated `{token}` brace form (the configurable `{token}`
template itself ships in MVP); the PixInsight good-set hand-off; the
grading plugin itself; multi-site / multi-`rp` plans; and any durable
session/event history (still file/ring-buffer per the predictive-deadlines
plan).

## Behavioral contracts

Happy path:

- **Add catalog target** — `rp` resolves the name via
  `rp_catalog::Catalog::resolve`, denormalizes
  `name/object_type/ra/dec/magnitude/size` onto a `Target`, derives a
  slug, sets `catalog_ref`, and `upsert`s. `get_target(slug)` returns it
  with its (initially empty) goals.
- **Add non-catalog target** — caller supplies raw `ra_hours/dec_degrees`
  (comet, custom framing, mosaic panel); `catalog_ref`/`object_type`/…
  are `None`. Accepted identically.
- **Set goals** — `set_goals(slug, goals)` replaces the goal set
  atomically.
- **Rename** — `upsert` with the same slug and a new `display_name`
  updates in place; the slug and on-disk frames are untouched.
- **List / delete** — `list_targets` returns all rows for the planner to
  filter/order; `delete_target` returns `false` for an absent slug.
  Deleting a target removes only its plan row; slug-keyed frames already
  on disk are intentionally left untouched, so re-adding the same slug
  later silently re-adopts them — `rp` should warn on delete-with-frames,
  or prefer `active = false` to retire a target without orphaning.
- **Reopen after upgrade** — opening a `schema_version < current`
  database migrates it forward within one transaction.

Errors:

- **Invalid slug** — `TargetSlug::new` rejects empty / out-of-charset
  input (caller-side, before `upsert`).
- **Newer on-disk schema** — `UnsupportedSchemaVersion` rather than
  lossy load.
- **Encode/storage faults** — surfaced as the corresponding
  `TargetStoreError` variant; no `.unwrap`/`.expect` in production code
  (workspace lint).
- **(rp-side) bad naming pattern** — rejected at config load with the
  offending token/ambiguity named.
- **(rp-side) missing sidecar for a frame** — that frame counts toward
  `total` but is *ungraded* (cannot be classified good/rejected) until a
  sidecar exists.

Concurrency: `rp` is the sole owner (Q4). The database is opened once;
writes are serialized by `redb`'s single-writer transaction model.
External consumers (UI, orchestrator) read/write targets through `rp`'s
API, never the file directly. `redb` takes an exclusive OS file lock on
open, so a stray second opener fails fast with an `Open` error rather
than corrupting the file; a crash mid-migration leaves either the
pre-migration committed state or the fully-migrated state, never a
partial one (the migration runs in a single write transaction).

## Module Layout

```
crates/rp-targets/src/
├── lib.rs        # crate root: TargetStore trait + re-exports
├── model.rs      # Target, AcquisitionGoal, Binning,
│                 #   SchedulingConstraints, GradingThresholds, TargetSlug
├── error.rs      # TargetStoreError (thiserror)
├── redb_store.rs # RedbTargetStore: tables, transaction-per-op, spawn_blocking
├── migrate.rs    # schema_version constant + ordered migration steps
└── memory.rs     # InMemoryTargetStore test double (cfg(any(test, feature = "mock")))
```

Crate-root attributes match the sibling crates:
`#![cfg_attr(coverage_nightly, feature(coverage_attribute))]` and
`#![deny(unsafe_code)]`.

## Testing

- **Unit (in-crate):** `Target`/`AcquisitionGoal` serde round-trip;
  `TargetSlug` normalization + rejection; `upsert` overwrites rather than
  duplicates; `delete` of present vs absent; `set_goals` replaces;
  migration from a checked-in `v1` fixture database; `UnsupportedSchema`
  on a future version. Tests use `.unwrap()` per
  [testing.md](../skills/testing.md), scoped via the `#[allow(...)]` on
  the test module.
- **Test double:** `InMemoryTargetStore` (a `BTreeMap<String, Target>`
  behind the same trait) gives `rp`'s planner deterministic, clock-free
  unit tests without a temp database. Offered alongside (not instead of)
  a `mockall::automock` option for tests that want call-assertions.
- **BDD (rp-side, Phase 2):** target CRUD via MCP; progress derivation
  over a fixture `data_directory` (total from filenames, good/rejected
  from fixture sidecars + thresholds); naming-pattern validation
  rejections; constraint-gated selection. Feature files are the contract
  per [development-workflow.md](../skills/development-workflow.md).

## Dependencies

| Crate | Purpose |
|---|---|
| `redb` | embedded ACID key-value store (the file format) |
| `serde` / `serde_json` | value encoding inside `redb` |
| `humantime-serde` | `Duration` (exposure) config/value encoding |
| `thiserror` | `TargetStoreError` derive |
| `tracing` | `debug!` on store operations |
| `async-trait` | the `TargetStore` async seam |

`redb` is a new crates.io dependency. It is MIT-OR-Apache-2.0 and pure
Rust with no `build.rs` C compile — satisfying the no-system-C /
permissive-license bar that [ADR-001](../decisions/001-fits-file-support.md)
and [ADR-002](../decisions/002-tls-for-inter-service-communication.md)
established, and building cleanly on all four target platforms including
the Raspberry Pi 5. As only `rp-targets` uses it initially, it is
declared in `crates/rp-targets/Cargo.toml` rather than hoisted to the
workspace (CLAUDE.md Rule 10); after adding it, run
`CARGO_BAZEL_REPIN=1 bazel mod tidy && bazel mod tidy` to refresh
`MODULE.bazel.lock` (Rule 10 / [bazel notes](../workspace.md#bazel-primary-ci-gate)).
The crate does **not** depend on `rp-catalog` or `rp-ephemeris`: catalog
resolution and ephemeris evaluation happen in `rp`, which passes already
-resolved coordinates into `upsert_target`, keeping the store pure.

## Decision Rationale (alternatives considered)

Captured from the requirements discussion that produced this design, so
the *why* travels with the crate (a candidate to promote to a formal
ADR-007 if desired):

- **redb over SQLite / a structured file / native_db.** SQLite is C
  (against the pure-Rust lean; its SQL strengths go unused since the hard
  selection queries — altitude, moon, progress — run app-side anyway). A
  plain JSON/TOML file needs no dependency and is defensible at this
  scale, but gives no transactional partial update and no growth room.
  `native_db` would hand us indexes + migrations but its API is
  explicitly unstable and it pins the on-disk encoding. `redb` is the
  pure-Rust, permissive, ACID middle: a real database with a stable
  *within-generation* file format (a major redb upgrade may need a
  one-time `Database::upgrade()`) and a minimal dependency footprint that
  fits the Pi and the conservative-dependency culture the licensing /
  no-system-C ADRs established (ADR-001/002).
- **Slug identity, not UUID.** A human, immutable slug *is* the filename
  token, so frame→target matching is trivial; it matches `rp-catalog`'s
  name-keying; UUIDs stay on transient operational artifacts.
- **Actuals derived from files, not stored.** The filesystem is the
  source of truth (cull in PixInsight and counts update); the grading
  verdict is computed from per-frame sidecar metrics + dynamic
  thresholds, so it is reversible and never baked into disk layout.
- **Pure plan repository, not a progress/selection engine.** Keeps the
  crate clock-free, filesystem-free, and mockable, consistent with the
  `rp-ephemeris` / `rp-catalog` / `rp-fits` split.
- **Targets denormalize catalog coordinates.** Self-contained rows that
  also represent non-catalog targets (comets, custom framings, mosaic
  panels); the catalog stays embedded and read-only.
