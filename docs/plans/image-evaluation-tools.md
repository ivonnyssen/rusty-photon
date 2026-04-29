# Plan: Image evaluation tools in rp

**Date:** 2026-04-28
**Branch:** `worktree-image-evaluation-tools`

## Background

`rp` ships only `compute_image_stats` today. The MCP catalog already lists
`measure_basic` (HFR, star count, background) but it is not implemented, and
there is no path for the broader image-evaluation toolkit needed by focus,
centering, and quality-screening workflows. This plan adds those tools as
**built-in** capabilities of `rp` per the "batteries included" architecture
clarified during design — see `docs/services/rp.md` (Component Categories
and Image Analysis Strategy).

The rest of the toolkit (`detect_stars`, `measure_stars`, `estimate_background`,
`compute_snr`, plus compound tools `auto_focus` and `center_on_target`) is
defined in `rp.md` as planned. This plan sequences them.

## Goals

1. **MVP:** ship `measure_basic` end-to-end: design → BDD → implementation,
   with the supporting `imaging/` module structure and image cache.
2. Establish the patterns (Pixel trait, cache enum, document-section
   persistence, BDD style) that subsequent tools will reuse.
3. Land subsequent image-analysis tools incrementally, each as its own PR.
4. Land compound built-in tools (`auto_focus`, `center_on_target`) once the
   primitives they need exist.

## Decisions resolved (during design)

These are now in `docs/services/rp.md` and should not be re-litigated:

- **Component categories:** built-in / rp-managed services / plugins.
  Plugin mechanism serves both first-party workflow logic
  (`calibrator-flats`) and third-party extensibility.
- **Cache wire format:** ASCOM Alpaca ImageBytes (type-tagged 44-byte
  header + raw pixels). No `/fits` endpoint — consumers needing FITS
  bytes read the file directly via the path in the exposure document.
- **Cache storage type:** `CachedPixels::U16 | I32` enum from day one.
  `u16` is the primary path (every camera rusty-photon will encounter is
  ≤ 16-bit non-negative); `I32` is a hatch for future scientific cameras
  (Andor, Hamamatsu sCMOS HDR). Selection is per-camera at connect time
  based on `MaxADU`, not per-frame.
- **Cache eviction:** LRU, configured in MiB (`cache_max_mib`) plus an
  image-count safety net (`cache_max_images`). Whichever trips first.
- **FWHM fitting crate:** `rmpfit` (lighter deps, native parameter
  bounds, MPFIT astronomy heritage). Not `levenberg-marquardt`.
- **Module structure:** `imaging.rs` → `imaging/` with submodules
  (`mod.rs`, `pixel.rs`, `fits.rs`, `cache.rs`, `stats.rs`,
  `background.rs`, `stars.rs`, `hfr.rs`, `fwhm.rs`, `snr.rs`,
  `measure_basic.rs`).

## Phases

### Phase 1 — Design doc updates ✓

Status: **complete on this branch.**

- [x] Component Categories section in Architecture
- [x] Tool Catalog reframed as three sources
- [x] Built-in Tools tables expanded with image-analysis and compound rows
- [x] Image Analysis Strategy: rmpfit pinned, `measure_basic` MVP contract
- [x] Image Cache section: internal API (`CachedPixels` enum), HTTP API,
      MiB-based eviction, ImageBytes wire choice
- [x] Plugin-Provided Tools and Plugin Types reframed (first-party vs
      third-party)
- [x] Plate Solver and Guider Service relabeled as rp-managed services
- [x] Compound Tools section reframed as in-process pattern
- [x] Configuration: removed vcurve-focus / iterative-centering from the
      plugin list; added `imaging.cache_max_mib` and `cache_max_images`
- [x] API Layer: added `/api/images/{document_id}` and `/pixels`
- [x] Module Structure updated for `imaging/`

### Phase 2 — BDD scenarios for `measure_basic`

Status: **complete.**

- [x] `services/rp/tests/features/measure_basic.feature` (8 scenarios:
      catalog, image_path happy path, document_id happy path, document
      section persistence, high-threshold zero-stars, three error paths)
- [x] `services/rp/tests/bdd/steps/measure_basic_steps.rs` — step
      definitions reusing shared steps from `tool_steps.rs` (capture,
      list tools, error assertions)
- [x] `RpWorld` additions: `last_measure_basic_result`,
      `last_exposure_document` (for the section-fetch step)
