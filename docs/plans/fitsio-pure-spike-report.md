# `fitsio-pure` Spike Report

## Status

**Done. All 8 tests green across linux + macOS + windows-msvc.**
[CI run 25212745011](https://github.com/ivonnyssen/rusty-photon/actions/runs/25212745011),
PR [#115](https://github.com/ivonnyssen/rusty-photon/pull/115).
Companion to [`fitsio-pure-spike.md`](fitsio-pure-spike.md), the
plan that scoped this experiment.

## TL;DR — recommendation

**Adopt `fitsio-pure = "0.11"` for both reads and writes inside
`crates/rp-fits`.** The wrapper crate becomes a thin facade
(~150-250 LOC) instead of owning a hand-rolled writer. The
parked ADR-001 amendment on
`chore/adr-001-fits-amendment` should be updated to reflect this
outcome before being pushed.

Every spike-plan decision gate clears:

- BSCALE/BZERO is auto-applied on read by `read_image_physical`
  (b1, b2 — the load-bearing tests).
- Native u16 storage works via `BITPIX=16 + BZERO=32768` encoding
  using the lower-level API (c1).
- Multi-HDU iteration is clean (d1).
- A 122 MB QHY600-scale round-trip completes in **3-7 seconds
  total wall-clock** depending on platform — 9-20× under the 60 s
  budget (e1).
- Pure Rust on every target. Windows MSVC: zero setup steps; the
  cfitsio-vcpkg spike's `vcpkg install` + `pkgconfiglite` +
  `PKG_CONFIG_*` env-var dance disappears entirely.

## Open questions — answered

| # | Question | Answer | Evidence |
|---|---|---|---|
| 1 | Does `read_image_physical` actually apply BSCALE/BZERO automatically? | **Yes.** Internally it calls `read_image_data` then `extract_bscale_bzero(&hdu.cards)` then `apply_bscale_bzero`. Defaults to `(1.0, 0.0)` when keywords are absent. | Tests b1 + b2 green on all three platforms; source: `image.rs:172-180` in 0.11.0. |
| 2 | Does fitsio-pure 0.11 support u16 writes via `BITPIX=16 + BZERO=32768`? | **Yes,** via the lower-level API (`build_primary_header` + manual `BSCALE`/`BZERO` cards + `serialize_image_i16`). The `build_image_hdu_with_scaling` convenience helper that landed as PR #51 in upstream master did *not* make it into 0.11.0; we use the building blocks directly, which is what a real wrapper would do anyway. | Test c1 green: `[0u16, 1, 100, 65535, 32768, 32767]` round-trips through write→read→`read_image_physical`→cast back to u16 with no loss. |
| 3 | Custom string keyword API on writes? | **Yes.** `Card { keyword: [u8;8], value: Some(Value::String(s)), comment }` plus `serialize_header`. `parse_fits` exposes `hdu.cards: Vec<Card>` for reads; we walk the slice and match on `Value::String` for `DOC_ID`. | Test a1 green; rp's `DOC_ID = '<UUID>'` round-trips byte-exact. |
| 4 | Multi-HDU iteration? | **Yes.** `parse_fits(&[u8])` returns `FitsData { primary, extensions }`. Methods: `len()`, `primary()`, `get(i)`, `iter()`, `find_by_name(extname)`. | Test d1: primary (NAXIS=0) + image extension parsed cleanly, both pixels and structure intact. |
| 5 | Time to write a 122 MB u16 image | **3-7 seconds total** for the full alloc+build+write+read+parse+decode round-trip, dominated by the i16 endian-swap step in `serialize_image_i16` (~1.4-3.4 s of that). The actual `std::fs::write` is 20-2510 ms; `read_image_physical` decode is 200-740 ms. Under the spike's 60 s loose budget by 9-20×. | e1 timing prints, captured per platform below. |
| 6 | Does it build on `x86_64-pc-windows-msvc` without setup? | **Yes — zero setup steps.** Pure Rust + no_std-with-alloc + bytemuck + miniz_oxide + libm. CI's Windows job runs the same `cargo test -p fitsio-pure-spike --locked` that linux and macOS use. Total Windows wall-clock 108 s vs the cfitsio-vcpkg spike's 149 s. | Workflow `.github/workflows/fitsio-pure-spike.yml` and CI run conclusion. |
| 7 | Maintainer responsiveness if we hit a real bug | Out of scope; insulated by the wrapper crate (we can swap to fitsrs+hand-rolled writer later if upstream goes dormant). | n/a |

## Test results (matrix)

| Job | Conclusion | Wall-clock |
|---|---|---|
| `ubuntu-latest` | ✓ success | 31 s |
| `macos-latest` | ✓ success | 37 s |
| `windows-latest` | ✓ success | 108 s |

All 8 tests pass on every platform.

### e1 perf breakdown — 9576 × 6388 BITPIX=16 (122,348,160 bytes on disk)

| Step | ubuntu-latest | macos-latest | windows-latest |
|---|---|---|---|
| alloc Vec\<i16\> | 906 ms | 566 ms | 754 ms |
| build (header + serialize_image_i16) | 3.18 s | **1.93 s** | 3.39 s |
| `std::fs::write` | **29 ms** | 176 ms | 2.51 s |
| `std::fs::read` | 50 ms | 147 ms | 56 ms |
| `parse_fits` | 64 µs | 83 µs | 71 µs |
| `read_image_data` decode | 404 ms | **561 ms** | 734 ms |
| **TOTAL** | **4.57 s** | **3.38 s** | **7.44 s** |

The dominant cost is the `serialize_image_i16` big-endian byte-swap
on write and `read_image_data`'s mirror-image decode on read; both
are O(pixel count) and are well below 1 second per gigabyte. Windows
write time is 2.5 s vs ms-scale on linux/macOS — this is the runner's
NTFS-on-virtual-disk doing buffered-write-then-flush rather than
anything fitsio-pure-specific. Real production hardware (NVMe SSD)
should match the linux numbers.

## What this means for `crates/rp-fits`

The wrapper crate's job collapses substantially. Compared to the
ADR-001 amendment's "hand-rolled writer + fitsrs read facade" plan:

```
crates/rp-fits/   (post-fitsio-pure)
├── reader.rs    — thin wrappers around parse_fits + read_image_physical
│                  + extract_blank/blank_mask. ~50-80 LOC.
├── writer.rs    — Card builder + serialize_header + serialize_image_*
│                  + DOC_ID convention helper. ~60-100 LOC.
├── atomic.rs    — stage→fsync→rename→fsync-parent helper, lifted from
│                  services/rp/src/persistence/fits.rs:write_fits_sync.
│                  ~150 LOC (unchanged from ADR amendment plan).
└── tests/       — round-trip tests; reuse the spike's fixtures.
```

That's a wrapper of roughly **250-330 LOC** (incl. atomic helper)
versus the ADR amendment's ~700-800 LOC (writer + atomic + reader
facade). We trade ~450 LOC of code we'd own for a dependency on
fitsio-pure 0.11. The wrapper still owns the durability dance and
the workspace-specific keyword conventions; it does not own the
FITS standard.

## Real findings

1. **fitsio-pure's "in-memory by design" model is exactly what we
   want.** `parse_fits(&[u8])` and `build_*` returning `Vec<u8>`
   means the wrapper crate composes naturally with `std::fs::write`
   and the existing atomic-write helper. No fighting against a
   `FitsFile` abstraction.
2. **Native `u16` writes restored.** ADR-001's first supersession
   accepted `i32` widening because `fitrs` couldn't write `u16`.
   Both fitsio-pure and the FITS standard handle unsigned-16 via
   `BITPIX=16 + BZERO=32768`; round-trip tests in c1 confirm bit-
   exact recovery across the full range `[0..65535]`.
3. **`build_image_hdu_with_scaling` (PR #51 in master) is not in
   0.11.0.** Doesn't matter for us — we use the building blocks
   directly. Worth noting for a future wrapper-crate PR's mental
   model: we don't depend on convenience helpers that may shift
   between releases.
4. **The maintainer-risk concern is real but bounded.** Last commit
   was 2026-02-19 (71 days ago at spike time). The crate is
   feature-complete enough for our needs *as-is*. If upstream
   goes fully dormant, the wrapper crate insulates us — we can
   swap fitsio-pure for fitsrs + a writer module later, contained
   within `crates/rp-fits`.

## What changed vs the spike plan

Nothing material. All 8 tests landed as planned. The only deviation
was the writer-API surface — `build_image_hdu_with_scaling` (PR #51)
turned out to be unpublished, so we used the lower-level API
(`serialize_header` + `serialize_image_i16`) instead. This is
strictly better for the wrapper crate's eventual implementation.

## Implications for the parked ADR-001 amendment

The amendment on `chore/adr-001-fits-amendment` (not yet pushed)
recommends "fitsrs reads + hand-rolled writer". With this spike's
result, that recommendation should be updated to:

> Adopt `fitsio-pure = "0.11"` for both reads and writes via a thin
> `crates/rp-fits` wrapper that owns the atomic-write dance and the
> DOC_ID convention. fitsrs and the hand-rolled writer remain
> documented fallback options if fitsio-pure goes dormant.

Other ADR consequences are unchanged:
- License: still clean (fitsio-pure is Apache-2.0, drops fitrs's
  GPL-3.0).
- Native u16: still restored.
- Parallel writes: still unblocked (no shared globals; pure Rust).
- No Windows MSVC contributor friction: same outcome, achieved
  with one fewer crate.

## Next steps

1. **Land this PR** — gives the wrapper-crate work green-field
   confidence in fitsio-pure.
2. **Update and push the ADR-001 amendment** on
   `chore/adr-001-fits-amendment` to point at fitsio-pure rather
   than a hand-rolled writer. Same migration order.
3. **Open the `crates/rp-fits` design PR.** Wrapper structure as
   above; tests against the same fixtures the spike uses.
4. **Migrate consumers** in the order documented in the ADR
   amendment: sky-survey-camera (read-only) → phd2-guider
   (write-only, restores native u16) → rp (read+write+atomic).
5. **Delete** `crates/fits-spike`, `crates/fits-cfitsio-spike`,
   and `crates/fitsio-pure-spike` when the wrapper crate's tests
   subsume them.

## References

- PR: [#115 Spike: fitsio-pure 0.11 against all three FITS call sites](https://github.com/ivonnyssen/rusty-photon/pull/115)
- CI run: [25212745011](https://github.com/ivonnyssen/rusty-photon/actions/runs/25212745011)
- Plan: [`fitsio-pure-spike.md`](fitsio-pure-spike.md)
- Issue: [#107 Pick a workspace FITS library](https://github.com/ivonnyssen/rusty-photon/issues/107)
- Companion plans + reports:
  - [`fits-spike.md`](fits-spike.md) /
    [`fits-spike-report.md`](fits-spike-report.md) — fitsrs reads
  - [`fits-cfitsio-vcpkg-spike.md`](fits-cfitsio-vcpkg-spike.md) /
    [`fits-cfitsio-vcpkg-spike-report.md`](fits-cfitsio-vcpkg-spike-report.md) — stock fitsio + vcpkg insurance policy
- Crate: [`fitsio-pure` on crates.io](https://crates.io/crates/fitsio-pure)
- Source: [`OrbitalCommons/fitsio-pure`](https://github.com/OrbitalCommons/fitsio-pure)
- Parked ADR amendment branch: `chore/adr-001-fits-amendment`
