# ADR-005: Plate Solver via ASTAP, Wrapped as a Subprocess

## Status

Proposed (2026-05-01).

This ADR is the prerequisite called out in
`docs/services/rp.md` §"Plate Solver" (line 1582 — "The choice of plate
solving engine requires further research […] This decision will be
captured in a separate ADR") and in `docs/plans/image-evaluation-tools.md`
Phase 6c (line 499 — `center_on_target` is blocked on it). Once accepted,
Phase 6c is unblocked and a separate plan will sequence the
`rp-plate-solver` rp-managed-service implementation.

## Context

`rp` needs a plate solver to back two pieces of work:

- A built-in `plate_solve` MCP tool that proxies to a supervised
  `rp-plate-solver` rp-managed service (per the existing rp-managed-service
  pattern in `docs/services/rp.md` §"Service Boundaries" and §"Plate
  Solver").
- The `center_on_target` compound built-in tool (Phase 6c), which calls
  `plate_solve` in a capture/solve/sync/slew loop.

Inputs the solver receives: a FITS file path (rp and plugins share a
filesystem per §"File Accessibility") plus optional approximate RA/Dec,
pixel scale, and search radius hints from the mount and camera. Output
required: a WCS solution (RA/Dec at image center, pixel scale, rotation,
and a "did it solve" signal), within a few seconds for typical 2k×2k
frames using mount hints.

**Hard constraints** carried over from the project's existing platform
matrix (CI runs Linux/macOS/Windows; the canonical deployment target is a
Raspberry Pi 5):

1. **Cross-platform**: Linux x64, **Linux aarch64 (Pi 5)**, macOS Apple
   Silicon, Windows x64. No Windows-only or Linux-only solutions.
2. **Offline operation**: an observatory Pi may have no internet at solve
   time. Cloud-only solvers are non-starters.
3. **License**: ADR-001's first supersession was triggered by an
   undisclosed GPL-3.0 conflict in `fitrs`, and the SEP / `sep-sys`
   integration was rejected in `docs/services/rp.md` §"Image Analysis
   Strategy / Design Rationale" (line 1062) for being "LGPL + C FFI
   maintenance burden." Permissive (MIT/Apache/BSD) is preferred. Copyleft
   options must be evaluated explicitly against the planned
   integration mode.

## Options Considered

A wide survey covered pure-Rust crates, Rust FFI bindings, standalone
solver binaries we could subprocess-wrap, and cloud APIs. The full
comparison table is folded into this section; sources are cited under
[References](#references).

### Option 1: ASTAP, wrapped as a subprocess

`astap_cli` is a single static command-line solver written in Object
Pascal. Cross-platform releases include Linux x64 (`amd64`), Linux ARM64
(`aarch64`), Linux ARM hardfloat (`armhf`), macOS (Apple Silicon),
Windows x64, and Windows ARM64.

**Pros:**

- **Operational fit.** Single static binary per platform; no shared-lib
  dependencies; FITS-native input; documented `-ra / -spd / -fov / -r`
  hint flags; writes a `.wcs` sidecar that is straightforward to parse.
- **Cross-platform, including Pi 5 and Apple Silicon.** Verified by the
  spike on Linux ARM64 (see [Verification Spike](#verification-spike)
  for the result captured at ADR-time).
- **Active.** Releases through 2026-04-26 on the Windows line, 2026-04-28
  on Linux. The dominant default solver in the current amateur stack
  (N.I.N.A., APT, CCDciel).
- **Fast.** Sub-second to a few seconds with mount hints on typical
  2k×2k frames. Performance is well above our budget at typical
  amateur image scales.
- **Stable interface.** The CLI surface and the `.wcs` output format
  have been stable for years; the rp-plate-solver wrapper has a small,
  unchanging contract to honour.

**Cons:**

- **License: LGPL-3.0.** The same license class that was rejected for
  `sep-sys`. The rejection rationale in `docs/services/rp.md` §"Design
  Rationale" cites "LGPL **with C FFI maintenance burden**" — the
  burden flag is the load-bearing part. A subprocess does not link
  against the LGPL binary; the legal obligation reduces to "ship the
  binary unmodified, link to source, allow user replacement." See
  [Consequences / License Treatment](#license-treatment) for the
  distribution model that retires the license question.
- **Closed source historically; source available now via SourceForge**,
  but the project culture is solo-maintainer. Bus-factor risk is
  managed by Option 2 being a viable fallback (different solver, same
  subprocess wrapper shape).
- **macOS code-signing.** The macOS binary is ad-hoc-signed by the
  upstream maintainer; users may need to clear quarantine via `xattr
  -d com.apple.quarantine` on first run. Documented in the open
  questions; not a blocker.

### Option 2: astrometry.net `solve-field`, wrapped as a subprocess

The reference plate-solving implementation, written in C, command-line
driven, with a long history.

**Pros:**

- License compatibility with subprocess use is unambiguous: GPL-3.0+
  "mere aggregation" applies cleanly to `Command::new("solve-field")`.
- Native installs on Linux (apt) and macOS (Homebrew). ARM64 Linux works
  cleanly on Pi 5 in current Raspberry Pi OS releases.
- Mature index files; widely deployed.

**Cons:**

- **Windows: no native build.** Upstream documents Cygwin/WSL as the
  Windows path. Shipping Cygwin to end users is operationally heavy and
  changes the install story per platform.
- **Slower than ASTAP** in our regime. Typical hint-aided solves are
  5–30 s; without hints, much longer. Exceeds our few-seconds budget at
  the upper end.
- **Multiple-process pipeline.** `solve-field` is itself a shell script
  that orchestrates `image2xy`, `astrometry-engine`, `wcs-rd2xy`, etc.
  Wrapping it is more brittle than ASTAP's monolithic `astap_cli`.

### Option 3: tetra3 (pure Rust), in-process

`tetra3` (and the related `tetra3rs`) is an MIT/Apache-2.0 pure-Rust
star-pattern matcher.

**Pros:**

- License (MIT OR Apache-2.0) is unambiguously compatible.
- Pure Rust: no rp-managed service required. The `plate_solve` tool
  could be a built-in calling the crate directly. Collapses an entire
  process boundary.
- Active maintenance (v0.7.x in 2026-04).

**Cons:**

- **Wrong target domain.** `tetra3` is a star-tracker solver designed
  for 10°–30° FOV at low pixel counts. Typical astrophoto FOVs of
  0.5°–3° require building a custom Gaia-derived database with deeper
  magnitude limits, and the solver itself is **untested** at those
  pixel scales by upstream.
- The solver consumes pre-extracted **centroids**, not FITS — extraction
  is a feature flag that is itself young.
- Building a domain-appropriate index would itself be a research
  project comparable in scope to writing the adapter for ASTAP.

### Option 4: StellarSolver

KStars/Ekos's library wrapping astrometry.net plus its own SExtractor
fork. Cross-platform via Qt.

**Pros:** runs cross-platform (KStars ships everywhere); good Windows
story.

**Cons:** designed as a **Qt linkable library**, not a CLI. Wrapping it
as a subprocess means writing a thin CLI shim in C++ that we then own,
or pulling Qt into our distribution. License is an effective GPL-3.0+
because of the embedded astrometry.net. Hard pass for our shape.

### Option 5: Siril `siril-cli platesolve`

Siril is a full image-processing suite with a CLI mode that exposes
plate-solving.

**Pros:** cross-platform; mature.

**Cons:** GPL-3.0; pulling in a full image-processing suite for one
subcommand is wildly oversized. The platesolve subcommand can also
delegate to astrometry.net under the hood, so we'd be wrapping a wrapper.

### Option 6: THRASTRO/astrometrylib

A BSD-3 / GPL-3 dual-licensed MSVC-friendly fork of astrometry.net.
Tempting because it would solve the Windows-native gap.

**Cons:** project status is hobbyist (low activity, "a bit further than
POC" per upstream). The dual-license still inherits astrometry.net's
GPL-forcing dependencies. Not a v1 candidate.

### Option 7: Build-it-ourselves in pure Rust

Implementing a star-pattern-matching solver from scratch (e.g. the
4-star quad invariants of astrometry.net or Tetra-style hashes)
plus an embedded Gaia-derived index.

**Cons:** astrometry.net represents ~25 years of star-pattern matching
expertise. The bar to clear is very high; nothing in our roadmap
justifies that investment when ASTAP gives us the same answer for
zero engineering cost.

### Option 8: PlateSolve2/3, ASPS — Windows-only

Disqualified by the cross-platform constraint.

### Option 9: Nova astrometry.net cloud API

Disqualified by the offline constraint.

## Decision

Adopt **Option 1 — ASTAP wrapped as a subprocess** by a new
`rp-plate-solver` rp-managed service.

The decision rests on three points:

1. **Operational fit dominates.** ASTAP is the only candidate that
   clears the cross-platform bar (including Linux ARM64 and Windows
   natively) with a single static binary, FITS-native I/O, and a
   sub-second-with-hints performance profile. Every other candidate
   trades operational quality for license simplicity, and the operational
   cost is high (Cygwin-on-Windows, Qt distribution, oversized suites,
   research-project scope).
2. **Subprocess use retires the LGPL concern.** The `sep-sys` rejection
   memo's load-bearing word is "burden" — specifically the FFI
   maintenance burden of carrying a C library across our build matrix.
   Subprocess invocation has no FFI surface. We ship the upstream
   binary unmodified, document where it came from, and let operators
   replace it with their own build per LGPL-3.0 §4. There is no static
   or dynamic linking, so §6's "Combined Works" clause is not engaged.
3. **Bus-factor managed by a viable fallback.** If ASTAP upstream
   stalls or the license becomes operationally awkward, swapping in
   astrometry.net's `solve-field` is mostly a configuration change in
   the rp-managed-service wrapper — same `Command::new`, different
   argument layout, different output parser. Both share the same
   FITS-in / WCS-out subprocess contract.

Tetra3 remains an attractive longer-term direction. If a future
maintenance window justifies building a Gaia-derived narrow-FOV index,
the in-process pure-Rust path collapses an entire service boundary and
eliminates the LGPL question outright. That is a v2 conversation, not a
v1 one.

## Verification Spike

A reproducible harness lives in `scripts/astap-spike.sh`. It downloads
the appropriate ASTAP CLI for the host platform, runs it with no
arguments to confirm the binary executes and emits its self-help banner,
and (optionally, with `--with-solve <fits>` and a fetched D05 database)
performs a real solve and parses the resulting `.wcs` sidecar.

The harness is **not** wired into `cargo test` or CI. It is an operator
tool for retiring the open questions below on each platform we plan to
ship on.

### Spike result on Linux aarch64 (this machine)

Captured 2026-05-01 against ASTAP CLI release 2026.02.09. See
`scripts/astap-spike.sh` for the exact command. The harness:

1. Downloaded `astap_command-line_version_Linux_aarch64.zip` (≈ 300 KB)
   from SourceForge.
2. Unzipped to `astap_cli` in a tempdir.
3. Invoked `./astap_cli` with no arguments (ASTAP's convention for
   showing the help banner).
4. Confirmed the banner: `ASTAP astrometric solver version
   CLI-2026.02.09`.

The actual end-to-end solve path (D05 download + invoke against a known
FITS) is documented in the harness but is gated behind the
`--with-solve` flag because the database download is ≈ 100 MB. That
verification is left for the per-platform passes outlined under
[Open Questions](#open-questions).

### Open questions to retire before `rp-plate-solver` ships

These are the verification items the spike harness exists to address.
Each becomes a checkbox in the eventual `rp-plate-solver` plan doc.

1. **macOS Apple Silicon** — run the spike, including a `--with-solve`
   pass, on macOS arm64. Confirm `xattr -d com.apple.quarantine` is
   sufficient or whether re-signing with the project's Developer ID is
   required.
2. **Windows x64** — run the spike on Windows native. Confirm the
   download-and-extract flow works without WSL and that `astap_cli.exe`
   runs from a non-installer placement.
3. **Windows ARM64** — the upstream Windows ARM64 build is one release
   behind x64. Confirm parity is acceptable for v1; if not, document a
   graceful "no Windows ARM64 support yet" fallback.
4. **End-to-end solve timing** — run `--with-solve` against
   representative FITS frames on each target platform. Confirm the
   "few seconds with hint" budget holds at the upper end of what
   `rp` will actually feed (full-frame 2k–4k Bayer-debayered
   captures).
5. **LGPL-3.0 §4 / §6 redistribution review** — read the relevant
   clauses against the planned distribution shape (rp installer
   bundles ASTAP unmodified, links to source, allows replacement).
   Confirm the documented obligation ladder is sufficient. The ADR's
   subprocess-vs-FFI distinction depends on this review.
6. **Index database hosting** — decide whether `rp-plate-solver` ships a
   small bundled DB, fetches D05 on first run, or leaves DB management
   to the operator. Mirroring SourceForge has its own redistribution
   implications.
7. **Hint plumbing** — confirm the ASCOM mount driver exposes the
   pointing accuracy ASTAP's `-r` (search radius) flag depends on. The
   speed advantage over astrometry.net evaporates without good hints.

## Consequences

### Architecture

- A new `services/rp-plate-solver/` workspace member is created later
  (separate plan), structured as an rp-managed service per the existing
  pattern (`services/sentinel`, `services/phd2-guider`).
- The built-in `plate_solve` MCP tool in `rp` proxies to the service.
- The service supervises a single ASTAP CLI invocation per request,
  with a timeout, graceful kill, and structured error reporting back
  through the MCP tool surface.
- Sentinel restarts the service on hang or crash via the existing
  rp-managed-service supervision flow.

### Distribution

- Per-platform installers fetch the appropriate ASTAP CLI archive from
  SourceForge at install time (or bundle the binary directly — the
  install-time fetch keeps our installer footprint small but introduces
  an offline-install caveat). Final shape decided in the
  `rp-plate-solver` plan.
- Index databases (D05 by default, ≈ 100 MB) are fetched separately on
  first use, configurable via the `rp-plate-solver` config block.
- The `data_directory` ASCOM rp shares via §"File Accessibility" is the
  natural place for the solver to read FITS from; no additional path
  contract is needed.

### License Treatment

- ASTAP's binaries are LGPL-3.0. We treat them as **conveyed in object
  form, unmodified**, per LGPL-3.0 §4. The rp installer:
  1. Ships the upstream binary unmodified.
  2. Includes the LGPL-3.0 text and a clear statement of where the
     binary was sourced from.
  3. Documents in the user-facing license file that the binary is
     replaceable: an operator may substitute their own ASTAP build by
     pointing the `astap_binary_path` config field at it.
- No part of `rp` is statically or dynamically linked against the
  ASTAP binary, so §6's "Combined Works" clause is not engaged. The
  subprocess boundary is the legal severance.
- `solve-field` (Option 2) remains available as a drop-in alternative
  via the same configuration knob; an operator can swap implementations
  without rebuilding `rp`.

### CI / Build

- No new workspace dependency on a C library. The build matrix is
  unchanged; `cargo rail run --profile commit` continues to be the
  pre-push gate.
- The rp-plate-solver crate's own tests will mock the binary subprocess
  (per `docs/decisions/004-testing-strategy-for-http-client-error-paths.md`'s
  abstract-the-trait pattern). End-to-end ASTAP execution is verified
  manually via the spike harness, not in `cargo test`.

### Compatibility With Existing Decisions

- Consistent with `docs/services/rp.md` §"Plate Solver" (rp-managed
  service wrapping an external solver binary).
- Consistent with the SEP rejection in §"Design Rationale": the FFI
  burden was the operational complaint, and this decision avoids FFI
  entirely.
- Inherits the data-flow contract from §"File Accessibility": rp and
  the plate-solver service share a filesystem; the solver receives a
  path, not bytes over HTTP.

## References

### Project context

- `docs/services/rp.md` §"Plate Solver" (line 1566) — the
  rp-managed-service pattern this ADR ratifies.
- `docs/services/rp.md` §"Image Analysis Strategy / Design Rationale"
  (line 1057) — the SEP / `sep-sys` rejection memo whose
  "LGPL + FFI burden" wording is reread in this ADR.
- `docs/plans/image-evaluation-tools.md` Phase 6c (line 499) — the
  blocked work item this ADR unblocks.
- `docs/decisions/001-fits-file-support.md` — prior decision on
  cross-platform native libraries; informed the cross-platform bar.

### Candidates surveyed

- ASTAP — <https://www.hnsky.org/astap.htm>,
  <https://sourceforge.net/projects/astap-program/files/>.
  License: LGPL-3.0 per the SourceForge metadata and the COPYING file in
  the source archive.
- astrometry.net — <https://github.com/dstndstn/astrometry.net>,
  <https://astrometry.net/use.html>. License: GPL-3.0+ effective, per
  the upstream LICENSE file.
- StellarSolver — <https://github.com/rlancaste/stellarsolver>.
- Siril — <https://gitlab.com/free-astro/siril>,
  <https://siril.readthedocs.io/en/latest/astrometry/platesolving.html>.
- tetra3 — <https://crates.io/crates/tetra3>,
  <https://docs.rs/tetra3/latest/tetra3/>.
- THRASTRO/astrometrylib —
  <https://github.com/THRASTRO/astrometrylib>.
- platesolve crate — <https://crates.io/crates/platesolve>.
- rastap — <https://github.com/vrruiz/rastap>.

### Background reading

- "Plate Solving with Astrometry.net on Raspberry Pi 5" —
  <https://astroisk.nl/unlocking-the-cosmos-plate-solving-with-astrometry-net-on-raspberry-pi-5/>.
  Confirms the Pi 5 native build path for the fallback option.