- [x] Wired into `tests/bdd/steps/mod.rs`
- [x] `@wip` tag on the feature + `filter_run` in `bdd.rs` — keeps the
      default suite green until Phase 4 implementation lands. The
      convention is documented in `docs/skills/testing.md` §2.6 and
      `docs/skills/development-workflow.md` Phase 2d. **Removing the
      `@wip` tag is the explicit Phase 4 completion milestone.**

**Exit criteria met:** `cargo build --all-features --all-targets -p rp`
clean; `cargo test --all-features --test bdd -p rp` passes 82/82
non-`@wip` scenarios. The 8 `measure_basic` scenarios are filtered out
and will fail correctly once enabled in Phase 4.

### Phase 3 — `imaging/` module promotion + image cache

Status: **complete.**

- [x] Crate adds: `ndarray-ndimage` (workspace dep — used by stars,
      background, smoothing). Update `Cargo.toml` (workspace + rp).
      Run `CARGO_BAZEL_REPIN=1 bazel mod tidy` per CLAUDE.md rule 10.
- [x] Promote `services/rp/src/imaging.rs` → `services/rp/src/imaging/`.
      Move existing `compute_stats`, `write_fits`, `read_fits_pixels`
      into `stats.rs` / `fits.rs`. Keep public API stable.
- [x] `pixel.rs`: `Pixel` trait with `u16` and `i32` impls.
- [x] `cache.rs`: `CachedPixels` enum, `CachedImage` struct,
      `ImageCache` with LRU and MiB-based eviction. Internal-only at
      this stage.
- [x] Capture path (`mcp.rs:capture`) inserts into the cache after FITS
      write, narrowing to `u16` when `max_adu ≤ 65535`. If `max_adu`
      can't be read, cache insert is skipped (FITS-on-disk fallback
      still works).
- [x] `imaging` config block: `cache_max_mib`, `cache_max_images`,
      with sensible Pi-5 defaults (1024 MiB / 8 images).

**Exit criteria met:** capture populates the cache; `compute_image_stats`
still passes its existing BDD (12 features / 82 scenarios green);
`cargo rail run --merge-base -q --` clean; `cargo clippy -D warnings`
clean. `Pixel` trait and `CachedPixels::I32` are wired but unused —
they exist for Phase 4 (`measure_basic`) and the future scientific
camera hatch respectively. max_adu is fetched per-capture rather than
stashed at connect time — see follow-up note below.

#### Phase 3 follow-up: stash `max_adu` on `CameraEntry`

**Status:** deferred — landed Phase 3 with per-capture fetch.

**What the design says (`docs/services/rp.md` → Image Cache → Storage
Type Selection):** "Read the camera's `max_adu` (ASCOM
`ICameraVx::MaxADU`) at connect time and stash it in the camera's
runtime state." Selection of `CachedPixels::U16` vs `I32` is
"per-camera (driven by capabilities), not per-frame".

**What Phase 3 does instead:** `mcp.rs:capture` calls
`cam.max_adu().await` after every exposure, immediately before
inserting into the cache. The result drives the U16/I32 narrowing
for that frame.

**Why this is fine for now:**
- `max_adu` is one Alpaca request to an in-process client. It is
  cheap relative to FITS write (which already happened) and
  vanishingly small relative to exposure time (seconds to minutes).
- Behavior is identical to the design intent — the same camera
  always reports the same `max_adu`, so the variant choice is in
  practice per-camera even though the lookup is per-frame.
- If `max_adu` fails, cache insert is skipped and the FITS-on-disk
  path absorbs the miss. No correctness regression.

**Trigger to revisit:**
- Profiling shows `max_adu` fetch on the capture hot path (unlikely
  given the above).
- A camera driver returns `max_adu` slowly or unreliably enough that
  the per-frame fetch becomes a robustness issue.
- We add a non-Alpaca camera path that does not expose `max_adu`
  inline — at which point a stashed value is the natural seam.

**Scope when picked up:** add a `max_adu: Option<u32>` field (or
similar) to `CameraEntry` in `services/rp/src/equipment.rs`,
populate it during `connect`, and replace the `cam.max_adu().await`
call in `mcp.rs:capture` with a read from the entry. Small,
self-contained, no test changes beyond updating the equipment
mocks.

### Phase 4 — Implement `measure_basic`

Status: **complete.**

#### Decisions resolved during Phase 4 planning

These are now in `docs/services/rp.md` (MVP `measure_basic` Contract)
and should not be re-litigated:

