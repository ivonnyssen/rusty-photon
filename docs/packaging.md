# Packaging & deployment guide

How to build, install, and operate the rusty-photon `.deb` / `.rpm`
packages on an observatory machine. Architecture decisions live in
[ADR-012](decisions/012-service-packaging-architecture.md) (naming, config
model, shared user, unit classes) and
[ADR-013](decisions/013-native-sdk-payload-policy.md) (native camera-SDK
payloads); the full design in
[docs/plans/service-packaging.md](plans/service-packaging.md); the
maintainer-script invariants in [packaging/README.md](../packaging/README.md).

Deployment is native packages by explicit decision — the drivers' USB /
udev / firmware needs and ASCOM Alpaca's UDP discovery defeat containers
(ADR-012).

Windows ships as one suite MSI instead of per-service packages — see
[docs/packaging-windows.md](packaging-windows.md) (ADR-015). macOS ships
per-service Homebrew formulas from the `ivonnyssen/homebrew-rusty-photon`
tap — see [docs/packaging-macos.md](packaging-macos.md).

## What gets installed

Every package is named `rusty-photon-<svc>` and installs
`/usr/bin/rusty-photon-<svc>` plus a hardened
`rusty-photon-<svc>.service` unit that is enabled and started on install.
All daemons run as the shared system user `rusty-photon` (home
`/var/lib/rusty-photon`, no login shell), created by the first package
installed. (`phd2-guider` was originally the one plain CLI package; it
gained a unit when its HTTP service mode landed — issue #464. Its binary
doubles as the PHD2 CLI via subcommands.)

| Service | Port | Notes |
|---------|------|-------|
| filemonitor | 11111 | Alpaca SafetyMonitor |
| ppba-driver | 11112 | serial (dialout) |
| qhy-focuser | 11113 | serial (dialout) |
| sentinel | 11114 | dashboard: `/` |
| rp | 11115 | orchestrator API |
| sky-survey-camera | 11116 | config-gated (see below) |
| star-adventurer-gti | 11117 | serial (dialout) |
| pa-falcon-rotator | 11118 | serial (dialout) |
| dsd-fp2 | 11119 | serial (dialout) |
| ui-htmx | 11120 | web config UI |
| qhy-camera | 11121 | USB camera; needs the firmware helper (below) |
| zwo-camera | 11122 | USB camera; its SDK blob bundled |
| pa-scops-oag | 11123 | serial (dialout) |
| zwo-focuser | 11124 | USB focuser; its SDK blob bundled |
| phd2-guider | 11130 | guider service wrapping PHD2 (PHD2 installed separately) |
| plate-solver | 11131 | config-gated; needs ASTAP (below) |
| calibrator-flats | 11170 | config-gated |

Alpaca UDP discovery is deliberately not served: with this many Alpaca
servers on one host they would collide on the discovery port. Point
clients (N.I.N.A. etc.) at `host:port` directly using the table above.

## Building packages

