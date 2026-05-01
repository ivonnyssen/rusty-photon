# `fitsio-pure` Spike

## Status

**Open / not yet started.** Third investigation for issue
[#107](https://github.com/ivonnyssen/rusty-photon/issues/107),
following:

- [`fits-spike.md`](fits-spike.md) — fitsrs reads (green; no writer)
- [`fits-cfitsio-vcpkg-spike.md`](fits-cfitsio-vcpkg-spike.md) — stock
  fitsio + vcpkg CFITSIO on Windows (green; non-reentrant CFITSIO is
  incompatible with QHY600 parallel-write throughput)

A new candidate surfaced after the first two spikes converged on
"fitsrs reads + hand-rolled writer": [`fitsio-pure`](https://crates.io/crates/fitsio-pure),
a pure-Rust FITS reader **and writer** published 2026-02-15 by
`OrbitalCommons/meawoppl`. If it works for our consumer profiles, it
collapses the wrapper-crate's job from "own a hand-rolled writer
(~500 LOC) plus a fitsrs read facade" to "thin atomic-write helper
plus DOC_ID convention" — substantially less code, single upstream
to track.

## Goal

Decide, in a time-boxed pass, whether `fitsio-pure 0.11.0` covers all
three call-site profiles cleanly and performs well enough on QHY600-
sized writes. Surface every gap with severity. Output is a one-page
report, not a migration.

## Time box

1–2 working days. If the answer is not converging by day 2, stop and
write up what was found.

## Why this is worth doing now

The ADR-001 amendment on `chore/adr-001-fits-amendment` (not yet
pushed) is parked specifically pending this spike. If fitsio-pure
clears, the amendment's "hand-rolled writer" recommendation becomes
unnecessary. If it doesn't, we land the amendment as written.

## Risks going in

- **Maintainer risk.** Single contributor, project is 3 months old,
  last commit 71 days ago (2026-02-19). Could easily go dormant. The
  wrapper crate insulates us from this — we can swap fitsio-pure for
  fitsrs+hand-rolled later — but it's worth pricing in.
- **Untested by us.** No production usage anywhere in our workspace.
- **Performance unknown for QHY600 scale.** README claims "within 3x
  of cfitsio on large images, ~10x at 1M rows for column writes."
  Image-only writes at 122 MB are the unknown.
- **Trace adoption.** 566 lifetime downloads, 1 GitHub star, 0
  external contributors.

## Setup

- New workspace member: `crates/fitsio-pure-spike/`. Throwaway —
  deleted before the wrapper-crate work merges, or kept only as a
  CI canary if useful. Workspace member rather than excluded: the
  crate is pure Rust with no system dependencies, so adding it to
  the workspace doesn't break local `cargo build` for contributors
  without CFITSIO.
- `fitsio-pure = "0.11"` is a **dev-dependency of
  `crates/fitsio-pure-spike` only.** No workspace-level commitment.
- Spike runs as `cargo test -p fitsio-pure-spike` on linux + macos +
  `x86_64-pc-windows-msvc`.
- Branch: `feature/fitsio-pure-spike`.

## Test design

A single `tests/spike.rs` exercises fitsio-pure's read+write surface
against the three consumer profiles. **All assertions use `unwrap()`**
per CLAUDE.md rule 7.

### Read profile (sky-survey-camera, the load-bearing case)

1. **`b1_sky_survey_bitpix16_bzero_unsigned_range`** — synthesise a
   `2×2` BITPIX=16 + BSCALE=1.0 + BZERO=32768.0 fixture with i16
   bytes encoding `[0, 32768, 65535, 12345]`. Parse via `parse_fits`,
   read with `read_image_physical`, assert pixels round-trip back to
   `[0.0, 32768.0, 65535.0, 12345.0]` in physical units. *This is
   THE test: fitsrs and fitrs both fail this without manual scaling
   at the call site; fitsio-pure should pass.*
2. **`b2_skyview_float_bscale_bzero`** — same fixture shape with
   BITPIX=-32 + BSCALE=2.0 + BZERO=5.0. Verify physical-unit reads
   apply both factors.
3. **`b3_parse_from_byte_buffer_no_filesystem`** — sanity check: the
   read API takes `&[u8]` end-to-end. No tempfile.

### Write profile

4. **`a1_rp_round_trip_i32_image_with_doc_id`** — BITPIX=32 i32
   image with custom `DOC_ID` string keyword. Build cards
   programmatically, serialise header + image data, write to a
   tempfile, reopen via `parse_fits`, walk cards to find `DOC_ID`,
   assert round-trip.
5. **`c1_phd2_u16_via_bzero_writes`** — write a small u16 image
   using `build_image_hdu_with_scaling(bitpix=16, bscale=1.0,
   bzero=32768.0, ...)`. Read it back, verify the unsigned range
   round-trips. **This restores the native u16 storage that the
   original ADR-001 conceded couldn't be done with `fitrs`.**

### Multi-HDU + structural

6. **`d1_multi_hdu_iteration`** — build a fixture with primary
   (NAXIS=0) plus one image extension. `fits.len() == 2`, both
   appear in `fits.iter()`.

### Performance (the QHY600 question)

7. **`e1_qhy600_scale_write_smoke`** — emit a `9576×6388` u16
   image (~122 MB raw, ~131 MB FITS-padded), write to tempfile,
   read it back, time both. Assert: write + read complete in under
   60 seconds total wall-clock. Print the timing under
   `cargo test -- --nocapture`. Loose bound — we care about "fast
   enough", not micro-perf. *If this fails the spike is over: a
   library that takes minutes to write a single QHY600 frame is
   not viable.*

### Build smoke

8. **`f1_compiles_on_target_smoke`** — empty test. Forces fitsio-pure
   to be compiled+linked on whichever target CI is running. Pure
   Rust so should be trivial on every platform.

## Open questions to resolve

| # | Question | How the spike answers it |
|---|---|---|
| 1 | Does `read_image_physical` actually apply BSCALE/BZERO automatically? | Test b1, b2 succeed if it does, fail if it doesn't |
| 2 | Does `build_image_hdu_with_scaling` accept i16 input + BZERO=32768 to encode u16? | Test c1 |
| 3 | Custom string keyword API on writes — does the header card builder accept arbitrary keywords? | Test a1 |
| 4 | Multi-HDU iteration | Test d1 |
| 5 | Time to write a 122 MB u16 image | Test e1 timing print |
| 6 | Does it build on `x86_64-pc-windows-msvc` without setup? | Test f1 + CI matrix |
| 7 | Maintainer responsiveness if we hit a real bug | Out of scope for this spike — would surface during migration |

## Decision gates

- **All eight tests green:** fitsio-pure clears. Update ADR-001
  amendment to recommend "fitsio-pure for everything; fitsrs and
  hand-rolled writer remain documented fallbacks". Wrapper crate
  becomes a thin facade (~150 LOC) instead of owning a writer.
- **Test b1 fails (BSCALE/BZERO read):** fitsio-pure is out — the
  load-bearing read case is broken. Fall back to current ADR-001
  amendment plan (fitsrs reads + hand-rolled writer).
- **Test c1 fails (u16 write):** fitsio-pure is partially viable
  but we still hand-roll u16 writes. Reduces but does not eliminate
  the writer module.
- **Test e1 fails (QHY600 perf):** fitsio-pure is out for our
  workload. Same fallback as b1.
- **Test f1 fails (Windows build):** fitsio-pure is out. Fall back.

## Out of scope

- The wrapper crate (`crates/rp-fits`) — gated on the consolidated
  decision after this third spike.
- Migrating any consumer.
- Performance work beyond the single timing assertion in e1.
- Stress-testing concurrent fitsio-pure writes — pure Rust, no shared
  globals, the question is academic. Trust the architecture until
  proven otherwise.
- Evaluating fitsio-pure's `compat` feature (drop-in replacement for
  the `fitsio` crate). Not needed because none of our consumers
  currently use `fitsio`.

## Deliverables

1. PR `feature/fitsio-pure-spike` containing
   `crates/fitsio-pure-spike/`, the CI workflow, and this plan doc.
2. **`docs/plans/fitsio-pure-spike-report.md`** — one page. Each
   open question gets a one-paragraph answer with code-link or CI
   evidence. Ends with the decision-gate outcome.
3. The PR body links the report and quotes the recommendation. CI
   shows all three platforms with their actual outcome.
4. **Either**: an ADR-001 amendment update (if fitsio-pure clears),
   **or**: green light to land the existing amendment unchanged.

## References

- Crate: [`fitsio-pure` on crates.io](https://crates.io/crates/fitsio-pure)
- Source: [`OrbitalCommons/fitsio-pure`](https://github.com/OrbitalCommons/fitsio-pure)
- Issue: [#107 Pick a workspace FITS library](https://github.com/ivonnyssen/rusty-photon/issues/107)
- Companion plans: [`fits-spike.md`](fits-spike.md),
  [`fits-cfitsio-vcpkg-spike.md`](fits-cfitsio-vcpkg-spike.md)
- Companion reports: [`fits-spike-report.md`](fits-spike-report.md),
  [`fits-cfitsio-vcpkg-spike-report.md`](fits-cfitsio-vcpkg-spike-report.md)
- Parked ADR amendment: `chore/adr-001-fits-amendment` branch
