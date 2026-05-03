# Plan: rp planning & ephemeris tools

**Date:** 2026-05-03
**Branch:** `worktree-planning-tools`
**Parent design doc:** [`docs/services/rp.md`](../services/rp.md)
**Closest precedent:** [`docs/plans/plate-solver.md`](plate-solver.md) (eight-phase, design-first plan)

## Background

Today rp's `targets[]` config carries J2000 RA/Dec by hand and the
"Dynamic Planner" section of `rp.md` describes `get_next_target` /
`get_target_status` decision logic without specifying who computes
altitude, transit, rise/set, twilight, moon position, or meridian-flip
timing. There is no observer-site config (lat/lon), no name-to-coords
catalog, and no ephemeris module. A user who says "image M41 right
now" cannot do so without manually pasting coordinates and trusting the
mount's pointing model.

This plan stands up the missing pieces: a small `rp-ephemeris` crate
that does the pure math, a small `rp-catalog` crate that resolves
common object names (M41, NGC 2287, IC 405) to ICRS coordinates,
site-location config with mount-side validation, and the MCP tool
surface that exposes both as primitive operations a planner plugin can
compose plus high-level convenience tools the default planner uses.

The math choice — wrap `erfars` (the Rust FFI binding for ERFA, the
BSD-licensed clean-room derivative of IAU SOFA used by Astropy and
LSST) behind a thin trait, in-process — is settled by an
out-of-band crate survey and a deep audit of the
otherwise-plausible-looking `astro-math` crate (the audit found
provably-wrong code in functions we'd ship). The trait wall is both
the swap-out point if `erfars` ever proves problematic and the seam
against the MCP tool layer. See "Decisions resolved" below for the
in-process-vs-service rationale.

## Goals

1. **MVP:** `rp` can take "image M41 right now" from a name string and
   produce a slew target, an altitude estimate, a transit time, and a
   meridian-flip ETA — with mount-side site validation as a hard error
   on mismatch.
2. **Granularity contract:** primitive MCP tools (one operation each)
   form a stable surface a planner plugin can compose; high-level
   convenience tools (`get_next_target`, `get_target_status`) are
   built on the same primitives, replaceable wholesale.
3. **Insulate against upstream risk:** the `erfars` dependency sits
   behind an `Ephemeris` trait so it can be swapped (vendored,
   replaced with hand-rolled Meeus, or extracted into a managed
   service) without touching MCP plumbing or the planner.
4. **Update `rp.md`** so the Dynamic Planner section actually
   specifies how altitude / transit / meridian-flip get computed, and
   the Configuration section carries the new `site` block.

## Decisions resolved (during design)

These are the load-bearing design calls; they belong in `rp.md`'s
"Planning and Ephemeris" section once Phase 1 lands and should not be
re-litigated.

### Wrap `erfars` (ERFA / IAU SOFA), behind a trait, in-process

The math layer wraps **`erfars`**, the Rust FFI binding for ERFA
(Essential Routines for Fundamental Astronomy — the BSD-licensed
clean-room derivative of IAU SOFA, function-for-function compatible
with SOFA C, used by Astropy, LSST/Rubin, and the gravity-wave
pipelines). License: BSD 3-clause for ERFA, MIT for `erfars` —
clean inside this workspace's MIT/Apache posture.

The `Ephemeris` trait in `rp-ephemeris` exposes only safe Rust;
`erfars`'s `unsafe` is bounded inside the binding crate. Twilight
and rise/set primitives are not in ERFA's surface — they live in
`rp-ephemeris` as small root-finders over the ERFA-supplied sun /
target altitude.

#### Why not `astro-math`

A deep audit of `astro-math` (April 2026, recorded in this PR's
review thread) found a provably wrong `sun_rise_set` implementation
(arguments to `atan2` swapped, off by up to ~6 hours), a silent
wrong-answer in `alt_az_to_ra_dec` at the celestial poles, a silent
fallback from the ERFA-backed alt/az path to a buggy hand-rolled
path on ERFA error, no twilight functions at all, 137 transitive
dependencies, 8 months without a commit, two GitHub stars, and
hobby-tier commit messages. The earlier "borrow `astro-math` behind
a trait" framing in this plan was wrong — the trait wall would
isolate the dependency identity but not the algorithmic bugs we'd
ship. Going directly to ERFA via `erfars` puts us on the same
algorithmic foundation Astropy uses, with a much smaller binding
surface to audit.

#### Why in-process, not a managed service

ERFA is in a fundamentally different risk class from ASTAP (which
*is* a managed service, justified by its specific failure modes).
ERFA is pure computation: no I/O, no file parsing, no allocation in
the hot path, scalar-only inputs, ~25 years of heavy production use
in safety-critical astronomy software. The realistic SIGSEGV
surface is "binding bug we hit on first call and immediately fix,"
not "crashes mysteriously at 3am during a session." The workspace
already runs C code in-process for crypto (`aws-lc-rs` /
`ring`); the engineering posture for well-bounded C deps is
"in-process behind a safe Rust binding."

Isolating `erfars` in a service would route every primitive call
through HTTP. The planner makes many small ephemeris calls per
decision (alt/az for every candidate target, transit times, sun
altitude for twilight, moon position + separation per candidate,
meridian-flip ETA). That's 20+ HTTP roundtrips per `get_next_target`
call — meaningful complexity and latency for safety we don't
actually need.

If a real ERFA-related crash is ever observed in production, the
trait wall makes service-extraction a mechanical follow-up.

#### Reference-value tests

Phase 2's reference-value tests assert `ErfarsEphemeris` outputs
match canonical values (Astropy script-generated, since Astropy
uses ERFA internally; agreement should be near-perfect, so
tolerances can be tight — 0.1 arcsec for alt/az, 1 second for
transit/rise/set). Disagreement is a wrapping bug, not an
algorithmic one — caught at a fixed surface.

### Use `tzf-rs` for lat/lon → IANA timezone

Site config holds lat/lon only. The IANA timezone name (`Europe/Madrid`,
`America/Santiago`) is derived at startup via `tzf-rs` (MIT-licensed
crate, ODbL-licensed bundled polygon data from
`evansiroky/timezone-boundary-builder`, currently maintained). System
tzdata then supplies DST rules. Cost: `DefaultFinder` with the
simplified dataset reports ≈128 MiB peak RSS (per the upstream README;
the full-precision dataset is ~5× larger and not enabled). The
finder is constructed once at startup, held for the lifetime of the
process, and the derived timezone is logged immediately so a
misconfigured lat/lon surfaces as a visibly-wrong timezone before it
produces wrong twilight times.

The tzf-rs licence advertises an additional "Anti CSDN License"
clause forbidding the Chinese aggregator CSDN specifically; the
upstream description and Cargo.toml make clear it has no practical
effect on this workspace's use. If the constraint ever becomes
material the swap-out point is `rp_ephemeris::site` — `Site` exposes
`iana_timezone()` and nothing further leaks.

### No elevation in v1 site config

For deep-sky targets, elevation is irrelevant: sidereal time depends
only on longitude, and the mount's refraction model handles
pressure/temperature. Elevation matters only for solar-system targets
(moon parallax up to ~1°, planets sub-arcsecond) and for horizon-dip
in twilight (~1° at 4000 m, negligible). Adding `elevation_meters`
later is a backwards-compatible config addition; not in v1.

A horizon profile (per-azimuth obstruction) is similarly deferred — a
single `min_altitude_degrees` covers the common case.

### Site config validated against the ASCOM mount on connect

`SiteLatitude` / `SiteLongitude` are standard ASCOM Telescope
properties. On connect, rp reads both and compares to config. **Hard
error on mismatch beyond 0.01°** (≈1 km), not a warning. Silently
running ephemeris math against a site that disagrees with what the
mount computes hour-angle from is precisely the class of bug that
produces plausible-looking wrong slew targets — i.e., the worst kind.

If the mount does not implement `SiteLatitude`/`SiteLongitude` (the
ASCOM `CanGetSite*` capability bits are false), config is the source
of truth and a `debug!()` log notes that mount validation was
skipped.

### Two-layer MCP tool surface — primitives plus convenience

Both layers call the same internal `Ephemeris` trait. The split is
purely how the operations are projected onto the MCP catalog:

**Primitive tools (one operation each):**

| Tool | Returns |
|------|---------|
| `resolve_target {name}` | ICRS RA/Dec, object type, magnitude (catalog) |
| `compute_alt_az {ra, dec, time?}` | altitude, azimuth |
| `compute_transit {ra, dec, date}` | UT of upper transit |
| `compute_rise_set {ra, dec, date, min_alt_degrees}` | rise/set times |
| `compute_meridian_flip {ra, dec, time, side_of_pier}` | time-to-flip |
| `get_sun_position {time?}` | RA/Dec, alt/az |
| `get_twilight {date, kind}` | civil/nautical/astronomical begin/end |
| `get_moon_position {time?}` | RA/Dec, alt/az, phase, illumination |
| `compute_moon_separation {ra, dec, time?}` | angular separation between target and moon |
| `get_local_sidereal_time {time?}` | LST |

**High-level convenience tools (compose primitives):**

| Tool | Existing in `rp.md` |
|------|---------------------|
| `get_target_status {target_name}` | Yes — finally implementable (parameter name matches the existing `rp.md` §"Dynamic Planner" table) |
| `get_next_target` | Yes — finally implementable |
| `get_meridian_status` | Yes |
| `record_exposure`, `get_session_progress` | Yes — orthogonal to ephemeris |

The chattiness cost of primitives is zero: planning runs at
target-switch cadence (minutes/hours), not per-frame. A plugin that
makes 20 MCP calls to compute "best target for the next 90 minutes"
is imperceptible.

### Catalog: built-in offline, not a plugin

`resolve_target` ships with an embedded Messier + NGC + IC catalog
(~13k objects). Justification: typing a Messier number and getting
coordinates is too core to require a plugin install, the data is
small (well under 1 MB compressed), and offline operation matters at
remote dark sites.

Source: openNGC (CC-BY-SA-4.0; attribution in the crate's README and
the data file's source-pointer comment) or equivalent. Final source
pick happens during Phase 5 — license-check before commit.

A future SIMBAD-backed plugin remains possible; the
`resolve_target` MCP tool name belongs to rp, but a plugin can
provide an enriched implementation by registering the same tool name
under the existing tool-provider override mechanism.

### Two crates, not one

`rp-ephemeris` (math) and `rp-catalog` (data) are separate workspace
members because:

- They have different update cadences (catalog rarely; ephemeris if
  the math dep changes).
- The catalog crate has zero math deps; rp-ephemeris has zero data
  deps. Test isolation is cleaner.
- Either could be useful to other tooling in the workspace
  independently (e.g., a future planning UI or test fixture).

Both are pure-function libraries with no I/O beyond reading their own
embedded data; no service supervision, no HTTP surface.

## MVP scope

### In scope for v1

- `site` config block: `latitude_degrees`, `longitude_degrees`.
- IANA timezone derivation at startup via `tzf-rs`, logged.
- ASCOM mount site validation on connect, hard error on mismatch.
- `rp-ephemeris` crate: `Ephemeris` trait + `ErfarsEphemeris`
  impl (in-process, safe Rust over the `erfars` ERFA bindings) +
  reference-value tests against Astropy.
- `rp-catalog` crate: embedded Messier + NGC + IC; `resolve(name) →
  ResolvedTarget` (RA, Dec, type, magnitude).
- Primitive MCP tools listed above (10 operations).
- High-level MCP tools: `get_target_status`, `get_next_target`,
  `get_meridian_status` actually backed by computed values.
- `target` definitions in config still accept literal RA/Dec; the
  catalog is opt-in via `name` lookup, not required.

### Out of scope for v1 (deferred or never)

- Elevation in site config.
- Horizon profile / per-azimuth obstruction.
- Solar-system targets (moon, planets) as imaging targets — the
  primitives compute their positions, but `resolve_target` and the
  planner do not handle them as targets.
- Custom catalogs (user-supplied star lists, exoplanet host catalogs).
- SIMBAD or external-service catalog lookup.
- Polar-alignment / pointing-model calculations — owned by the mount.
- Field-rotation predictions for alt-az mounts — rp is equatorial-only
  in v1.
- Replacing the planner with a multi-night optimizer — the high-level
  tools ship with the simple decision logic already in `rp.md`; smarter
  policy is a planner-plugin concern enabled by this work, not done by
  it.

## Module structure

```
crates/rp-ephemeris/
  Cargo.toml
  src/
    lib.rs                # Ephemeris trait + ResolvedSky / SunInfo / MoonInfo / etc.
    erfars_impl.rs        # ErfarsEphemeris: Ephemeris-trait wrapper around the erfars ERFA bindings
    derived.rs            # Twilight + rise/set + meridian-flip: small root-finders over ERFA-supplied positions
    site.rs               # Site { lat, lon } + tzf-rs timezone derivation
  refvals/
    *.json                # Reference Astropy values per object (Astropy uses ERFA — agreement should be near-perfect)
    gen.py                # Astropy generator script (run by hand, not CI)
  tests/
    reference_values.rs   # Asserts ErfarsEphemeris output matches refvals/

crates/rp-catalog/
  Cargo.toml
  src/
    lib.rs                # Catalog::resolve(name) -> Option<ResolvedTarget>
    data/
      messier.csv         # ~110 entries
      ngc.csv             # ~7,840 entries
      ic.csv              # ~5,386 entries
      LICENSE-DATA        # openNGC CC-BY-SA-4.0 (or replacement) + attribution
  tests/
    resolution.rs         # Known M / NGC / IC names resolve to known coords

services/rp/src/
  config.rs                # +SiteConfig; +validation against telescope on connect
  equipment.rs             # +read SiteLatitude/SiteLongitude; +hard-error mismatch
  planner/
    mod.rs                 # planner module (new sub-tree under src/)
    primitives.rs          # MCP wrappers: compute_alt_az, ..., get_local_sidereal_time
    catalog.rs             # MCP wrapper: resolve_target
    convenience.rs         # MCP wrappers: get_target_status, get_next_target, get_meridian_status
    decision.rs            # The "decision logic" from rp.md §Dynamic Planner
  mcp.rs                   # +planner tool registrations
  tests/features/
    site_validation.feature
    target_catalog.feature
    ephemeris_primitives.feature
    planner.feature
```

The closest workspace reference for a pure-function library crate is
`crates/rp-tls` (config-shaped, no service supervision). The closest
reference for a tool-family added to rp is the focuser primitives in
`services/rp/src/equipment.rs` plus their `tests/features/focuser.feature`.

## Phases

Each phase is its own PR. Within a phase, BDD scenarios land tagged
`@wip` and the tag is removed in the same commit that lands the
implementation (per `docs/skills/development-workflow.md`
§"Committing Phase 2 before Phase 3" and `docs/skills/testing.md`
§2.7).

### Phase 1 — Design doc updates

Status: **not started.**

- [ ] `docs/services/rp.md`: insert new section **"Planning and
      Ephemeris"** before §"Dynamic Planner" covering: site config,
      mount-side validation rule, IANA timezone derivation, the
      `Ephemeris` trait shape, the primitive vs. convenience MCP tool
      split (table per Decisions Resolved), the catalog source and
      license posture.
- [ ] `docs/services/rp.md` §"Dynamic Planner": update so the existing
      decision-logic bullets reference the primitive tools by name
      (e.g., "altitude check" → "compute_alt_az + min_altitude_degrees
      from config"), instead of leaving the "who computes this" gap.
- [ ] `docs/services/rp.md` §"Configuration": add a `site` block to the
      example JSON; reference the validation rule. Add the new MCP
      tools to the tool catalog example.
- [ ] `docs/services/rp.md` §"Equipment Integration": one paragraph
      under "ASCOM Alpaca Devices" naming `SiteLatitude` /
      `SiteLongitude` as read-on-connect properties and pointing at
      the validation rule in the Planning section.
- [ ] `docs/workspace.md`: add `rp-ephemeris` and `rp-catalog` to the
      Shared Crates table.
- [ ] No new ADR. The `erfars` and `tzf-rs` choices are tactical
      dep picks (similar to rmcp / fitsrs) — captured in this plan's
      Decisions Resolved section, no ADR file needed unless a future
      swap warrants one. The "in-process, not a managed service"
      decision for ERFA is also captured there, with explicit
      contrast to the plate-solver supervision posture.

**Exit criteria:** updated docs reviewed and merged. No code yet.
The `Ephemeris` trait sketched in prose in the design doc is the
contract Phase 2 implements verbatim.

### Phase 2 — `rp-ephemeris` crate (trait + impl + reference-value tests)

Status: **not started.**

- [ ] New workspace member `crates/rp-ephemeris` registered in root
      `Cargo.toml`. Standard crate metadata (workspace inheritance for
      version, edition, rust-version, lints).
- [ ] `BUILD.bazel` for the new crate; `CARGO_BAZEL_REPIN=1 bazel mod
      tidy` after adding `erfars` and `tzf-rs` to workspace deps.
- [ ] `src/lib.rs` — `Ephemeris` trait per the design doc. Methods are
      pure functions (`&self` only for any cached state inside an
      impl, no I/O). Surface:
      ```text
      sidereal_time(site, time) -> LocalSiderealTime
      alt_az(site, target_icrs, time) -> AltAz
      transit(site, target_icrs, date) -> Option<UtcTime>
      rise_set(site, target_icrs, date, min_alt_deg) -> Option<RiseSet>
      meridian_flip(site, target_icrs, time, side_of_pier) -> Option<DurationToFlip>
      sun_position(site, time) -> SunInfo  // RA/Dec + AltAz
      twilight(site, date, kind) -> TwilightWindow
      moon_position(site, time) -> MoonInfo  // + phase, illumination
      moon_separation(target_icrs, time) -> AngleDeg
      ```
      Inputs/outputs are concrete types in `rp_ephemeris::types`, not
      `erfars`-shaped. The trait surface contains zero `unsafe` and
      no `erfars` types — those stay inside `erfars_impl.rs`.
- [ ] `src/erfars_impl.rs` — `ErfarsEphemeris` implementing the trait
      by calling `erfars` for the ERFA-native operations:
      - `sidereal_time` → ERFA `Gst06a` (apparent sidereal time) on
        a UT1 input, with a documented note that ΔUT1 is treated as
        zero (UT1 ≈ UTC error ≤ 0.9 s = ≤ 13″ of LST — well inside
        what plate-solving will refine).
      - `alt_az` → ERFA `Atco13` (the high-precision ICRS →
        observed path; bundles precession, nutation, aberration,
        polar motion, refraction). Returns a typed error if ERFA
        signals one — no silent fallback.
      - `sun_position` → ERFA `Epv00` (Earth heliocentric position) +
        sign flip + ERFA frame-rotation routines for GCRS.
      - `moon_position` → ERFA `Moon98`. Phase computed from
        ecliptic-longitude difference with the Sun (~1° accuracy,
        sufficient for "is the moon close to my target?" checks).
      Unit-conversion (degrees ↔ radians, JD double-precision split,
      time-scale handling) lives here, not in the trait surface.
- [ ] `src/derived.rs` — Operations not in ERFA's surface, built as
      small root-finders over ERFA-supplied positions:
      - `transit` → solve for the time where the target's hour angle
        is zero (closed form from LST and target RA).
      - `rise_set` → bisection on target altitude, bracketed by the
        transit and the previous/next anti-transit.
      - `twilight` → bisection on sun altitude crossing −6 / −12 /
        −18°, bracketed by sunset/sunrise.
      - `meridian_flip` → closed-form from current hour angle and
        side-of-pier convention.
      Each function is unit-tested with an in-source `#[cfg(test)]
      mod tests` block against a couple of known cases (e.g.,
      Polaris never transits at the equator → returns `None`;
      M31 transit time at Greenwich on the 2026 spring equinox).
- [ ] `src/site.rs` — `Site { latitude_degrees, longitude_degrees }`
      with `pub fn iana_timezone(&self) -> &'static str` derived once
      at construction via `tzf-rs`. `Display` impl logs lat/lon + tz
      so `info!("site: {site}")` is operator-friendly.
- [ ] `tests/reference_values.rs` — golden-file tests. Pick ~10 named
      objects (Polaris, M31, M42, M81, Sirius, Vega, Antares, NGC
      891, IC 1396, the Sun, the Moon) at ~3 sites (a mid-northern, a
      mid-southern, an equatorial) at ~3 times (solstices and an
      equinox). For each (object, site, time) tuple, the canonical
      Astropy value is committed in `refvals/<object>.json` (crate
      root, alongside the generator), and the test asserts
      `ErfarsEphemeris` output is within tolerance (**0.1 arcsec**
      for alt/az, **1 second** for transit/rise/set, 0.1° for moon
      phase). Tolerances are tight because Astropy uses ERFA
      internally — disagreement is a wrapping bug, not an
      algorithmic one. Reference values are generated once via a
      documented Astropy script committed under
      `crates/rp-ephemeris/refvals/gen.py` — not run in CI;
      provenance is the commit message.
- [ ] No `mockall` here — the `Ephemeris` trait is consumed by rp's
      planner module, which mocks it there if needed. The trait
      itself is tested via the real `ErfarsEphemeris` impl.
- [ ] **No `unsafe` in `rp-ephemeris`'s own code.** `erfars` already
      encapsulates the unsafe FFI; the wrapper exposes only safe
      Rust. Enforced via `#![deny(unsafe_code)]` at the crate root.

**Exit criteria:** `cargo build -p rp-ephemeris --all-features
--all-targets` clean. `cargo nextest run -p rp-ephemeris
--all-features` green. `cargo rail run --profile commit -q` clean.

### Phase 3 — `rp-catalog` crate (embedded Messier + NGC + IC)

Status: **not started.**

- [ ] License pick: confirm openNGC CC-BY-SA-4.0 is acceptable for
      embedded data in this workspace (project license is dual
      MIT/Apache; CC-BY-SA-4.0 attribution requirements are
      compatible with bundling under those terms — verify before
      merge). If not, fall back to NASA NED or HEASARC public-domain
      sources. Outcome documented in
      `crates/rp-catalog/src/data/LICENSE-DATA`.
- [ ] New workspace member `crates/rp-catalog` registered in root
      `Cargo.toml`.
- [ ] `BUILD.bazel`; `bazel mod tidy` not required (no new
      crates.io deps — uses workspace `serde` / `csv` only).
- [ ] `src/data/messier.csv`, `ngc.csv`, `ic.csv` — committed CSVs
      with columns `name, type, ra_hours, dec_degrees, magnitude,
      size_arcmin`. Aliases (e.g., M42 ↔ NGC 1976) covered in a
      separate `aliases.csv`.
- [ ] `src/lib.rs` — `Catalog::load_embedded() -> Catalog` parses
      CSVs at startup via `include_str!` + `csv` crate; `pub fn
      resolve(&self, name: &str) -> Option<ResolvedTarget>` does
      case-insensitive lookup with whitespace-insensitive matching
      (`"m41"`, `"M 41"`, `"M41"`, `"Messier 41"` all resolve).
      Aliases handled in the lookup, not at parse time, so the data
      stays one-row-per-object.
- [ ] `tests/resolution.rs` — exhaustive smoke for the well-known
      Messier objects and a sampling of NGC / IC entries; alias
      lookup; case-insensitive lookup; missing-object → `None`.

**Exit criteria:** `cargo nextest run -p rp-catalog --all-features`
green. README documents the data source and license.

### Phase 4 — Site config + mount-side validation in rp

Status: **not started.**

- [ ] `services/rp/src/config.rs` — `SiteConfig {
      latitude_degrees: f64, longitude_degrees: f64 }` with serde
      validation (lat ∈ [−90, 90], lon ∈ [−180, 180]). Wired into
      the top-level `Config` struct under a new `site` field. Unit
      tests cover the range validation and a happy-path round-trip.
- [ ] `services/rp/src/equipment.rs` — on telescope connect, read
      `SiteLatitude` / `SiteLongitude` via the existing
      `ascom-alpaca` telescope feature. If `CanGetSiteLatitude` and
      `CanGetSiteLongitude` are both true, compare to config; abs
      diff > 0.01° in either dimension → return a typed error
      `SiteMismatch` containing both pairs. If the mount lacks the
      capability, `debug!()` log and proceed.
- [ ] `services/rp/tests/features/site_validation.feature` —
      scenarios:
      1. Config + mount agree → connect succeeds, log line shows
         derived IANA timezone.
      2. Config + mount disagree (lat off by 1°) → connect fails,
         error message names both lat values.
      3. Mount lacks `CanGetSiteLatitude` → connect succeeds, debug
         log notes validation was skipped.
      4. Config lat out of range → config-load error, named field.
      Target ~5 scenarios.
- [ ] All four feature scenarios tagged `@wip` initially; tag
      removed in the same commit that lands the validation
      implementation.
- [ ] `services/rp/tests/bdd/steps/site_steps.rs` — new step file.
      Reuses the existing OmniSim Alpaca telescope plumbing; OmniSim
      already exposes configurable `SiteLatitude` / `SiteLongitude`
      so no test-double extension is needed.
- [ ] `services/rp/src/equipment.rs` and any related modules updated
      so the `Ephemeris` trait + `Site` are available wherever the
      planner module needs them; no MCP exposure yet (Phases 5–7).
- [ ] `docs/services/rp.md` Configuration example: ensure the `site`
      block lands now, not later (it's now load-bearing for
      `cargo run`).
- [ ] `docs/references/ascom-alpaca.md` — extend with a Telescope
      section covering `SiteLatitude`, `SiteLongitude`,
      `CanGetSiteLatitude`, `CanGetSiteLongitude` (the four properties
      this phase actually reads), so future contributors don't have to
      chase the upstream spec for the in-repo workflow.

**Exit criteria:** all `site_validation.feature` scenarios green.
`cargo rail run --profile commit -q` clean. The rp binary fails
loudly at startup on a misconfigured site, and logs the derived
timezone on success.

### Phase 5 — `resolve_target` MCP tool

Status: **not started.**

- [ ] `services/rp/src/planner/catalog.rs` — MCP wrapper around
      `rp_catalog::Catalog::resolve`. Returns
      `{ name, ra_hours, dec_degrees, object_type, magnitude,
      size_arcmin }` or a structured "not found" error with a
      suggestion list (top 3 fuzzy matches, computed by Levenshtein
      over canonical names).
- [ ] `services/rp/src/mcp.rs` — register `resolve_target`.
- [ ] `services/rp/tests/features/target_catalog.feature` —
      scenarios:
      1. `resolve_target {name: "M41"}` → returns M41 coords (assert
         exact RA/Dec from openNGC).
      2. `resolve_target {name: "NGC 2287"}` → resolves to the same
         object (alias).
      3. `resolve_target {name: "M999"}` → structured not-found with
         a suggestions list.
      4. Case- and whitespace-insensitive lookup for `"m 41"`.
      Target ~6 scenarios.
- [ ] `target` definitions in config remain unchanged — they still
      accept literal RA/Dec. Catalog lookup is a tool call, not a
      config-time resolution. (A future enhancement could let
      `targets[]` reference catalog names; explicitly out of v1
      scope.)

**Exit criteria:** all `target_catalog.feature` scenarios green
without `@wip`. `cargo rail run --profile commit -q` clean.

### Phase 6 — Primitive ephemeris MCP tools

Status: **not started.**

- [ ] `services/rp/src/planner/primitives.rs` — MCP wrappers for the
      10 primitive operations from the Decisions Resolved table.
      Each wrapper is a thin `serde_json` ↔ `Ephemeris`-trait-call
      adapter; the math lives in `rp-ephemeris`.
- [ ] `services/rp/src/mcp.rs` — register all 10 primitives.
- [ ] `services/rp/tests/features/ephemeris_primitives.feature` —
      one scenario per primitive, with a known-answer check (the
      reference-value table from Phase 2 supplies the canonical
      values; the BDD scenario asserts the MCP response matches).
      Two scenarios per error case (out-of-range time, RA/Dec out of
      range, unknown twilight kind). Target ~15 scenarios.
- [ ] All scenarios `@wip` until the implementation lands in the
      same commit.
- [ ] No new test-double crates needed — the `Ephemeris` trait is
      consumed via its real implementation. The reference-value
      tests in Phase 2 already validate the underlying math; these
      BDD scenarios validate the MCP wrapping (serde shapes,
      error mapping, time-default handling).

**Exit criteria:** all `ephemeris_primitives.feature` scenarios
green without `@wip`. `cargo rail run --profile commit -q` clean.

### Phase 7 — High-level convenience tools (`get_target_status`, `get_next_target`)

Status: **not started.**

- [ ] `services/rp/src/planner/decision.rs` — implement the
      decision-logic bullets from `rp.md` §"Dynamic Planner" as a
      pure function over the planner's inputs (target list, current
      time, site, `Ephemeris` impl, per-target progress). Step 1
      (eliminate-by-altitude) calls `compute_alt_az`; step 2
      (prefer-transiting) calls `compute_transit`; step 5
      (meridian-flip avoidance) calls `compute_meridian_flip`. The
      decision function returns `NextTargetRecommendation` with a
      structured `reason` field (one of
      `BestTransitingCandidate`, `LeastProgress`, `WaitForTwilight`,
      `EndOfSession`, etc.).
- [ ] `services/rp/src/planner/convenience.rs` — MCP wrappers for
      `get_target_status`, `get_next_target`, `get_meridian_status`.
- [ ] `services/rp/src/mcp.rs` — register the three.
- [ ] `services/rp/tests/features/planner.feature` — scenarios:
      - `get_target_status {target_name: "M41"}` mid-evening → returns
        positive altitude, transit time in the future, finite
        time-to-set.
      - Same call near dawn → altitude negative or below
        `min_altitude_degrees`, status flagged "below horizon".
      - `get_next_target` with one transiting and one rising target
        → returns the transiting one with `reason:
        BestTransitingCandidate`.
      - `get_next_target` with two equally-transiting targets,
        different progress → returns the less-progressed one with
        `reason: LeastProgress`.
      - `get_next_target` after all targets complete → returns
        `EndOfSession`.
      - `get_meridian_status` 30 min before flip → returns
        `time_to_flip: 30m, side_of_pier: east`.
      Target ~8 scenarios.
- [ ] All scenarios `@wip` until the implementation lands in the
      same commit.
- [ ] BDD World gains a `frozen_time: Option<DateTime<Utc>>` for
      determinism — the decision logic accepts an `Ephemeris`
      handle plus an explicit `now: DateTime<Utc>` so tests don't
      race the wall clock. The MCP wrappers default `now` to
      `Utc::now()` when the call omits it.

**Exit criteria:** all `planner.feature` scenarios green without
`@wip`. `cargo rail run --profile commit -q` clean.

## Sequencing notes

**All seven phases land before this work is considered complete.**
None are "harden later." The decisions in `rp.md` §"Dynamic Planner"
are vacuous until Phase 7 lands the implementation that actually
honors them.

Within that fixed scope, dependencies are mostly linear:

- Phase 1 → 2 → 4: design doc → math crate → site config + mount
  validation. The site work (Phase 4) needs `rp-ephemeris` in scope so
  the timezone display works on connect.
- Phase 3 (catalog) is independent of Phase 2 (ephemeris); both can
  land in parallel once Phase 1 merges. Phase 3 does **not** block
  Phase 4.
- Phase 5 (`resolve_target` MCP) needs Phase 3.
- Phase 6 (primitive MCP tools) needs Phases 2 and 4.
- Phase 7 needs Phases 5 and 6 (the convenience tools compose
  primitives, and the planner's decision logic uses the catalog for
  any future name-referenced targets).

**Cross-plan unblocking:**

- `image-evaluation-tools.md` Phase 6c-prep (telescope primitives:
  `slew`, `sync_mount`, `get_telescope_position`) is a prerequisite
  for *exercising* the planner end-to-end ("slew to M41"), but is
  **not** a prerequisite for any phase here — the MCP tools land and
  the BDD scenarios pass without an actual mount slew. The first
  end-to-end "image M41 right now" demo requires both this plan's
  Phase 7 and 6c-prep + 6c-3 (`center_on_target`) to be complete.

## References

- Crate survey + deep audit (April 2026): the initial survey
  flagged `astro-math` as the only single-crate option; the
  follow-up code audit found provably-wrong `sun_rise_set` math, a
  silent fallback from the ERFA path to a buggy hand-rolled path,
  no twilight functions, 137 transitive deps, and 8 months of
  inactivity. Conclusion: wrap `erfars` (ERFA / IAU SOFA) directly,
  in-process, behind the `Ephemeris` trait. `tzf-rs` remains the
  right tz-from-coords pick. No ADR — captured in this plan's
  Decisions Resolved.
- [`docs/services/rp.md`](../services/rp.md) §"Dynamic Planner",
  §"Configuration", §"Equipment Integration"
- [`docs/skills/development-workflow.md`](../skills/development-workflow.md)
  — design-first, test-first phasing
- [`docs/skills/testing.md`](../skills/testing.md) — BDD and unit
  test conventions; `@wip` filter (§2.7)
- [`docs/plans/plate-solver.md`](plate-solver.md) — closest
  precedent for an eight-phase, design-first plan in this workspace
- [`docs/plans/image-evaluation-tools.md`](image-evaluation-tools.md)
  §"Phase 6c-prep — Telescope (mount) primitives" — prerequisite for
  end-to-end exercise of the planner output
- ASCOM Telescope `SiteLatitude` / `SiteLongitude` /
  `CanGetSiteLatitude` / `CanGetSiteLongitude` properties — the
  in-repo reference [`docs/references/ascom-alpaca.md`](../references/ascom-alpaca.md)
  does not yet cover the Telescope interface; see the upstream Alpaca
  device spec at
  [ascom-standards.org/api](https://ascom-standards.org/api/) (Telescope
  endpoints `/sitelatitude`, `/sitelongitude`, `/cangetsitelatitude`,
  `/cangetsitelongitude`). Phase 4 should extend the in-repo reference
  with a Telescope section as part of the work.
- [erfars on crates.io](https://crates.io/crates/erfars) — Rust FFI
  binding for ERFA
- [ERFA project (liberfa/erfa)](https://github.com/liberfa/erfa) —
  the upstream BSD-licensed C library
- [IAU SOFA](https://www.iausofa.org/) — the canonical reference
  ERFA derives from
- [tzf-rs on crates.io](https://crates.io/crates/tzf-rs)
- [openNGC dataset](https://github.com/mattiaverga/OpenNGC) (candidate
  catalog source; CC-BY-SA-4.0)
