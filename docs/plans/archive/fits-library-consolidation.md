# FITS Library Consolidation

## Status

**Superseded by [ADR-001 Amendment A](../decisions/001-fits-file-support.md).**
The workspace now consolidates FITS I/O in `crates/rp-fits` (reads via
`fitsrs`, writes via a hand-rolled pure-Rust BITPIX 8/16/32 writer).
Both `fitrs` and the hand-rolled sky-survey-camera parser have been
retired. This document is retained as the historical audit that
informed the ADR.

## Why this exists

As of `feature/sky-survey-camera` (PR #105), the workspace has **two**
hand-rolled FITS code paths:

| Location | Purpose | Direction |
|---|---|---|
| `services/rp/src/persistence/fits.rs` | Persist captured images on disk; resolve `DOC_ID` from header during disk-fallback | read + write |
| `services/sky-survey-camera/src/fits.rs` | Parse SkyView HTTP responses into `ImageArray` | read-only, in-memory |

`rp` wraps the workspace-pinned `fitrs = "0.5.0"` crate and works
around several of its limitations.
`sky-survey-camera` evaluated `fitrs` while building the SkyView client
and rejected it in favour of a small bespoke parser; the rationale is
recorded below so we don't have to redo the analysis next time.

A third FITS consumer is likely (any future image-analysis or
calibration plugin), and at that point sharing a single FITS layer
across the workspace will pay off — but only if the chosen library
covers the actual shapes our consumers need.

## What our consumers need

Use this checklist to evaluate any candidate library:

- [ ] **Read from `&[u8]` / `Read`, not just a path.** sky-survey-camera
      receives FITS bytes from a `reqwest` response; forcing them
      through a tempfile is slow and adds a runtime tempdir
      requirement.
- [ ] **Apply `BSCALE` and `BZERO` on read.** SkyView commonly emits
      `BITPIX = 16` with `BZERO = 32768` (the unsigned-16 convention).
      A library that ignores those keys hands back values shifted by
      −32768.
- [ ] **`BITPIX = 8 | 16 | 32 | -32 | -64`.** SkyView, DSS2, 2MASS,
      WISE, etc. each pick a different BITPIX. Float (-32) is common.
- [ ] **`u16` writes.** rp captures `u16` natively; today it widens to
      `i32` only because `fitrs` won't accept `u16` on write.
- [ ] **Atomic write helpers** *or* clear control over file handles
      (rp's atomic-write dance is `stage → fsync → rename → fsync
      parent dir` and depends on the lib not closing the file out of
      our control).
- [ ] **WCS read.** The deferred local-resampling work (see the
      sky-survey-camera *Future Work* section) needs sky↔pixel
      transforms from header keys (`CTYPE*`, `CRVAL*`, `CRPIX*`,
      `CD*_*`, `PV*_*`).
- [ ] **`BLANK` pixel handling** — preferably surfaced as `Option<T>`
      with a documented fallback so consumers don't accidentally treat
      the sentinel as data.
- [ ] **Multi-HDU iteration.** Most consumers will only ever look at
      the primary HDU, but the library should not error out on files
      that have extensions.
- [ ] **No-std-friendly or at least Rust-2021 + sane MSRV.** Workspace
      MSRV is `1.94.1` (root `Cargo.toml`).

## Why `fitrs 0.5.0` falls short for sky-survey-camera

Confirmed against `~/.cargo/registry/src/.../fitrs-0.5.0/src/fits.rs`
plus rp's documentation of empirical behaviour:

1. **No `BSCALE` / `BZERO` scaling on read.**
   `services/rp/src/persistence/fits.rs:155–157`:
   > `fitrs` always returns `IntegersI32` for any `BITPIX = 32` (and
   > `BITPIX = 16`) file — it does not inspect `BZERO`/`BSCALE`. Files
   > written as "u32" (`BITPIX = 32 + BZERO = 2147483648`) would
   > arrive here with values shifted by `2_147_483_648`.
   For sky-survey-camera this is fatal: SkyView's BITPIX=16 + BZERO=32768
   responses would render as `[-32768, 32767]` instead of `[0, 65535]`.
2. **Path-only read API.** `Fits::open<P: AsRef<Path>>` only — no
   `from_bytes` or `Read`-based constructor. We'd write SkyView
   responses to a tempfile per cache miss, which adds disk I/O on the
   hot path and forces every test that exercises the parser to do
   real filesystem work.
3. **`FitsData::IntegersI32(FitsDataArray<Option<i32>>)`.** Pixel data
   is `Option<i32>` because of the `BLANK` convention. Convenient for
   correctness, awkward when downstream needs a plain `i32` slice for
   `ndarray::Array2::from_shape_vec`.
4. **No `u16` write support.** Forces rp to widen on write; for any
   future writer of unsigned 16-bit data this remains an issue.
5. **Crate health.** `fitrs 0.5.0` was published in 2020 and has not
   seen significant updates since. Last-known-good but not actively
   maintained.

The hand-rolled `services/sky-survey-camera/src/fits.rs` covers items
1, 2, 3 (returns `Vec<i32>`), and partially 4 (read-only). It deliberately
does NOT cover items 6 (WCS), 7 (BLANK), or 8 (multi-HDU). The trade
was deliberate: `parse_primary_hdu` is ~200 lines vs the fitrs wrapper
that would still need to read `BSCALE`/`BZERO` manually anyway.

## Candidate libraries to investigate

Listed in rough order of how worth-a-look they seem; none have been
field-tested for our requirements yet.

- **`fits-rs`** (different crate from `fitrs`). Newer, has both Read- and
  Path-based constructors. Worth evaluating against the checklist above.
- **`astrors`** — astronomy-focused Rust toolkit; includes FITS, WCS,
  and several utility primitives. Heavier dependency footprint.
- **`fitsio`** — Rust bindings to the canonical CFITSIO C library.
  Pros: feature-complete, battle-tested, full WCS via cfitsio's WCS
  helpers. Cons: C dependency (`libcfitsio-dev`); cross-compilation /
  static-binary work for the `.deb` and `.msi` packaging paths needs
  thought; OS-specific availability of `cfitsio` headers in CI.
- **Stay on `fitrs`, contribute upstream.** PR `BSCALE`/`BZERO`
  scaling and a `Read`-based constructor. Lowest disruption if the
  maintainer is responsive; uncertain because of the inactivity.
- **Build a workspace-internal `crates/rp-fits`** that wraps whichever
  third-party lib wins (or stays hand-rolled) and provides the exact
  surface our consumers need: `parse_primary(&[u8]) → (Header, Image)`,
  `write_atomic(path, image, header) → Result<()>`, `Wcs::from(header)`.
  This is the most defensive option and is independent of which
  underlying library we pick.

## Recommended next step

Before any consolidation work:

1. Write a one-page "what does each call site actually use?" audit
   covering rp's read + write + DOC_ID extraction, sky-survey-camera's
   primary-HDU read, and any imminent third consumer (e.g. calibrator
   plugins). The checklist above is the template.
2. Spike a single library against that audit — `fits-rs` or `fitsio`
   are the lowest-risk first stops based on what we know today.
3. If the spike clears the checklist, propose a `crates/rp-fits` wrapper
   as the migration vehicle so individual services can switch incrementally
   without a workspace-wide flag day.

Until then, the two hand-rolled paths are documented, narrow, and
tested. The cost of duplication is bounded; the cost of picking the
wrong shared library would not be.

## References

- `services/rp/src/persistence/fits.rs` — fitrs wrapper, atomic-write
  dance, DOC_ID handling, the empirical fitrs limitations comment at
  L155–157.
- `services/sky-survey-camera/src/fits.rs` — bespoke `parse_primary_hdu`
  with `BSCALE`/`BZERO` and BITPIX 8/16/32/-32/-64 support.
- PR #105 review thread on `fitrs` evaluation (2026-05-01).
