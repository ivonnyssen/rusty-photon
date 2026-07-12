# Nightly Releases Plan — a rolling nightly channel for every OS target

## Goal

Publish a nightly release built from the latest `main`: one rolling GitHub
prerelease carrying installable packages for Debian (`.deb`, x86_64 +
arm64), Fedora (`.rpm`, x86_64 + arm64), Windows (suite MSI, x64), and
macOS (Homebrew tap, arm64) — each OS phase independently implementable.
Today packaging is on-demand only (`scripts/build-packages.sh` on a target
machine; `release.yml` on a `v*` tag); the nightly channel gives the rigs
and any test box an always-current, verified upgrade path between releases.

This plan changes nothing in
[ADR-012](../decisions/012-service-packaging-architecture.md)/
[ADR-013](../decisions/013-native-sdk-payload-policy.md)/
[ADR-014](../decisions/014-zwo-per-device-services-and-link-features.md)/
[ADR-015](../decisions/015-windows-packaging-architecture.md) — it adds a
release *channel* on top of the packaging those ADRs define. `release.yml`
stays tag-triggered and untouched here; the scripts this plan builds are
deliberately reusable so the deferred `release.yml` generalization
(PR-7 in [service-packaging.md](service-packaging.md)) becomes thin.

## Implementation Status

| Phase | Description | Status | Branch / PR |
|-------|-------------|--------|-------------|
| N0 | Tech spike: hosted-arm64 verify, timings, asset naming, version dialects — settles the Orange Pi question | Not started | |
| N1 | Debian anchor: `nightly-packages.yml` shared spine + `.deb` legs (x86_64 + arm64), rolling release, docs | Not started | |
| N2 | Fedora: `.rpm` build on both arches + Fedora lifecycle verify leg | Not started | |
| N3 | Windows: suite-MSI leg (strictly after W5 of [windows-packaging.md](windows-packaging.md)) | Not started | |
| N4 | macOS: per-service arm64 tarballs + Homebrew tap channel + `verify-brew.sh` | Not started | |

N1 is the anchor (it builds the shared spine); N2, N3, N4 are mutually
independent afterwards. N3 is gated only on W5; N4 has synergy with PR-7
(stable-channel formula generation) and the two are best done as one arc.

## Decisions (fixed)

- **Channel shape: one rolling `nightly` prerelease.** A single tag,
  force-moved to the packaged commit each night; all assets replaced; no
  dated history. The release body records source SHA + date + asset table.
  Stable asset URLs make a future rig-update helper trivial. Rolling back
  means rebuilding a known-good SHA on demand. The tag MUST NOT match
  `v*` — that pattern triggers `release.yml`.
- **Source: HEAD of `main`, skip-if-unchanged.** Main is already PR-gated
  by the Bazel checks; no last-green lookup. If HEAD equals the SHA the
  `nightly` tag points at, the run exits early — no rebuild, no asset
  churn.
- **All-or-nothing publish.** Every *enabled* OS leg must pass its build +
  lifecycle verification before anything is published, so the release is
  always one coherent SHA with a complete asset set. A flaky leg means "no
  nightly today" plus a tracking issue — never a mixed-SHA release.
- **Version dialects.** One version job computes the base version (from
  the workspace `Cargo.toml`) + UTC date + short SHA, and each packager
  renders its own dialect of "sorts after 0.1.0, before 0.1.1, upgrades in
  place":

  | Target | Nightly version | Mechanics |
  |--------|-----------------|-----------|
  | Debian `.deb` | `0.1.0+nightly.20260712.gabc1234` | dpkg sorts `+…` above `0.1.0`, below `0.1.1`; `apt` upgrades over an on-demand `0.1.0-1` install |
  | Fedora `.rpm` | `0.1.0^20260712.gabc1234` | rpm forbids `+` in Version; `^` is rpm's post-release snapshot operator with the same ordering |
  | Windows MSI | ProductVersion `0.1.0.<YYDDD>` | MSI versions are numeric `x.y.z[.w]` and ignore the 4th field in upgrade comparisons, so nightlies pair it with `MajorUpgrade AllowSameVersionUpgrades="yes"`; the full string travels in the filename and ARP comments. `YYDDD` = 2-digit year × 1000 + day-of-year (fits the 65535 cap through 2065) |
  | macOS | full string in the tarball filename + tap formula `version` | no installer database; Homebrew compares formula versions (see flagged unknowns for the `+` tokenization check) |

  Known caveat (Debian): once a machine runs nightlies, a plain on-demand
  `0.1.0-1` build is a *downgrade* for apt (`--allow-downgrades` or
  `dpkg -i`). Accepted.