Packages are built natively on the target architecture — nightly in CI on
hosted x86_64 + arm64 runners (see [Nightly channel](#nightly-channel)),
or on demand directly on the rig / a dev box. From a repo checkout on a
Debian-family machine with Rust installed:

```sh
scripts/build-packages.sh                  # all services, .deb only
scripts/build-packages.sh --rpm            # also .rpm
scripts/build-packages.sh --services qhy-camera,filemonitor
scripts/build-packages.sh --skip-sdk-staging   # offline rebuild from cache
scripts/build-packages.sh --deb-version 0.1.0+nightly.20260712.gba09dc9
                                           # nightly version stamp (CI / rollback builds)
```

With `--rpm` and `--deb-version` together, the rpm version is derived
from the deb stamp by rendering `+nightly.` as rpm's `^` snapshot
separator (`0.1.0^20260712.gba09dc9`) — each packager renders its own
dialect of "sorts after the base release, before the next one".

The script installs apt build prerequisites, stages the pinned native
SDKs into `~/.cache/rusty-photon-pkg/` (QHYCCD static lib for the
link; per zwo service its ONE MIT blob, which also becomes that package's
payload per ADR-013 + ADR-014), then release-builds with the RUNPATH the
zwo packages need — the two zwo services each in their own cargo
invocation, so feature unification cannot re-union their per-device SDK
links — and runs `cargo deb` (and `cargo generate-rpm` with `--rpm`) per
service. Artifacts land in `dist/<version>/` with a `SHA256SUMS.txt`.

The QHY SDK version/sha256 pins and the ZWO blob ref are pinned in the
script and cross-checked by `scripts/check-pkg-assets.sh` against the
firmware helper and the CI SDK action, so shipped and CI-linked SDK bits
cannot drift apart.

## Nightly channel

CI publishes a rolling **`nightly` prerelease** built from the HEAD of
`main` whenever it has changed since the last publish
(`.github/workflows/nightly-packages.yml`): every packaged service as a
`.deb` *and* `.rpm` for both amd64/x86_64 and arm64/aarch64, each
package lifecycle-verified in a systemd container (Debian for the debs,
Fedora for the rpms) before anything is published, plus the Windows
suite MSI ([docs/packaging-windows.md](packaging-windows.md#nightly-channel))
and the macOS arm64 tarballs with their regenerated `-nightly` Homebrew
formulas ([docs/packaging-macos.md](packaging-macos.md#nightly-channel))
— all-or-nothing across the legs, so the release is always one coherent
commit with a complete asset set. There is one release and one tag;
assets are replaced on each publish, with no dated history.

Nightly debs carry the version `<base>+nightly.<date>.g<sha>` (e.g.
`0.1.0+nightly.20260712.gba09dc9`), which dpkg sorts above the plain
`<base>` release and below the next patch release — `apt` upgrades a
release install to a nightly in place, and the next release upgrades
over any nightly.

Nightly rpms carry `<base>^<date>.g<sha>` (e.g.
`0.1.0^20260712.gba09dc9`); rpm's `^` separator sorts the same way, so
`dnf` upgrades in place identically. One wrinkle: GitHub rewrites `^`
to `.` in uploaded asset names, so the *file* is called
`…-0.1.0.<date>.g<sha>-1.<arch>.rpm` while `rpm -q` after install shows
the true `^` version. `SHA256SUMS.txt` lists the dot-rendered names, so
checksums verify against the files as downloaded.

Filenames change nightly (they carry the version), so use
`SHA256SUMS.txt` — the one asset with a stable URL — as the index:

```sh
curl -fsSL https://github.com/ivonnyssen/rusty-photon/releases/download/nightly/SHA256SUMS.txt
# pick the file for your service + arch, then:
curl -fLO "https://github.com/ivonnyssen/rusty-photon/releases/download/nightly/<file>"
sha256sum -c --ignore-missing SHA256SUMS.txt
sudo apt-get install "./<file>"     # Debian-family
sudo dnf install "./<file>"         # Fedora
```

or, with the GitHub CLI (rpms: `--pattern 'rusty-photon-<svc>-*.<arch>.rpm'`
with `<arch>` = `x86_64` or `aarch64`):

```sh
gh release download nightly --repo ivonnyssen/rusty-photon \
    --pattern 'rusty-photon-<svc>_*_arm64.deb'
sudo apt-get install ./rusty-photon-<svc>_*_arm64.deb
```

Upgrading is installing a newer nightly the same way; a running unit is
restarted onto the new binary and the config untouched, as with any
package upgrade.

**Downgrades.** Once a machine runs nightlies, anything older is a
downgrade — an on-demand build stamped with the plain workspace
version, or an older nightly — and needs:

```sh
sudo apt-get install --allow-downgrades ./rusty-photon-<svc>_0.1.0-1_arm64.deb
sudo dnf downgrade ./rusty-photon-<svc>-0.1.0-1.<arch>.rpm      # Fedora
```

**Rolling back.** The channel keeps no history. To return to a
known-good state, downgrade to the plain release as above, or rebuild
the known-good commit on demand (add `--rpm` for the rpm set) and
install that the same downgrade way:

```sh
git checkout <known-good-sha>
scripts/build-packages.sh --deb-version "<base>+nightly.<date>.g<short-sha>"
```

(`<base>` = the workspace version at that commit.)

## Installing

```sh
sudo apt-get install ./rusty-photon-<svc>_*.deb
```

`apt-get install ./<file>` (not `dpkg -i`) resolves the runtime
dependencies. The unit is enabled and started immediately; on upgrade it is
restarted. On Fedora:

```sh
sudo dnf install ./rusty-photon-<svc>-*.rpm
sudo systemctl start rusty-photon-<svc>
```

The rpm enables the unit but — Fedora convention — does not start it:
start it once by hand (or reboot); upgrades restart a running unit and
leave a stopped one alone. Verify with:

```sh
systemctl status rusty-photon-<svc>
curl http://localhost:<port>/management/apiversions   # Alpaca services
```

**Config-gated services** (`sky-survey-camera`, `plate-solver`,
`calibrator-flats`) have no sensible default config, so their units carry
`ConditionPathExists=` on the config file: on a fresh install the unit
stays inactive (not failed) until you write
`/etc/rusty-photon/<svc>.json`, then `systemctl start rusty-photon-<svc>`.

**Serial-device drivers** (`ppba-driver`, `qhy-focuser`,
`pa-falcon-rotator`, `pa-scops-oag`, `dsd-fp2`, `star-adventurer-gti`) validate their
hardware eagerly at startup and exit if the device is missing — by design,
so a broken device is never advertised on the network. Until the device is
attached (and its path matches the config), the unit sits in a
restart-every-5s loop; it comes up by itself once the hardware appears.
The cameras and the network-only services serve with no hardware attached.

## Configuration

Packages ship no config files. Daemons self-create their config on first
start at `/var/lib/rusty-photon/.config/rusty-photon/<svc>.json` (the
shared user's XDG path), reachable via the `/etc/rusty-photon` symlink.
Exceptions: the config-gated three (above) never write one, and the two
cameras, zwo-focuser, and phd2-guider run on built-in defaults without
writing a file until settings are saved (via ui-htmx `config.apply`) or
one is created by hand at that path. To change settings:

```sh
sudo -e /etc/rusty-photon/<svc>.json
sudo systemctl reload rusty-photon-<svc>    # reload-capable services
sudo systemctl restart rusty-photon-<svc>   # the rest
```

Reload-capable (SIGHUP): filemonitor, ppba-driver, qhy-focuser,
sky-survey-camera, pa-falcon-rotator, pa-scops-oag, dsd-fp2,
star-adventurer-gti, qhy-camera, zwo-camera. Note that services with `config.apply` support
(via ui-htmx) rewrite these files at runtime — hand-edits and UI edits
share the same file.

## Camera specifics

**qhy-camera** — QHYCCD's SDK is proprietary and never redistributed
(ADR-013). After installing the package, run once, as root, with internet
access:

```sh
sudo rusty-photon-qhy-firmware-install
```

It downloads the pinned SDK release from qhyccd.com, verifies a pinned
sha256, and installs the camera firmware images, QHYCCD's udev
firmware-upload rules, and their FX3-capable `fxload`. An already-plugged
cold camera is flashed immediately (the helper re-emits udev add events);
otherwise firmware uploads on the next plug-in. Offline installs work; the
camera just stays unusable until the helper has run.

**zwo-camera / zwo-focuser** — nothing to do: each package bundles its own
MIT-licensed SDK blob at `/usr/lib/rusty-photon/` (`libASICamera2.so` /
`libEAFFocuser.so`; license in the package docdir), so the two co-install
cleanly (ADR-014). ZWO devices keep firmware in onboard flash.

Both camera packages install a udev rule granting the `plugdev` group
access to their USB VID (the service user is in `plugdev` via the unit's
`SupplementaryGroups=`).

## plate-solver: ASTAP

ASTAP is an external runtime dependency, deliberately not a package
dependency: install it separately (arm64/amd64 `.deb` from the
[ASTAP site](https://www.hnsky.org/astap.htm), plus a star database) and
point `astap_binary_path` / `astap_db_directory` in
`/etc/rusty-photon/plate-solver.json` at it.

## Removing

```sh
sudo apt-get remove rusty-photon-<svc>   # keeps the service's config + state
sudo apt-get purge rusty-photon-<svc>    # also deletes its config + state dir
```

The shared user, `/var/lib/rusty-photon`, and the `/etc/rusty-photon`
symlink are never removed (shared across packages, Debian convention for
system users). rpm has no purge lifecycle: erase behaves like `remove`;
to fully clean up after an erase, delete
`/var/lib/rusty-photon/.config/rusty-photon/<svc>.json` and
`/var/lib/rusty-photon/<svc>/` by hand.

## Verifying a build

```sh
scripts/verify-packages.sh            # all debs in dist/<version>/
scripts/verify-packages.sh --rpm      # the rpms, in a Fedora container
scripts/verify-packages.sh --services filemonitor,zwo-camera --keep
```

Runs a podman `--systemd=always` debian:trixie container and, per package:
install → unit active → config self-created → HTTP probe → remove (config
survives) → purge (config and state gone, shared pieces stay). The `--rpm`
flavor runs the same per-service checks in a Fedora container, adjusted
where rpm's lifecycle genuinely differs: it asserts the scriptlets'
enabled-but-not-started contract before starting each unit itself, its
`dnf install` doubles as the proof that every rpm's declared requires
resolve (nothing is preinstalled to compensate), and erase is verified as
remove-not-purge — config and state must survive. Gated
services verify enabled-but-inactive-and-not-failed instead; zwo-camera
additionally proves via `ldd` that each zwo binary resolves exactly its own
bundled blob through the RUNPATH — and does not link the other services'
SDKs (ADR-014). Rootless podman cannot apply the units' sandboxing, so
the script resets the hardening inside the container — hardening is
verified on real hosts with `systemd-analyze security
rusty-photon-<svc>.service`.

Expected `lintian` findings (accepted, not bugs):
`custom-library-search-path` on every package (the RUNPATH is injected
uniformly; only the zwo packages use it); `no-changelog` / `no-manual-page` /
`copyright-without-copyright-notice` pre-1.0; `unstripped-binary-or-object`
and `hardening-no-relro` on the vendored ZWO blobs (shipped exactly as
published); `embedded-library` on qhy-camera's statically linked SDK;
`appstream-metadata-missing-modalias-provide` on the camera packages' udev
rules; `empty-field Depends` and `unstripped-binary` on our own binaries
only on ad-hoc builds from non-Debian hosts.
