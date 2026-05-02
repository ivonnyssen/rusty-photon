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
- **Cross-platform, including Pi 5 and Apple Silicon.** Verified by
  the [`install-astap`](../../.github/actions/install-astap/action.yml)
  smoke workflow on Linux x64 + macOS arm64 + Windows x64, plus a
  manual Linux aarch64 install captured at ADR-time. See
  [Verification Spike](#verification-spike) for the table of results.
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

### Option 4: zodiacal (pure Rust)

`zodiacal` is an Apache-2.0 pure-Rust blind plate solver implementing
the same Lang-et-al-2010 4-star quad geometric-hashing algorithm
astrometry.net uses, with kd-tree code matching, TAN-WCS fitting, and
Bayesian verification.

**Pros:**

- License (Apache-2.0) is unambiguously compatible.
- Pure Rust: no rp-managed service required. The `plate_solve` tool
  could be a built-in calling the crate directly. Collapses the entire
  service boundary that the BYO ASTAP model still needs.
- **Targets our FOV regime** — the published index is built from
  Gaia DR3 for typical amateur scales, unlike `tetra3` whose default
  index targets star trackers.
- Self-reported benchmark: 985 / 1000 simulated 9568×6380 frames
  solved, median 1.07 s — same order of magnitude as ASTAP, slower
  with hints.

**Cons:**

- **Alpha, ~3 months old.** First crate publication 2026-02-06,
  current 0.4.1 of 2026-04-29. ~100 total downloads. The version
  trajectory (0.0.1 → 0.0.2 → 0.2.0 → 0.4.0 → 0.4.1 in 12 weeks)
  signals an API in motion.
- **Single-org bus-factor risk.** OrbitalCommons is the sole
  publisher; no upstream ecosystem yet.
- **Heavy index footprint.** The Gaia-derived index is ~2.5 GB at
  mag-16 (vs. ASTAP D05's ~100 MB) and ~17 GB at mag-19. Real cost
  on a Pi-5 SD-card deployment.
- **Not FITS-native.** Inputs are PNG or pre-extracted JSON
  centroids. Bridging FITS in is straightforward via our existing
  `imaging/stars.rs` centroid extraction, but it is glue we'd own.
- Untested at the scale ASTAP has been hammered by amateur operators
  nightly for years.

Same archetype as `tetra3` — the right shape for v2, not v1. Worth
revisiting in 6–12 months once the API stabilises and the field-test
record fills in.

### Option 5: StellarSolver

KStars/Ekos's library wrapping astrometry.net plus its own SExtractor
fork. Cross-platform via Qt.

**Pros:** runs cross-platform (KStars ships everywhere); good Windows
story.

**Cons:** designed as a **Qt linkable library**, not a CLI. Wrapping it
as a subprocess means writing a thin CLI shim in C++ that we then own,
or pulling Qt into our distribution. License is an effective GPL-3.0+
because of the embedded astrometry.net. Hard pass for our shape.

### Option 6: Siril `siril-cli platesolve`

Siril is a full image-processing suite with a CLI mode that exposes
plate-solving.

**Pros:** cross-platform; mature.

**Cons:** GPL-3.0; pulling in a full image-processing suite for one
subcommand is wildly oversized. The platesolve subcommand can also
delegate to astrometry.net under the hood, so we'd be wrapping a wrapper.

### Option 7: THRASTRO/astrometrylib

A BSD-3 / GPL-3 dual-licensed MSVC-friendly fork of astrometry.net.
Tempting because it would solve the Windows-native gap.

**Cons:** project status is hobbyist (low activity, "a bit further than
POC" per upstream). The dual-license still inherits astrometry.net's
GPL-forcing dependencies. Not a v1 candidate.

### Option 8: Build-it-ourselves in pure Rust

Implementing a star-pattern-matching solver from scratch (e.g. the
4-star quad invariants of astrometry.net or Tetra-style hashes)
plus an embedded Gaia-derived index.

**Cons:** astrometry.net represents ~25 years of star-pattern matching
expertise. The bar to clear is very high; nothing in our roadmap
justifies that investment when ASTAP gives us the same answer for
zero engineering cost.

### Option 9: PlateSolve2/3, ASPS — Windows-only

Disqualified by the cross-platform constraint.

### Option 10: Nova astrometry.net cloud API

Disqualified by the offline constraint.

## Decision

Adopt **Option 1 — ASTAP, executed as a subprocess by a new
`rp-plate-solver` rp-managed service, with the ASTAP binary and index
database supplied by the operator (BYO)** rather than bundled,
mirrored, or fetched by `rp` itself.

The decision rests on three points:

1. **Operational fit dominates.** ASTAP is the only candidate that
   clears the cross-platform bar (including Linux ARM64 and Windows
   natively) with a single static binary, FITS-native I/O, and a
   sub-second-with-hints performance profile. Every other candidate
   trades operational quality for license simplicity, and the operational
   cost is high (Cygwin-on-Windows, Qt distribution, oversized suites,
   research-project scope).
2. **Bring-your-own binary keeps `rp` out of the LGPL distribution
   path entirely.** `rp` does not ship, bundle, mirror, or stage the
   ASTAP binary or index database. Operators install ASTAP from
   upstream (hnsky.org / SourceForge / their package manager) and
   point `rp-plate-solver` at the install via required config fields.
   CI does the same via a local
   [`install-astap`](../../.github/actions/install-astap/action.yml)
   composite action that mirrors the existing `install-omnisim`
   pattern (CI is just acting as a normal user that installs ASTAP
   for itself). Working assumption pending the formal review listed
   under [Open Questions](#open-questions-to-retire-before-rp-plate-solver-ships):
   because `rp` never *conveys* the LGPL work, the LGPL-3.0 §4 / §6
   redistribution paths are not engaged at the `rp` boundary at all.
   The SEP/`sep-sys` rejection rationale ("LGPL + FFI burden") is
   doubly avoided — there is no FFI surface *and* there is nothing
   for `rp` to redistribute. The runtime subprocess boundary
   additionally keeps §6's "Combined Works" clause out of scope at
   execution time.
3. **Bus-factor managed by a viable fallback.** If ASTAP upstream
   stalls or the license becomes operationally awkward, swapping in
   astrometry.net's `solve-field` is mostly a configuration change in
   the rp-managed-service wrapper — same `Command::new`, different
   argument layout, different output parser. Both share the same
   FITS-in / WCS-out subprocess contract, and both fit the BYO
   posture identically.

Pure-Rust solvers — `tetra3` (Option 3) and `zodiacal` (Option 4) —
remain attractive longer-term directions. Either would collapse the
rp-managed-service boundary entirely (`plate_solve` becomes a
built-in calling a crate function in process) and remove the LGPL
question outright (both Apache-2.0). `zodiacal` already targets our
FOV regime, which `tetra3` does not; its v1-disqualifying gap is
maturity (alpha, ~3 months old) and a heavy index footprint, not
domain fit. Revisit in 6–12 months.

## Verification Spike

The verification work has a single artefact: the local
[`install-astap`](../../.github/actions/install-astap/action.yml)
composite action plus the
[`install-astap.yml`](../../.github/workflows/install-astap.yml)
workflow that exercises it on `ubuntu-latest`, `macos-latest`, and
`windows-latest`. The action is itself the install recipe — its per-OS
table names the SourceForge archive each platform downloads from. The
smoke workflow forces a fresh upstream download on every run (via
`cache-key-suffix: github.run_id`) so upstream regressions surface
within a CI cycle. This mirrors the model `install-omnisim` follows
for the ASCOM simulator.

### Pinned SHA-256 + refresh procedure

The action's per-OS table pins a SHA-256 for each downloaded archive
(snapshot captured 2026-05-02) and the optional D05 database. Every
download is verified before extraction; mismatch fails the action
closed. ASTAP's URLs are unversioned ("latest" filenames), so
upstream rotates the bytes without bumping the URL — the SHA pin
turns that rotation into a deliberate, reviewed event rather than a
silent supply-chain drift.

**Refresh procedure** (when ASTAP releases an upstream update and the
verify step starts failing):

1. Download each archive listed in the action's per-OS table from
   SourceForge (and `d05_star_database.zip` if the database SHA is
   what failed).
2. `sha256sum` each (or `shasum -a 256` on macOS).
3. Update the corresponding `SHA256=` line in
   `.github/actions/install-astap/action.yml` and the
   `astap-d05-database-<8-hex-prefix>` cache-key prefix if applicable.
4. Run the smoke workflow on the change to confirm the new pin
   verifies cleanly on all three runner OSes.
5. Land the PR with a brief note of which ASTAP CLI version the new
   bytes correspond to (read from the binary's `astap_cli` banner).

### Operator verification, after a BYO install

An operator who has installed ASTAP on their own machine (per the BYO
posture this ADR adopts) can verify it works in three commands:

```sh
# 1. Sanity-check the binary: prints the banner and a usage block.
"$ASTAP_BINARY"

# 2. End-to-end solve against a known FITS, with a star database
#    matched to the field of view (D05 for 0.6°-6° FOV).
"$ASTAP_BINARY" -f path/to/your.fits -d "$ASTAP_DB_DIR" -wcs

# 3. The previous step writes path/to/your.wcs alongside the FITS.
#    Read CRVAL1/CRVAL2 (RA/Dec at center), CDELT1/CDELT2 (pixel
#    scale), CROTA2 (rotation).
cat path/to/your.wcs
```

The earlier `scripts/astap-spike.sh` operator harness was deleted
once `install-astap` matured: the script duplicated the action's
per-OS table (causing divergence bugs), and its auto-download
behaviour conflicted with the BYO posture this ADR adopts. The
action remains the canonical install recipe for both CI and human
operators reading the file.

### Smoke results captured during ADR drafting

| Platform | ASTAP CLI version | Source | Result |
|----------|------------------|--------|--------|
| Linux aarch64 | CLI-2026.02.09 | manual download from SourceForge (2026-05-01) | banner OK |
| Linux x64 | latest | `install-astap` smoke workflow | banner OK |
| macOS arm64 | latest | `install-astap` smoke workflow | banner OK |
| Windows x64 | CLI-2026.03.05 | `install-astap` smoke workflow | banner OK |

End-to-end solve verification (D05 database + a known FITS) is left
to the per-platform passes outlined under
[Open Questions](#open-questions-to-retire-before-rp-plate-solver-ships).

### Open questions to retire before `rp-plate-solver` ships

Each becomes a checkbox in the eventual `rp-plate-solver` plan doc.

1. **macOS Apple Silicon — end-to-end solve.** The `install-astap`
   smoke workflow already covers binary install + banner. Add an
   end-to-end solve pass (manual or in the workflow with
   `download-database: true` and a publicly-fetchable test FITS).
   Confirm `xattr -d com.apple.quarantine` is sufficient or whether
   re-signing with the project's Developer ID is required.
2. **Windows x64 — end-to-end solve.** Same shape as item 1 — the
   smoke workflow proves the binary runs; outstanding question is
   whether a real solve completes within budget on a stock Windows
   runner.
3. **Windows ARM64** — the upstream Windows ARM64 build is one
   release behind x64. Confirm parity is acceptable for v1 (no
   GitHub-hosted ARM64 Windows runner today, so this is a
   manual-machine verification); if not, document a graceful "no
   Windows ARM64 support yet" fallback and remove the `Windows-ARM64`
   row from the install-astap action's per-OS table.
4. **End-to-end solve timing** — once items 1–3 are wired,
   capture timing on representative FITS frames on each target
   platform. Confirm the "few seconds with hint" budget holds at
   the upper end of what `rp` will actually feed (full-frame 2k–4k
   Bayer-debayered captures).
5. **LGPL-3.0 §4 / §6 review under BYO** — confirm that "execute a
   binary the operator installed" does not engage either clause; that
   the `install-astap` GitHub action does not constitute conveyance
   (it triggers a fresh upstream download per run); and that the
   GH-Actions cache layer for that download is scoped narrowly enough
   to count as ephemeral build infrastructure rather than mirroring.
   The §6 "Combined Works" question is also confirmed moot for
   subprocess execution. This question is the formal closure of the
   ADR's working assumption — see [License Treatment](#license-treatment).
6. **Hint plumbing** — confirm the ASCOM mount driver exposes the
   pointing accuracy ASTAP's `-r` (search radius) flag depends on.
   The speed advantage over astrometry.net evaporates without good
   hints.

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

- `rp` ships **no** ASTAP code, binary, archive, or index database.
  Operator-supplied install via `astap_binary_path` and
  `astap_db_directory` is the contract.
- Per-platform install instructions live in the `rp-plate-solver`
  README, linking out to hnsky.org / SourceForge by platform. The
  README also points operators at
  [`.github/actions/install-astap`](../../.github/actions/install-astap/action.yml)
  in this repo as the reference for how CI installs ASTAP — the same
  recipe an operator would follow manually.
- The `data_directory` `rp` shares via §"File Accessibility" remains
  the place the solver reads FITS from; no additional path contract
  is needed.

### License Treatment

This section captures the working assumption pending the formal
LGPL-3.0 review listed as Open Question #5. The posture is "BYO so
the question reduces to a sanity check," not "conveyance compliant."

- ASTAP's binaries are LGPL-3.0. **`rp` does not convey them.**
  Operators install ASTAP separately (from hnsky.org, SourceForge,
  their package manager, or their own build) and point
  `rp-plate-solver` at it via required config fields:
  - `astap_binary_path` — absolute path to `astap_cli` (or
    `astap_cli.exe` on Windows).
  - `astap_db_directory` — directory containing the operator's
    chosen star database (D05 by default; operators can choose
    larger DBs for wider FOVs).
- Both fields are validated at `rp-plate-solver` startup; missing or
  unreadable values produce an explicit error that names the field
  and links to install instructions.
- CI installs ASTAP into its own runners via a local
  [`.github/actions/install-astap`](../../.github/actions/install-astap/action.yml)
  composite action — same shape `install-omnisim` already uses. The
  action triggers a fresh upstream download per run; it is repo
  tooling, not a published distribution channel. (Whether the
  GH-Actions cache layer for that download counts as "mirroring" is
  a sub-item of Open Question #5.)
- Working assumption: because `rp` never conveys the LGPL work,
  LGPL-3.0 §4 and §6 are not engaged at the `rp` boundary. The
  runtime subprocess boundary additionally keeps §6 out of scope at
  execution time. **This is the assumption Open Question #5
  formally closes** — not a settled legal conclusion the ADR makes
  on its own authority.
- `solve-field` (Option 2) and any other compatible solver remain
  available via the same configuration knobs; an operator can swap
  implementations without rebuilding `rp`.

### CI / Build

- No new workspace dependency on a C library. The build matrix is
  unchanged; `cargo rail run --profile commit` continues to be the
  pre-push gate.
- The rp-plate-solver crate's own tests will mock the binary subprocess
  (per `docs/decisions/004-testing-strategy-for-http-client-error-paths.md`'s
  abstract-the-trait pattern). End-to-end ASTAP execution is verified
  by the `install-astap` smoke workflow plus the per-platform
  end-to-end solve passes outlined in Open Questions 1–4 — not in
  `cargo test`.

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
- zodiacal — <https://crates.io/crates/zodiacal>,
  <https://github.com/OrbitalCommons/zodiacal>. Apache-2.0,
  alpha (~3 months old at ADR time).
- THRASTRO/astrometrylib —
  <https://github.com/THRASTRO/astrometrylib>.
- platesolve crate — <https://crates.io/crates/platesolve>.
- rastap — <https://github.com/vrruiz/rastap>.

### Background reading

- "Plate Solving with Astrometry.net on Raspberry Pi 5" —
  <https://astroisk.nl/unlocking-the-cosmos-plate-solving-with-astrometry-net-on-raspberry-pi-5/>.
  Confirms the Pi 5 native build path for the fallback option.