- **Build hosts: GitHub-hosted runners.** `ubuntu-latest` (x86_64) +
  `ubuntu-24.04-arm` (arm64; free on public repos, already used by
  `install-astap.yml`), `windows-latest`, `macos-latest` (Apple Silicon).
  No self-hosted machines in the nightly path unless the N0 spike fails
  the hosted-arm64 leg (see N0's Orange Pi criteria). Cross-compilation
  stays off the table (native-SDK linking is unproven cross-arch and
  buys nothing while hosted arm64 runners exist).
- **Verification gate: the full lifecycle, per leg, pre-publish.**
  `verify-packages.sh` (Debian), its Fedora leg (N2), `verify-msi.ps1`
  (N3), `verify-brew.sh` (N4). Building without installing is not a gate.
- **Intel macOS is dropped everywhere.** The nightly ships arm64-only
  macOS artifacts, and the stable-channel formulas lose their Intel
  branches when N4 lands.
- **macOS distribution is Homebrew, not a suite `.pkg`.** Rationale
  recorded in the N4 design below: the Windows suite MSI exists because
  Windows has no package manager; with a tap, macOS is Linux-shaped —
  per-service formulas are the feature tree, `brew services` is systemd,
  and a meta-formula gives the one-command family install.
- **Thin workflows, thick scripts.** Every leg drives a repo script
  (`build-packages.sh`, `build-msi.ps1`, the formula generator), so
  on-demand builds, nightly, and the future `release.yml` generalization
  share one logic layer.
- **Failure handling** reuses the open-or-update tracking-issue pattern
  from `scheduled.yml` / `pi-nightly.yml`, label `nightly-packages`.

## Design

### Shared spine (`nightly-packages.yml`, built in N1)

- Triggers: `schedule` (`7 5 * * *` — 05:07 UTC, after pi-nightly at
  04:00, before the 07:07 nightly cluster) + `workflow_dispatch`.
  `concurrency` group without cancel-in-progress (same reasoning as
  pi-nightly: losing a scheduled run is worse than two back-to-back).
- Job graph:
  1. **`plan`** — checkout, compare `main` HEAD against
     `nightly^{commit}`; if equal, output `skip=true` and every later job
     no-ops. Otherwise compute the base version, date, short SHA and emit
     each dialect as outputs.
  2. **Per-OS build+verify legs** (matrix as phases land) — each stages
     pinned SDKs (with `actions/cache` on `~/.cache/rusty-photon-pkg`
     keyed on the SDK pins, keeping qhyccd.com off the nightly critical
     path), builds via the repo script with the version dialect, runs its
     lifecycle verifier, and uploads the artifacts.
  3. **`publish`** — needs *all* enabled legs; downloads artifacts, writes
     a merged `SHA256SUMS.txt`, force-moves the `nightly` tag to the
     packaged SHA, replaces the prerelease's assets, rewrites the release
     body (SHA, date, asset table). For N4 it also pushes the regenerated
     `-nightly` formulas to the tap in the same job, so tap and assets
     move together.
  4. **`notify-on-failure`** — tracking issue, `schedule` events only.
- Rust build caching per leg via `Swatinem/rust-cache` (packaging builds
  are cargo, not Bazel — same split as today's `build-packages.sh`).

### Phase N0 — tech spike

A `workflow_dispatch`-only scratch workflow on a spike branch, never
merged; findings land as edits to this plan (status table + flagged
unknowns). Checklist, in priority order:

1. **`verify-packages.sh` on `ubuntu-24.04-arm`** — does podman
   `--systemd=always` with the debian:trixie image work on the hosted
   arm64 image the way it does on x86? This is the load-bearing
   assumption of the arm64 leg.
2. **Timings** — cold and warm build+verify wall-clock on both Linux legs
   with the SDK and rust caches wired, plus observed queue latency for
   the arm64 runner pool.
3. **Asset naming** — how `gh release download` and plain `curl` handle
   `+` (and `^`) in asset filenames; pick the convention (keep the
   dialect characters vs a filename-safe dot rendering while package
   metadata keeps the real dialect).
4. **`--deb-version` prototype** — `build-packages.sh` pass-through to
   `cargo deb --deb-version`, then prove `apt` upgrades a `0.1.0-1`
   install to `0.1.0+nightly.…` in a scratch container.
5. **Homebrew version comparison** — confirm
   `0.1.0+nightly.20260712.gabc1234`-shaped strings compare monotonically
   for `brew upgrade` (worst case: dot-render the formula `version`).

**Orange Pi decision (spike output).** Bring up the Orange Pi 5 Ultra as
a dedicated packaging runner **only if** the hosted arm64 leg fails
check 1 or is unacceptable on check 2 (guideline: > 90 min wall-clock or
chronic multi-hour queueing). Trade-offs if adopted: persistent warm
caches and no queue, but packaging needs sudo/apt/podman — a *privileged*
self-hosted runner on a public repo, unlike the deliberately sudo-less
Pi 5 CI runner — so it must copy pi-nightly's security model exactly
(schedule/dispatch-only triggers, never `pull_request`/`push`,
main-pinned checkout), and it couples nightlies to home infrastructure
being up. Default expectation: hosted wins; record the measured numbers
here either way.

### Phase N1 — Debian (the anchor)

- `build-packages.sh` grows `--deb-version <v>` (pass-through to
  `cargo deb --deb-version`; `dist/` path keyed on the full version).
- `nightly-packages.yml` lands with the shared spine and two legs:
  `ubuntu-latest` and `ubuntu-24.04-arm` (or the Orange Pi per N0), each
  running `build-packages.sh --deb-version …` then `verify-packages.sh`
  (the full install → probe → remove → purge lifecycle).
- Publish job as described in the spine; 17 packages per arch +
  `SHA256SUMS.txt`.
- Docs: nightly-channel section in [docs/packaging.md](../packaging.md) —
  install/upgrade commands, channel semantics, the on-demand-downgrade
  caveat, rollback-by-rebuilding-a-SHA.
- `session-runner` remains the one unpackaged daemon (tracked in
  [service-packaging.md](service-packaging.md)); the nightly ships
  whatever has a `pkg/` dir, so it joins automatically once packaged.

### Phase N2 — Fedora

Building is nearly free — `cargo generate-rpm` is pure Rust and already
wired behind `build-packages.sh --rpm`, so the `.rpm`s build on the same
two Ubuntu legs, both arches. The real work:

- **Versioning**: confirm `cargo-generate-rpm` can override Version with
  the `^` dialect (`--set-metadata` or equivalent); if it cannot, that is
  an upstream gap to work around or file.
- **Verification**: `verify-packages.sh` is Debian-only today. Add a
  Fedora leg — podman systemd container on a Fedora image, `dnf install`
  → unit active → config self-created → HTTP probe → remove. rpm has no
  purge lifecycle; the manual-cleanup story in `docs/packaging.md`
  already covers it.
- **Carried unknowns** from service-packaging.md land here: the
  rpm package-name override (packages may stay crate-named rather than
  `rusty-photon-<svc>`), and whether rpm dependency auto-resolution is
  adequate across the family.
- **Scope honesty**: no known Fedora consumer today — this phase keeps
  the door open. If none has appeared by the time N2 is picked up,
  consider a shallower verify leg (install + probe, skip the remove
  matrix) and note it here.

### Phase N3 — Windows (after W5)

W5 of [windows-packaging.md](windows-packaging.md) delivers
`build-msi.ps1`, `verify-msi.ps1`, and the release-tag MSI job. Its
"nightly build + verify on a schedule" item is implemented **here**, as
the `msi` leg of `nightly-packages.yml` — not as a scheduler of its own
(coordination note recorded in that plan).

- `build-msi.ps1` gains a version-stamp parameter (ProductVersion
  `0.1.0.<YYDDD>` + the full nightly string for the filename).
- `installer/Package.wxs`: `MajorUpgrade AllowSameVersionUpgrades="yes"`
  (required for nightly-over-nightly, harmless for releases) and the full
  nightly string in ARP comments so Programs & Features can tell
  nightlies apart despite the numeric ProductVersion.
- Leg on `windows-latest`: build → `verify-msi.ps1` (already the full
  lifecycle gate, including the kill-and-observe SCM restart check) →
  upload `rusty-photon-<fullversion>-x64.msi`.
- Nightly-specific check added to `verify-msi.ps1`: download the
  *current* nightly MSI from the release (before assets are replaced),
  install it, then install the freshly built MSI over it — proving the
  `AllowSameVersionUpgrades` upgrade path that release-tag testing never
  exercises. Skip gracefully when no prior nightly exists.

### Phase N4 — macOS (Homebrew)

**Why not a suite `.pkg`** (the literal MSI analogue via `productbuild` +
choices XML), and why not a cask wrapping one:

- Unsigned/unnotarized pkgs are blocked outright by Gatekeeper on modern
  macOS (System-Settings-approval dance); we are unsigned pre-1.0, and
  notarization means a paid Developer ID + CI ceremony just to reach the
  install dialog.
- pkg has no uninstall or upgrade management — we would hand-build what
  MSI (`MajorUpgrade`) and dpkg give for free.
- It duplicates the service layer: 17 hand-authored launchd plists plus
  activation logic, versus the one-line `service do` block per formula
  that `brew services` turns into managed launchd services.
- The cask variant fixes none of this: a cask's pkg choices are fixed at
  authoring time (no feature tree for the user) and casks don't
  participate in `brew services`.

The suite MSI exists because Windows has no package manager. Homebrew
*is* the package manager, which puts macOS in the Linux camp: per-service
formulas are the feature tree, and selection moves from install time to
service-start time. The "single installer" experience is a meta-formula:

```sh
brew tap ivonnyssen/rusty-photon
brew install rusty-photon                      # meta-formula → whole family
brew services start rusty-photon-zwo-camera    # start what this box uses
brew services start rusty-photon-ui-htmx
```

Config-gated services need no mechanism at all — nothing starts until
`brew services start`, so the gating is inherent.

**Tap layout** (the existing `homebrew-rusty-photon` repo):

- `Formula/rusty-photon-<svc>.rb` × 17 — stable channel, updated by
  `release.yml` on `v*` tags. Absorbs PR-7's rename
  (`filemonitor.rb` → `rusty-photon-filemonitor.rb`, class
  `RustyPhotonFilemonitor`).
- `Formula/rusty-photon-<svc>-nightly.rb` × 17 — nightly channel, updated
  by the nightly publish job, `conflicts_with` its stable sibling (same
  binary names). Channels must be distinct formulas: a formula pins one
  url+sha256, and the stable pointer cannot be overwritten nightly.
- `Formula/rusty-photon.rb` + `rusty-photon-nightly.rb` — meta-formulas
  depending on their channel's 17.

**Binary formulas, not bottles.** Formulas point `url` at per-service
arm64 tarballs on the GitHub release and `bin.install` the binary — the
existing filemonitor pattern. Source-build formulas + bottling would
force `depends_on "rust"` and the native-SDK staging story onto end-user
machines for no benefit. This route also sidesteps signing entirely:
Homebrew's curl download never sets the quarantine xattr, and Rust's
linker ad-hoc-signs arm64 binaries, so brew-installed binaries run with
no notarization — Homebrew is precisely the macOS channel where unsigned
pre-1.0 works smoothly.

**Service model** — each formula carries a `service do` block
(`run`, `keep_alive true`, `log_path`/`error_log_path`,
`run_type :immediate`); `brew services` generates and loads the plist:

| Linux / Windows concept | macOS equivalent |
|---|---|
| systemd unit / SCM service | `service do` block → launchd via `brew services` |
| `Restart=on-failure` + 5 s / SCM failure actions | `keep_alive true` (launchd respawn, ~10 s throttle) — the serial drivers' eager-exit loop carries over |
| `ConditionPathExists=` / demand-start | don't `brew services start` it |
| shared system user / LocalSystem | user-level LaunchAgent by default; `sudo brew services` → boot-time LaunchDaemon for a headless Mac (document both) |
| `/etc/rusty-photon` / `%PROGRAMDATA%` | whatever `rusty-photon-config` resolves on macOS (self-creation unchanged — see flagged unknowns) |
| rolling log files | `log_path` / `error_log_path` under Homebrew's `var/log` |

**Generation.** Versions and sha256s change nightly, so formulas are
stamped by CI — but to keep the explicitness rule meaningful, the
per-service metadata (description, port, service-block particulars) lives
as committed data in this repo; one template renders it with
version/url/sha. The same generator serves the stable channel from
`release.yml` (the PR-7 synergy) and pushes to the tap with the existing
`HOMEBREW_TAP_TOKEN`.

**Nightly leg** (`macos-latest`, arm64): build per-service tarballs —
per-service rather than one family tarball because each formula fetches
its own asset, and versioned filenames keep Homebrew's download cache
coherent — run `verify-brew.sh`, upload. The publish job attaches assets
and pushes the regenerated `-nightly` formulas together.

**`verify-brew.sh`** (the `verify-packages.sh` analogue, pre-publish):
render the formulas with `file://` URLs pointing at the just-built
tarballs → `brew install` → `brew services start` → port probe
(`/management/apiversions` for Alpaca services) → config self-created →
`brew services stop` → uninstall clean. Formulas also carry `test do`
blocks (`--help` probe) for `brew test`.

**Native-SDK services on macOS**: the workspace already builds on macOS
in `bazel.yml`, so SDK provisioning is solved; the packaging-specific
work is the zwo dylib payload/rpath and the QHY firmware story (flagged
unknowns below).

## Verification

- Per-leg lifecycle gates (N1 `verify-packages.sh` both arches, N2 Fedora
  leg, N3 `verify-msi.ps1` incl. the nightly-over-nightly upgrade, N4
  `verify-brew.sh`) run *before* publish; the publish job requires all
  enabled legs.
- First post-merge validation per phase: consume the nightly on a real
  machine — apt-upgrade an arm64 box from `0.1.0-1` to a nightly (N1),
  `brew install` + `brew services start` on a physical Mac (N4), install
  a nightly MSI over the previous one on the Windows box (N3) — and
  record results here.
- The skip-if-unchanged path and the failure-tracking issue get exercised
  naturally within the first week of N1 being live; confirm both behaved
  and note it here.

## Flagged unknowns (resolve during the noted phase)

- [ ] (N0) podman `--systemd=always` viability on the `ubuntu-24.04-arm`
      hosted image — the arm64 leg's load-bearing assumption.
- [ ] (N0) `+` / `^` in GitHub release asset filenames vs `gh`/`curl`
      URL-encoding → final filename convention.
- [ ] (N0) Homebrew version comparison of the nightly dialect (fallback:
      dot-rendered formula `version`).
- [ ] (N0) Hosted arm64 wall-clock + queue numbers → Orange Pi go/no-go.
- [ ] (N2) `cargo-generate-rpm` Version override with the `^` dialect.
- [ ] (N2) rpm package-name override (carried from service-packaging.md).
- [ ] (N4) zwo dylib payload on macOS: `@loader_path`-relative rpath
      instead of the Linux `/usr/lib/rusty-photon` RUNPATH; indi-3rdparty
      publishes the mac dylibs.
- [ ] (N4) qhy-camera firmware on macOS: the SDK uploads in-process
      (no udev/fxload), but firmware images must live where the SDK looks
      — likely a formula caveat + small helper; needs a real look.
- [ ] (N4) What `rusty-photon-config` resolves on macOS (`~/.config` vs
      `~/Library/Application Support`) — confirm and bless in docs; W2
      only pinned down Windows.
- [ ] (N4) sentinel's watchdog restart commands pointed at
      `brew services restart` — should be pure config, but untested.

## Future considerations

- An apt repository (and/or dnf copr) instead of release-asset downloads
  — `apt upgrade` picking up nightlies automatically is the nicest end
  state for the rigs; needs repo tooling + GPG signing + hosting.
- A rig-update helper consuming the stable nightly asset URLs (the
  rolling-tag design exists partly to enable this).
- Code signing / notarization post-1.0 (Developer ID, winget manifest —
  see windows-packaging.md's future considerations); a notarized suite
  `.pkg` could be revisited then, though the Homebrew model likely stays.
- Dated nightly archives (a second, pruned channel) if bisecting old
  nightlies ever becomes a real need — deliberately excluded from v1.