- **Saturation: flag, don't reject.** Saturated stars carry real signal.
  Bright in-focus stars routinely clip at long exposures, and donut PSFs
  at extreme defocus saturate in their bright annulus. Rejecting them
  would make HFR-vs-focus non-monotonic and break auto-focus. Output
  carries `saturated_star_count` so downstream consumers (focus runs,
  quality screens) apply their own policy.
- **`min_area` / `max_area`: required parameters, no defaults.** Pixel
  area encodes a pixel-scale (arcsec/px) assumption that the tool cannot
  infer from the image alone. `threshold_sigma` keeps its default of
  `5.0` — it's unit-free (multiples of background stddev) and so
  scale-independent.
- **Connected components: hand-rolled 4-connectivity BFS.**
  `ndarray-ndimage` 0.6's `label` is 3D-only with a hard `assert!` on a
  3×3×3 structuring element. Wrapping the 2D mask via `insert_axis` is
  possible but yields a labels grid we'd then re-walk into per-component
  pixel lists anyway — and per-component pixel lists are exactly what
  centroiding and HFR want. A direct BFS producing
  `Vec<Vec<(usize, usize)>>` is the smaller total diff. The crate is
  still used for `gaussian_filter`.
- **`ImageBytesMetadata` layout replicated locally.**
  `ascom-alpaca`'s struct is `pub(crate)`. The 11×i32 LE header is
  small and well-defined; we replicate it in `routes.rs` with a unit
  test pinning the byte layout.
- **Exposure document store does not exist yet.** Before Phase 4 there
  is no `ExposureDocument` type, no in-memory store, no
  `GET /api/documents/{id}`. The original Phase 4 sketch assumed
  persistence existed; it does not. Phase 4 builds the foundation as
  Step 1 so subsequent persistence work has a place to land. Reload
  from sidecars on process restart is a Phase 5 follow-up.

#### Work breakdown (in order)

- [x] **Step 1 — Exposure document store.** New
      `services/rp/src/document.rs` with `ExposureDocument`,
      `DocumentStore::{create, get, put_section}`. Atomic sidecar JSON
      write (`<fits>.json.tmp` → rename) next to each FITS file.
      `mcp.rs:capture` constructs the document after FITS write +
      cache insert. New route
      `GET /api/documents/{document_id}` in `routes.rs` (the BDD
      step at `measure_basic_steps.rs:79` fetches this).
- [x] **Step 2 — `imaging/background.rs`.** Sigma-clipped
      mean/stddev/median over a `Pixel`-generic `ArrayView2`. Iterative
      clip (k=3, max_iters=5) with median via `select_nth_unstable` on
      the surviving set.
- [x] **Step 3 — `imaging/stars.rs`.**
      `gaussian_filter` (σ ≈ 1.0 px) → threshold → 4-connectivity BFS
      labelling → filter (area in `[min_area, max_area]`, border
      rejection — *no* saturation rejection) → intensity-weighted
      centroiding using background-subtracted flux. `Star` carries
      `saturated_pixel_count: u32`.
- [x] **Step 4 — `imaging/hfr.rs`.** Per-star radial flux accumulation
      to half of total flux, with sub-pixel linear interpolation between
      bracketing pixels. `aggregate_hfr` returns the median of per-star
      HFRs; `None` when no stars.
- [x] **Step 5 — `imaging/measure_basic.rs`.** Composes the above into
      `MeasureBasicResult { hfr, star_count, saturated_star_count,
      background_mean, background_stddev, pixel_count }`.
- [x] **Step 6 — MCP tool wiring in `mcp.rs`.** `MeasureBasicParams`
      with `min_area: Option<usize>` / `max_area: Option<usize>`
      (`#[serde(default)]`), optional `threshold_sigma: f64` (default
      5.0), and exclusive `document_id` / `image_path`. The area params
      are required-but-validated-in-body so the tool can produce error
      messages in deterministic input order: `document_id`/`image_path`
      first (the "missing both" error mentions `image_path` per
      `measure_basic.feature:78`), then `min_area`, then `max_area`. If
      the area fields were strictly required at the serde level, serde
      would error first on whichever it deserializes first, breaking
      the error-message ordering. Resolution order: cache hit →
      FITS-on-disk fallback via document `file_path` → error. After
      successful analysis with `document_id`, write the `image_analysis`
      section via `DocumentStore::put_section`.
- [x] **Step 7 — HTTP image endpoints in `routes.rs`.**
      `GET /api/images/{document_id}` (JSON metadata) and
      `/pixels` (`application/imagebytes`: 44-byte header where
      `transmission_element_type` = 8 for `U16`, 2 for `I32`; pixel
      bytes serialized via per-element `to_le_bytes` rather than a
      `bytemuck` cast — avoids adding a new workspace crate). Cache
      miss falls back to FITS decode + serve. *Note: not exercised by
      the 8 `measure_basic` BDD scenarios.*
