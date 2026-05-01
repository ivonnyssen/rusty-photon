# CFITSIO-via-vcpkg Spike Report

## Status

**Done. All three platforms green, reproduced twice.** Spike code lives in
`crates/fits-cfitsio-spike/` (excluded from workspace), CI workflow at
`.github/workflows/fits-cfitsio-vcpkg-spike.yml`. Companion to
[`fits-cfitsio-vcpkg-spike.md`](fits-cfitsio-vcpkg-spike.md), which laid
out the platform matrix.

## TL;DR — recommendation

**Option 3a works. Adopt stock `fitsio = "0.21"` from crates.io for the
write path, with a per-platform CI/dev-env recipe for CFITSIO. No
patches anywhere — not in `fitsio-sys`, not in CFITSIO, not in our
workspace.** Issue [`simonrw/rust-fitsio#230`](https://github.com/ivonnyssen/rusty-photon/pull/113)
(open since Jan 2023, the historical reason ADR-001 abandoned fitsio)
is bypassed for our use case.

This unblocks the `crates/rp-fits` wrapper proposal: combined with the
fitsrs spike's read-side win, we have a complete picture for the write
path that does not require coordinating with `fitsio-sys` upstream.

## Open questions — answered

All evidence cites green CI runs on PR
[#113](https://github.com/ivonnyssen/rusty-photon/pull/113).

| # | Question | Answer | Evidence |
|---|---|---|---|
| 1 | Does vcpkg's `cfitsio:x64-windows-static-md` ship a `cfitsio.pc` file that pkg-config can parse? | **Yes.** The CI's "pkg-config sanity check" step succeeds (`pkg-config --modversion cfitsio` and `pkg-config --libs --cflags cfitsio` both return clean output) and the subsequent `cargo build` resolves the library. | First green Windows run: [25209384843](https://github.com/ivonnyssen/rusty-photon/actions/runs/25209384843) |
| 2 | Does `static-md` (static lib, dynamic CRT) link cleanly with cargo's MSVC profile? | **Yes,** once we drop the `[pthreads]` vcpkg feature. With `[pthreads]`, vcpkg's `cfitsio.lib` archive references `pthread_mutex_lock` etc., and fitsio-sys's pkg-config probe does not emit a transitive `pthreadVCE3.lib` link directive — leaving the MSVC linker with unresolved symbols. Without `[pthreads]`, the archive has no pthread refs and the link is clean. | Failure [25209259691](https://github.com/ivonnyssen/rusty-photon/actions/runs/25209259691) → fix in commit `ddff1f1` → green run above. |
| 3 | Does the binary actually *run*, not just link? | **Yes,** when tests run serially. Without `--test-threads=1`, the non-reentrant CFITSIO runtime hits two race conditions: status 122 "too many I/O drivers" (driver registration table) and status 107 "tried to move past end of file" (shared error stack). Serialising tests resolves both. | Runtime failure logged in run [25209384843 (initial cargo test)](https://github.com/ivonnyssen/rusty-photon/actions/runs/25209384843) → fix in commit `e43cbbc`. |
| 4 | What's the cold-cache build time on a fresh runner? | **~2m30s for the Windows job.** Most of that is vcpkg's `install cfitsio` step (binary cache hit) + cargo build of fitsio + fitsio-sys + their transitive deps. linux/macOS control jobs finish in 24–41s. | Job timings on run [25209384843](https://github.com/ivonnyssen/rusty-photon/actions/runs/25209384843): linux 36s, macos 24s, windows 2m29s. |
| 5 | Does it stay green on subsequent runs with vcpkg's binary cache? | **Yes — verified by re-running the same workflow.** Second run completed at almost exactly the same wall-clock time as the first (linux 41s / macOS 35s / Windows 2m32s) with all three jobs green. | Re-run conclusion: success on the same run id. |

## Test results

```
linux  / system cfitsio (control)        ✓ success   41s
macos  / brew cfitsio (control)          ✓ success   35s
windows-msvc / vcpkg cfitsio (RESEARCH)  ✓ success   2m32s
```

All three end-to-end tests pass:

- **`creates_and_reads_back_i32_image`** — `2×2` BITPIX=32 image
  round-trip via `FitsFile::create → write_image → close → open →
  read_image`. Pixels match.
- **`writes_and_reads_custom_string_keyword`** — same flow plus a
  `DOC_ID` keyword written via `hdu.write_key` and read back via
  `hdu.read_key::<HeaderValue<String>>`. Round-trip exact.
- **`fitsio_links_smoke`** — minimal 1-byte BITPIX=8 image creation
  to isolate "does CFITSIO link at all" from the other two tests.
  Passes.

`cargo rail run --profile commit -q`: 988/988 (workspace level — the
spike crate is excluded by design).

## Real findings, beyond "it works"

These came out of the failure modes the spike walked through and
will directly shape the wrapper-crate design:

1. **Vcpkg without `[pthreads]` is the right configuration.** Same
   runtime trade-off as Option 1 (no internal CFITSIO thread safety),
   achieved through vcpkg config rather than a fitsio-sys patch.
   Documented inline on the workflow's `vcpkg install` step.
2. **Callers must serialise CFITSIO access.** Without reentrant mode,
   CFITSIO is not thread-safe at the C level. For our consumers this
   is automatic via `tokio::task::spawn_blocking` only if we keep one
   FITS operation in-flight at a time; the wrapper crate should
   document this contract explicitly. **Concurrent SkyView requests
   in `sky-survey-camera` would race** unless the wrapper enforces
   serialisation (e.g. an `Arc<Mutex<()>>` around the C call sites,
   or a single-thread blocking queue).
3. **Pkg-config probe sticks the spike to crates.io fitsio-sys
   behaviour.** `fitsio-sys = 0.5.7` calls
   `pkg_config::Config::probe("cfitsio")` without `.statik(true)` —
   we set `PKG_CONFIG_ALL_STATIC=1` in the workflow env to compensate,
   but a future fitsio-sys release that changes the probe could
   re-break us. Worth a routine "rerun the spike workflow on every
   fitsio-sys minor version bump" hygiene step.
4. **Cargo.lock matters for standalone spike crates.** Excluding the
   crate from the workspace also excludes it from the workspace
   lockfile. `cargo test --locked` then needs the spike crate's own
   lockfile to be committed. Captured in `crates/fits-cfitsio-spike/Cargo.lock`.

## What this means for `crates/rp-fits`

Combined with the fitsrs spike's findings, the wrapper crate has two
viable shapes:

- **A) fitsrs reads + stock fitsio writes.** Keep fitsrs for the
  read API (license-clean, pure-Rust, no link-time setup, fast).
  Use stock fitsio for writes (full feature set, license-clean,
  Windows now works via this spike's recipe).
- **B) fitsio for both reads and writes.** Drop fitsrs entirely.
  Trade-off: (i) loses fitsrs's pure-Rust no-build-deps property
  (every dev now needs CFITSIO installed locally), (ii) gains
  full BINTABLE / WCS / compression support out of the box, (iii)
  inherits the "callers must serialise" constraint above.

Recommendation lands on **(A)** unless and until we have a use case
that demands BINTABLE or compression read support, because the
distribution friction of "every contributor needs vcpkg" is real
and ongoing — and the read-side wins of fitsrs (Cursor/Read API,
zero C deps) directly benefit sky-survey-camera's hot path.

## Per-platform CFITSIO recipe (for the wrapper-crate PR to inherit)

The CI workflow encodes this — copy/paste into the wrapper crate's
CI when ready:

| Platform | Setup |
|---|---|
| ubuntu-latest | `apt-get install -y libcfitsio-dev pkg-config` |
| macos-latest | `brew install cfitsio pkg-config` |
| windows-latest | `vcpkg install --triplet x64-windows-static-md cfitsio` (no `[pthreads]`) + `choco install pkgconfiglite -y --no-progress` + export `PKG_CONFIG_PATH`, `PKG_CONFIG_ALL_STATIC=1`, `PKG_CONFIG_ALLOW_SYSTEM_LIBS=1`, `PKG_CONFIG_ALLOW_SYSTEM_CFLAGS=1` |

Tests must run with `--test-threads=1` if the test suite uses CFITSIO
across multiple test functions concurrently. (Production code is
gated by spawn_blocking serialisation in our consumers.)

## Next steps

1. **Land this PR.** It validates the recipe in CI and gets the plan
   doc + report on `main` so the wrapper-crate work has a target to
   point at.
2. **Open the `crates/rp-fits` design PR.** Asymmetric wrapper:
   fitsrs reads, fitsio writes, with a small `Mutex`-based
   serialisation guard around fitsio call sites. The two spike
   crates can be deleted at that PR's merge time, or kept as CI
   regression canaries for the recipe.
3. **Migration order, unchanged from the consolidation plan:**
   sky-survey-camera (read-only via fitsrs) → phd2-guider (write via
   fitsio) → rp (read+write).
4. **Decide on the pre-existing GPL-3.0 fitrs dependency separately.**
   The wrapper crate eliminates rp / phd2-guider's reason to keep
   fitrs in the workspace; once they migrate, drop the workspace dep.
5. **Document the windows-msvc CI step** in the wrapper crate so
   future contributors can run the test suite locally (or know to
   skip CFITSIO-touching tests) without surprises.

## References

- PR: [#113 Spike: stock fitsio + vcpkg CFITSIO on Windows MSVC](https://github.com/ivonnyssen/rusty-photon/pull/113)
- Plan: [`fits-cfitsio-vcpkg-spike.md`](fits-cfitsio-vcpkg-spike.md)
- Companion read-side spike: [`fits-spike-report.md`](fits-spike-report.md)
- Issue: [#107 Pick a workspace FITS library](https://github.com/ivonnyssen/rusty-photon/issues/107)
- Historical context: [`docs/decisions/001-fits-file-support.md`](../decisions/001-fits-file-support.md)
- Crate: [`fitsio` on crates.io](https://crates.io/crates/fitsio)
- vcpkg port: [`microsoft/vcpkg/ports/cfitsio`](https://github.com/microsoft/vcpkg/tree/master/ports/cfitsio)
- Long-running upstream issue (now bypassable): [`simonrw/rust-fitsio#230`](https://github.com/simonrw/rust-fitsio/issues/230)
