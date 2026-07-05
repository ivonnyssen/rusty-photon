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

## What gets installed

Every package is named `rusty-photon-<svc>` and installs
`/usr/bin/rusty-photon-<svc>` plus (for daemons) a hardened
`rusty-photon-<svc>.service` unit that is enabled and started on install.
All daemons run as the shared system user `rusty-photon` (home
`/var/lib/rusty-photon`, no login shell), created by the first package
installed. `phd2-guider` is the one plain CLI package (no unit, no user).

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
| zwo-camera | 11122 | USB camera; SDK blobs bundled |
| plate-solver | 11131 | config-gated; needs ASTAP (below) |
| calibrator-flats | 11170 | config-gated |

## Building packages

Packages are built natively on the target architecture — arm64 directly on
the rig, x86_64 on a dev box (CI packaging for arm64 is deferred; see the
plan). From a repo checkout on a Debian-family machine with Rust installed:

```sh
scripts/build-packages.sh                  # all services, .deb only
scripts/build-packages.sh --rpm            # also .rpm (dev-box convenience)
scripts/build-packages.sh --services qhy-camera,filemonitor
scripts/build-packages.sh --skip-sdk-staging   # offline rebuild from cache
```

The script installs apt build prerequisites, stages the pinned native
camera SDKs into `~/.cache/rusty-photon-pkg/` (QHYCCD static lib for the
link; ZWO MIT blobs, which also become package payload per ADR-013), builds
everything in one release pass with the RUNPATH the zwo-camera package
needs, then runs `cargo deb` (and `cargo generate-rpm` with `--rpm`) per
service. Artifacts land in `dist/<version>/` with a `SHA256SUMS.txt`.

The QHY SDK version/sha256 pins and the ZWO blob ref are pinned in the
script and cross-checked by `scripts/check-pkg-assets.sh` against the
firmware helper and the CI SDK action, so shipped and CI-linked SDK bits
cannot drift apart.

## Installing

```sh
sudo apt-get install ./rusty-photon-<svc>_*.deb
```

`apt-get install ./<file>` (not `dpkg -i`) resolves the runtime
dependencies. The unit is enabled and started immediately; on upgrade it is
restarted. Verify with:

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
`pa-falcon-rotator`, `dsd-fp2`, `star-adventurer-gti`) validate their
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
cameras run on built-in defaults without writing a file until settings
are saved (via ui-htmx `config.apply`) or one is created by hand at that
path. To change settings:

```sh
sudo -e /etc/rusty-photon/<svc>.json
sudo systemctl reload rusty-photon-<svc>    # reload-capable services
sudo systemctl restart rusty-photon-<svc>   # the rest
```

Reload-capable (SIGHUP): filemonitor, ppba-driver, qhy-focuser,
sky-survey-camera, pa-falcon-rotator, dsd-fp2, star-adventurer-gti,
qhy-camera, zwo-camera. Note that services with `config.apply` support
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
firmware-upload rules, and their FX3-capable `fxload`. Then unplug/replug
the camera — udev uploads firmware on plug-in. Offline installs work; the
camera just stays unusable until the helper has run.

**zwo-camera** — nothing to do: the MIT-licensed SDK blobs are bundled at
`/usr/lib/rusty-photon/` (license in
`/usr/share/doc/rusty-photon-zwo-camera/`). ZWO cameras keep firmware in
onboard flash.

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
scripts/verify-packages.sh --services filemonitor,zwo-camera --keep
```

Runs a podman `--systemd=always` debian:trixie container and, per package:
install → unit active → config self-created → HTTP probe → remove (config
survives) → purge (config and state gone, shared pieces stay). Gated
services verify enabled-but-inactive-and-not-failed instead; zwo-camera
additionally proves via `ldd` that the bundled blobs resolve through the
binary's RUNPATH. Rootless podman cannot apply the units' sandboxing, so
the script resets the hardening inside the container — hardening is
verified on real hosts with `systemd-analyze security
rusty-photon-<svc>.service`.

Expected `lintian` findings (accepted, not bugs):
`custom-library-search-path` on zwo-camera (the RUNPATH is the design);
`no-changelog` / `no-manual-page` / `copyright-without-copyright-notice`
pre-1.0; `empty-field Depends` and `unstripped-binary` appear only on
ad-hoc builds from non-Debian hosts.
