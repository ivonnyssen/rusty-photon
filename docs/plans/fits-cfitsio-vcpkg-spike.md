# CFITSIO-via-vcpkg Spike on Windows MSVC

## Status

**Open / not yet started.** Sibling investigation to
[`fits-spike.md`](fits-spike.md) for issue
[#107](https://github.com/ivonnyssen/rusty-photon/issues/107).

The fitsrs spike already established that `fitsrs` covers the
read-side cleanly but has no writer. This spike investigates the
*other* end of the trade space: can we use stock `fitsio` (the
CFITSIO Rust binding) on Windows MSVC by installing CFITSIO via
vcpkg, with **no source patches anywhere** — neither in `fitsio-sys`
nor in CFITSIO itself, neither in our workspace nor upstream?

If yes, we recover full CFITSIO functionality (writer, BINTABLE,
WCS, RICE/HCOMPRESS) on Windows without coordinating with anyone.
If no, the failure mode tells us which fallback to choose
(upstream-PR `fitsio-sys`, fork it, or stay on fitsrs).

## Goal

Prove or disprove this single claim:

> A workspace crate that depends on `fitsio = "0.21"` from crates.io,
> with no `[patch.crates-io]` and no source-level changes anywhere,
> compiles cleanly and produces a working binary on
> `x86_64-pc-windows-msvc` when CFITSIO is supplied by vcpkg and
> located via pkg-config.

Surface every gap with severity. The output is a one-page report,
not a migration.

## Time box

1–2 working days. If the answer is not converging by day 2, stop and
write up what was found.

## Setup

- New crate at `crates/fits-cfitsio-spike/`, **excluded from the
  workspace** via `[workspace] exclude = [...]`. Throwaway — deleted
  before the wrapper-crate work merges, or kept only as a CI canary
  if useful. Excluded so machines without a system CFITSIO install
  don't fail their default `cargo build`; the spike is opt-in and
  CI-driven only.
- `fitsio = "0.21"` is a **dev-dependency of `crates/fits-cfitsio-spike`
  only.** No workspace-level commitment.
- Spike runs as `cargo test --manifest-path
  crates/fits-cfitsio-spike/Cargo.toml` on linux + macos +
  `x86_64-pc-windows-msvc`.
- Branch: `feature/fits-cfitsio-vcpkg-spike`.

## Test design

A single `tests/spike.rs` exercises stock `fitsio`'s end-to-end
read+write surface so we know we're testing real linkage, not a
trivial header-only path:

1. **`creates_and_reads_back_i32_image`** — write a small `2×2` BITPIX=32
   image via `FitsFile::create` + `primary_hdu().write_image`, close the
   file, reopen via `FitsFile::open`, read pixels, assert equality.
2. **`reads_custom_string_keyword`** — same flow, additionally writing
   a `DOC_ID` keyword on write and reading it back on open.
3. **`fitsio_links_smoke`** — empty test that just instantiates a
   `fitsio::FitsFile` against a tempfile to force the C library linkage
   on whichever target CI is running.

The same tests run on linux/macos using a system-installed CFITSIO
(`libcfitsio-dev` / `brew install cfitsio`). Cross-platform parity
confirms the test logic is valid before judging the Windows result.

## Per-platform CI configuration

`.github/workflows/fits-cfitsio-vcpkg-spike.yml`, isolated workflow
that does not block any other CI:

| Platform | CFITSIO source | Setup steps |
|---|---|---|
| `ubuntu-latest` | system | `apt-get install -y libcfitsio-dev pkg-config` |
| `macos-latest` | Homebrew | `brew install cfitsio pkg-config` |
| `windows-latest` | **vcpkg** | use the pre-installed vcpkg at `$VCPKG_INSTALLATION_ROOT`; `vcpkg install cfitsio[pthreads]:x64-windows-static-md`; `choco install pkgconfiglite`; export `PKG_CONFIG_PATH` to vcpkg's `lib/pkgconfig` |

Windows is the only platform where this workflow is actually
informative — linux and macos confirm the test logic.

## Open questions to resolve

| # | Question | How the spike answers it |
|---|---|---|
| 1 | Does vcpkg's `cfitsio:x64-windows-static-md` ship a `cfitsio.pc` file that pkg-config can parse? | If `pkg_config::Config::probe("cfitsio")` succeeds from `fitsio-sys/build.rs`, yes. If it errors, we know the gap and Option 3a is dead — fall back to Option 3b (vcpkg-rs integration in `fitsio-sys`). |
| 2 | Does `static-md` (static lib, dynamic CRT) link cleanly with cargo's MSVC profile, or do we need a different triplet? | Build either succeeds or surfaces a linker error pointing at the right triplet. |
| 3 | Does the resulting `.exe` actually *run*, not just link? | Test 1 round-trips a FITS file; if it links but crashes at runtime (DLL hell, runtime mismatch), test fails on the Windows runner. |
| 4 | What's the cold-cache build time on a fresh runner? | Workflow timing — the vcpkg `cfitsio` install dominates the first run. We want a number for documentation. |
| 5 | Does it stay green on subsequent runs with vcpkg's binary cache? | Run the workflow twice in a row; second run should be much faster. |

## Decision gates

- **All three platforms green:** Option 3a works. Documented recipe
  becomes a candidate for the wrapper-crate write path. Compare
  against the fitsrs spike's read-side recommendation to choose:
  fitsrs-only, fitsio-only, or hybrid.
- **Linux+macOS green; Windows fails at pkg-config probe:** Option 3a
  is dead. Escalate to Option 3b (vcpkg-rs integration in
  `fitsio-sys/build.rs`, requires upstream PR) or Option 1 (conditional
  `USE_PTHREADS` patch).
- **Windows links but binary fails to run:** vcpkg triplet mismatch.
  Try `x64-windows` (full DLL) instead of `x64-windows-static-md`.
  If that also fails, escalate as above.
- **Linux or macOS fails:** test logic itself is wrong; this is a
  spike bug, not a finding. Fix and rerun.

## Out of scope

- The wrapper crate (`crates/rp-fits`) — gated on the consolidated
  decision after both spikes complete.
- Migrating any consumer.
- Any patches to `fitsio-sys` or CFITSIO. The whole point is "no
  source changes".
- Performance comparison vs `fitsrs`. Different question.
- vcpkg manifest mode (`vcpkg.json`) integration. Classic mode
  (`vcpkg install`) is enough for the spike.

## Deliverables

1. PR `feature/fits-cfitsio-vcpkg-spike` containing
   `crates/fits-cfitsio-spike/`, the CI workflow, and this plan doc.
2. **`docs/plans/fits-cfitsio-vcpkg-spike-report.md`** — one page.
   Each open question gets a one-paragraph answer with CI run
   evidence (link to the green/red workflow run). Ends with the
   decision-gate outcome.
3. The PR body links the report and quotes the recommendation. CI
   shows all three platforms with their actual outcome.

## References

- Issue: [#107 Pick a workspace FITS library](https://github.com/ivonnyssen/rusty-photon/issues/107)
- Companion plan: [`fits-spike.md`](fits-spike.md)
- Companion report: [`fits-spike-report.md`](fits-spike-report.md)
- Crate: [`fitsio` on crates.io](https://crates.io/crates/fitsio)
- vcpkg port: [`microsoft/vcpkg/ports/cfitsio`](https://github.com/microsoft/vcpkg/tree/master/ports/cfitsio)
- Historical context: [`docs/decisions/001-fits-file-support.md`](../decisions/001-fits-file-support.md)
- Long-running upstream issue: [`simonrw/rust-fitsio#230 "MSVC?"`](https://github.com/simonrw/rust-fitsio/issues/230)
