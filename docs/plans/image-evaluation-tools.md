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

- [ ] Crate adds: `ndarray-ndimage` (workspace dep — used by stars,
      background, smoothing). Update `Cargo.toml` (workspace + rp).
      Run `CARGO_BAZEL_REPIN=1 bazel mod tidy` per CLAUDE.md rule 10.
- [ ] Promote `services/rp/src/imaging.rs` → `services/rp/src/imaging/`.
      Move existing `compute_stats`, `write_fits`, `read_fits_pixels`
      into `stats.rs` / `fits.rs`. Keep public API stable.
- [ ] `pixel.rs`: `Pixel` trait with `u16` and `i32` impls.
- [ ] `cache.rs`: `CachedPixels` enum, `CachedImage` struct,
      `ImageCache` with LRU and MiB-based eviction. Internal-only at
      this stage.
- [ ] Capture path (`mcp.rs:capture`) inserts into the cache after FITS
      write, narrowing to `u16` when `max_adu ≤ 65535`.
- [ ] `imaging` config block: `cache_max_mib`, `cache_max_images`,
      with sensible Pi-5 defaults.

**Exit criteria:** capture populates the cache; `compute_image_stats`
still passes its existing BDD; `cargo rail run --merge-base -q --` clean.

### Phase 4 — Implement `measure_basic`

- [ ] Crate add: `rmpfit` (workspace dep). *Not yet used by
      `measure_basic` — `rmpfit` is for FWHM in Phase 5. Defer adding
      until then if it keeps this PR smaller.*
- [ ] `background.rs`: sigma-clipped mean/stddev/median.
- [ ] `stars.rs`: Gaussian smoothing (via `ndarray-ndimage`) →
      thresholding → connected components → component filtering
      (area, saturation, border) → centroiding.
- [ ] `hfr.rs`: per-star radial flux accumulation to half-max.
- [ ] `measure_basic.rs`: compose the above; output the contract fields.
- [ ] MCP tool wiring in `mcp.rs`: accept either `document_id` or
      `image_path` plus optional `threshold_sigma`. Resolve via cache
      first, fall back to FITS read on miss.
- [ ] Document section persistence: write `image_analysis` section to
      the exposure document when called with `document_id`.
- [ ] HTTP cache endpoints: `GET /api/images/{document_id}` (JSON
      metadata) and `/pixels` (Alpaca ImageBytes). Wire into `routes.rs`.
- [ ] Unit tests on imaging primitives (synthetic in-test FITS with
      controlled star list — exact-value assertions; per
      `docs/skills/testing.md` §1.2).

**Exit criteria:** all `measure_basic.feature` scenarios green; unit
tests cover background/stars/hfr exact behavior; full `cargo rail run
--merge-base` clean; `cargo fmt`.

### Phase 5 — Subsequent image-analysis tools

One PR per tool, in this order:

- [ ] `estimate_background` (extracted from `measure_basic`'s shared
      code; adds median to the output).
- [ ] `detect_stars` (extracted similarly; returns per-star list).
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