- [x] **Step 8 — Activate `measure_basic.feature` and round out tests.**
      Remove `@wip` from line 1. Extend `read_fits_pixels` to also
      return `(width, height)` (one caller — `compute_image_stats` —
      updated alongside) so the FITS fallback path can reconstruct
      `Array2`. Bake test-fixture `min_area` / `max_area` into the
      step helper for the OmniSim image. Add unit tests on
      background/stars/hfr/measure_basic with exact-value assertions
      per `docs/skills/testing.md` §1.2.

`rmpfit` is **not** added in this phase — it's deferred to Phase 5
(`measure_stars` / FWHM).

**Exit criteria met:** 90/90 BDD scenarios pass (was 82/82 + 8
`@wip`); 33 new unit tests across `document`, `background`, `stars`,
`hfr`, `measure_basic`, plus 2 in `routes` for ImageBytes header
layout — 101 lib tests total, all green. `cargo rail run --merge-base`
exits 0 with no warnings. `cargo fmt` clean. No new workspace deps,
so no `bazel mod tidy` was needed.

### Phase 5 — Subsequent image-analysis tools

One PR per tool, in this order:

- [x] `estimate_background` (sigma-clipped mean / stddev / median; reuses
      `imaging/background.rs` kernel from Phase 4). Optional `k` (default
      3.0) and `max_iters` (default 5) clip parameters; persists into the
      exposure document as a `background` section, separate from
      `measure_basic`'s `image_analysis`. 10 BDD scenarios (catalog,
      image_path / document_id happy paths, persistence, custom k+iters,
      five error paths). 100/100 BDD scenarios green.
- [x] `detect_stars` (per-star list with `{x, y, flux, peak,
      saturated_pixel_count}` plus aggregate counts and the sigma-clipped
      background used for the threshold). Reuses `imaging/stars.rs` and
      `imaging/background.rs` kernels from Phase 4. `peak: f64` (raw, not
      background-subtracted) was added to `Star` for this tool and to give
      `measure_stars` an FWHM-fit initial guess. Persists into the
      exposure document as a `detected_stars` section. 10 BDD scenarios
      (catalog, image_path / document_id happy paths, persistence,
      high-threshold empty list, five error paths). 110/110 BDD scenarios
      green.
- [ ] `measure_stars` (per-star HFR, FWHM, eccentricity, flux). Adds
      `fwhm.rs` via `rmpfit` — pull the crate in here if not already.
- [ ] `compute_snr` (signal/noise summary).

Each tool follows the same shape: design doc already covers it →
BDD feature file → step defs → unit tests → impl.

### Phase 6 — Compound built-in tools

- [ ] `auto_focus` (V-curve). Drives `move_focuser` + `capture` +
      `measure_basic` in-process. New BDD feature file.
- [ ] `center_on_target` (iterative centering). Drives `capture` +
      `plate_solve` + `sync_mount` + `slew`. Depends on plate-solver
      rp-managed service (separate effort, ADR pending — see
      `docs/services/rp.md` plate-solver note).

## Out of scope / deferred

- **Andor / Hamamatsu / sCMOS HDR support.** The `CachedPixels::I32`
  variant is in place, but the connect-time selection logic and any
  driver-specific wiring are deferred until we have a real device to
  test against.
- **Cache LRU forced-eviction BDD scenario.** A unit test on `cache.rs`
  is the right place for LRU correctness; the end-to-end fallback path
  is exercised implicitly by Phase 4's scenarios.
- **Plate-solver rp-managed service.** A separate ADR is needed for
  ASTAP vs. astrometry.net (rp.md notes this). `center_on_target` is
  blocked on it.
- **SEP / sep-sys.** Considered and rejected during design (LGPL +
  C FFI burden) — see Image Analysis Strategy / Design Rationale.
- **Multi-camera image cache** with per-camera quotas. Single global
  LRU is sufficient for now.

## Dependencies on other workstreams

- **Bazel:** every workspace `Cargo.toml` change needs
  `CARGO_BAZEL_REPIN=1 bazel mod tidy` (CLAUDE.md rule 10). Bazel
  remains shadow during the migration; not a blocker.
- **calibrator-flats:** no impact — it uses `compute_image_stats`,
  not `measure_basic`. Adding new tools doesn't break the existing
  contract.
