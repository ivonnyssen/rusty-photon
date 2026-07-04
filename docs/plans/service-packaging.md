# Service Packaging Plan — `.deb` / `.rpm` for the whole family

## Goal

Provide installable `.deb` and `.rpm` packages for every rusty-photon service
so an observatory machine (first target: the Debian 13 arm64 test rig) can be
provisioned with `apt install` — no Rust toolchain, no hand-copied binaries —
with systemd supervising every service. This generalizes the proven
single-service pattern from
[`filemonitor-packaging.md`](archive/filemonitor-packaging.md) (PR #33),
superseding several of its per-service decisions; the architecture is recorded
in [ADR-012](../decisions/012-service-packaging-architecture.md) and the
native-SDK payload policy in
[ADR-013](../decisions/013-native-sdk-payload-policy.md).

Deployment is native packages, not containers, by explicit decision: the
drivers' USB/udev/firmware needs and ASCOM Alpaca's UDP discovery would force
`--network host` + privileged device passthrough, erasing container isolation
while keeping host-level setup anyway (see ADR-012 Context).

## Implementation Status

| Phase | Description | Status | Branch / PR |
|-------|-------------|--------|-------------|
| PR-1 | This plan + ADR-012 + ADR-013 + archive old plan | In progress | `feature/service-packaging-plan` |
| PR-2 | Migrate filemonitor to the new pattern (template proof) + filemonitor/sentinel XDG fix + `check-pkg-assets.sh` | In progress | `feature/service-packaging-filemonitor` |
| PR-3 | 11 pure-Rust daemons + `phd2-guider` CLI package | Pending | |
| PR-4 | `qhy-camera` package + firmware downloader helper | Pending | |
| PR-5 | `zwo-camera` package with bundled MIT SDK blobs | Pending | |
| PR-6 | `build-packages.sh` + `verify-packages.sh` + `docs/packaging.md`; first on-rig install | Pending | |
| PR-7 | `release.yml` generalization (x86_64 matrix, Homebrew rename) | Deferred | |

Carried over from the superseded plan (still open, now owned by PR-7):
create the `ivonnyssen/homebrew-rusty-photon` tap content and add the
`HOMEBREW_TAP_TOKEN` secret.

## Decisions (fixed — see ADR-012 / ADR-013 for rationale)

- **Scope:** all services. 14 systemd-supervised daemons + `phd2-guider`
  packaged as a plain CLI (it is a clap subcommand tool, not a daemon).
  Test-double bins (`mock_phd2`, `mock_astap`) are never packaged.
- **Naming:** `rusty-photon-` prefix on packages **and** installed binaries
  **and** units: package `rusty-photon-<svc>`, `/usr/bin/rusty-photon-<svc>`,
  `rusty-photon-<svc>.service`. Cargo bin names are unchanged; the rename
  happens in the packaging asset mapping. filemonitor's bare-named
  package/binary/unit are renamed (breaking; acceptable pre-1.0).
- **Config is user-based XDG, owned by the service** — packages ship **no
  config file** and units pass **no `--config` flag**. Services resolve
  `~/.config/rusty-photon/<svc>.json` via `resolve_config_path`
  (`crates/rusty-photon-config`) and self-create it on first start
  (`materialize_identity`). With the shared system user's home at
  `/var/lib/rusty-photon`, live config lands at
  `/var/lib/rusty-photon/.config/rusty-photon/<svc>.json`. postinst creates
  an `/etc/rusty-photon` → `/var/lib/rusty-photon/.config/rusty-photon`
  symlink for operator discoverability. No dpkg conffiles anywhere (six
  services rewrite their config at runtime via `config.apply` /
  `materialize_identity`; conffile semantics cannot express that).
- **One shared system user** `rusty-photon` (home `/var/lib/rusty-photon`,
  no login shell). Hardware privileges are scoped per unit via
  `SupplementaryGroups=` (`dialout` for serial, `plugdev` for cameras), not
  via the user. Enables the shared-home XDG config model and the
  rp ↔ plate-solver shared FITS tree, and keeps deb maintainer scripts
  byte-identical across packages.
- **Formats:** `.deb` + `.rpm` only for the family. MSI and Homebrew remain
  filemonitor-only until there is a real Windows/macOS deployment need.
- **ARM64 packages are built natively on the rig for now** via
  `scripts/build-packages.sh`; CI packaging for arm64 is deferred (PR-7 keeps
  x86_64 in `release.yml`).
- **Native SDK payloads (ADR-013):** ZWO's MIT-licensed `.so` blobs are
  redistributed inside `rusty-photon-zwo-camera`; QHYCCD's proprietary
  firmware is **never** redistributed — a pinned, checksum-verified
  downloader helper installs it on the target machine.

## Design

### Asset layout — per-service `pkg/` dirs + a consistency checker

Plain committed files, no generator (explicitness over DRY; `git grep` must
not lie). Shared canonical copies live in `packaging/`; a checker keeps the
per-service instances convergent.

```
packaging/
  postinst.common        # canonical; every daemon pkg/postinst is byte-identical
  postrm.common          # canonical; ditto (qhy/zwo: documented +udevadm variant)
  README.md              # invariants, how to add a service
scripts/
  check-pkg-assets.sh    # asserts the invariants below
  build-packages.sh      # on-device package build (PR-6)
  verify-packages.sh     # container install/upgrade/purge verification (PR-6)
services/<svc>/pkg/
  rusty-photon-<svc>.service
  postinst               # copy of packaging/postinst.common
  postrm                 # copy of packaging/postrm.common
```

Extras: `services/qhy-camera/pkg/` adds `70-rusty-photon-qhy.rules` +
`rusty-photon-qhy-firmware-install`; `services/zwo-camera/pkg/` adds
`70-rusty-photon-zwo.rules` + a gitignored `lib/` dir into which the build
script stages the ZWO blobs + license before `cargo deb` runs.

`check-pkg-assets.sh` asserts, per daemon crate: unit file named
`rusty-photon-<dir>.service` with `ExecStart=/usr/bin/rusty-photon-<dir>`
(no `--config`); `postinst`/`postrm` byte-identical to the canonical copies
(or to the documented camera variant); `[package.metadata.deb] name`,
`unit-name`, and `[package.metadata.generate-rpm] name` all equal
`rusty-photon-<dir>`; reload-capable services have `ExecReload`; the QHY SDK
version pinned in `build-packages.sh` matches the one in
`rusty-photon-qhy-firmware-install`.

### systemd unit template

```ini
[Unit]
Description=Rusty Photon <svc> — <role> (port <port>)
After=network.target

[Service]
Type=simple
ExecStart=/usr/bin/rusty-photon-<svc>
ExecReload=/bin/kill -HUP $MAINPID        # reload-capable services only
Restart=on-failure
RestartSec=5
User=rusty-photon
Group=rusty-photon
Environment=RUST_LOG=info
Environment=HOME=/var/lib/rusty-photon
WorkingDirectory=/var/lib/rusty-photon
StateDirectory=rusty-photon/<svc>

# Hardening — hobby-rig level; must not break config write-back, serial, USB.
NoNewPrivileges=yes
ProtectSystem=strict
ReadWritePaths=/var/lib/rusty-photon
ProtectHome=yes
PrivateTmp=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictSUIDSGID=yes
LockPersonality=yes
RestrictRealtime=yes
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
UMask=0027

[Install]
WantedBy=multi-user.target
```

Notes: `ConfigurationDirectory=` is deliberately **not** used (systemd
creates it root-owned, which would break the config crate's atomic
tmp+rename+fsync-dir write-back). `HOME` is set explicitly as belt-and-braces
even though the `dirs` crate falls back to the passwd entry.
`ReadWritePaths=/var/lib/rusty-photon` covers both the XDG config write-back
and the rp ↔ plate-solver shared FITS tree.

Per-class deltas:

| Class | Services | Delta |
|-------|----------|-------|
| serial | ppba-driver, qhy-focuser, pa-falcon-rotator, dsd-fp2, star-adventurer-gti | `SupplementaryGroups=dialout` |
| USB camera | qhy-camera, zwo-camera | `SupplementaryGroups=plugdev`, `RestrictAddressFamilies` += `AF_NETLINK` (libusb hotplug), no `MemoryDenyWriteExecute` (vendor blob caution) |
| network-only | filemonitor, sentinel, rp, ui-htmx, plate-solver, calibrator-flats, sky-survey-camera | `PrivateDevices=yes`, `MemoryDenyWriteExecute=yes` |

Reload-capable (get `ExecReload`): filemonitor, zwo-camera,
pa-falcon-rotator, star-adventurer-gti, dsd-fp2, qhy-camera, ppba-driver,
sky-survey-camera, qhy-focuser.

Ports (for unit descriptions): filemonitor 11111, ppba-driver 11112,
qhy-focuser 11113, sentinel 11114, rp 11115, sky-survey-camera 11116,
pa-falcon-rotator 11118, dsd-fp2 11119, ui-htmx 11120, qhy-camera 11121,
zwo-camera 11122, plate-solver 11131, calibrator-flats 11170,
star-adventurer-gti 11880.

### Maintainer scripts

`packaging/postinst.common` (deb; byte-identical in every daemon package):

```sh
#!/bin/sh
set -e
if ! getent passwd rusty-photon > /dev/null; then
    adduser --system --group --home /var/lib/rusty-photon --quiet rusty-photon
fi
install -d -m 0750 -o rusty-photon -g rusty-photon /var/lib/rusty-photon
if [ ! -e /etc/rusty-photon ]; then
    ln -s /var/lib/rusty-photon/.config/rusty-photon /etc/rusty-photon
fi
#DEBHELPER#
```

`packaging/postrm.common`:

```sh
#!/bin/sh
set -e
SVC="${DPKG_MAINTSCRIPT_PACKAGE#rusty-photon-}"
if [ "$1" = "purge" ]; then
    rm -f "/var/lib/rusty-photon/.config/rusty-photon/$SVC.json"
    rm -rf "/var/lib/rusty-photon/$SVC"
fi
#DEBHELPER#
```

The shared user, home, and `/etc/rusty-photon` symlink are never removed on
purge (shared across packages; Debian convention keeps system users). The
camera packages' postinst appends a `udevadm control --reload-rules &&
udevadm trigger || true` stanza — the one sanctioned variant, checked by
`check-pkg-assets.sh`.

RPM mirrors this via `post_install_script` / `pre_uninstall_script` /
`post_uninstall_script` (plain `systemctl` calls — cargo-generate-rpm does
not process rpm macros). rpm has **no purge lifecycle**: erase preserves the
runtime-created config and state (parity with `dpkg remove`); removing them
is a documented manual step in `docs/packaging.md`. Camera packages additionally
`getent group plugdev >/dev/null || groupadd -r plugdev` (the group is not
standard on RPM distros).

### Code changes required (PR-2 / PR-3)

Survey result: the 8 hardware drivers use `resolve_config_path` (+
`materialize_identity`), but **six services do not**: `filemonitor`
(CWD-relative `config.json` default), `sentinel` (loads config only with
`--config`, else silent in-memory defaults), and `rp` / `ui-htmx` /
`plate-solver` / `calibrator-flats`. All six adopt the same pattern:
`resolve_config_path("<svc>", args.config)` + the new
`rusty_photon_config::init_file_if_absent` (writes the typed default config
on first start, so a packaged install materializes an editable file).
Self-creation applies **only to the XDG default path**: an explicit
`--config` naming a missing file stays a hard error (fail-fast contract —
a typo'd path must never silently run on defaults; filemonitor's
integration test and BDD scenarios pin this).
filemonitor + sentinel land in PR-2 (filemonitor also gains an
`impl Default for Config` based on its previously packaged default, watch
path moved to `/var/lib/rusty-photon/filemonitor/`); the remaining four land
in PR-3 alongside their packaging.

### qhy-camera (PR-4)

- `libqhyccd.a` is linked **statically** → runtime NEEDED is only
  `libusb-1.0.so.0` + `libstdc++.so.6`; deb `depends = "$auto"` resolves them
  via dpkg-shlibdeps.
- Own-authored udev rule `70-rusty-photon-qhy.rules` →
  `/usr/lib/udev/rules.d/` (never `/etc/udev/rules.d` from a package):

  ```
  SUBSYSTEMS=="usb", ATTRS{idVendor}=="1618", GROUP="plugdev", MODE="0660"
  ACTION=="add", SUBSYSTEMS=="usb", ATTRS{idVendor}=="1618", RUN+="/bin/sh -c 'echo 200 > /sys/module/usbcore/parameters/usbfs_memory_mb'"
  ```

- `/usr/sbin/rusty-photon-qhy-firmware-install` (root-only, run manually;
  postinst prints a pointer but never downloads — offline installs must not
  fail): downloads the QHYCCD SDK pinned at **26.06.04** from
  `https://www.qhyccd.com/file/repository/publish/SDK/260604/` —
  `sdk_linux_arm64_26.06.04.tar.gz` (aarch64) or
  `sdk_linux64_26.06.04.tar.gz` (x86_64) — verifies a pinned sha256, and
  installs **only the firmware files** to `/lib/firmware/qhy`. It installs
  neither `libqhyccd.so` (we link statically) nor the SDK's udev rules (we
  ship our own). `--force` re-installs.

### zwo-camera (PR-5)

- MIT blobs `libASICamera2.so` + `libEFWFilter.so` (from the same pinned
  indi-3rdparty commit `.github/actions/install-zwo-sdk` uses) are packaged
  at `/usr/lib/rusty-photon/`, license at
  `/usr/share/doc/rusty-photon-zwo-camera/ZWO-SDK-LICENSE.txt`.
- The blobs carry **no SONAME** → loader resolution via **RUNPATH**:
  `build-packages.sh` exports
  `RUSTFLAGS="-C link-arg=-Wl,-rpath,/usr/lib/rusty-photon"` for the release
  build (uniform across binaries; harmless where unused; deliberately not a
  `build.rs` change, which would ripple into Bazel/repin). Expected lintian
  finding `custom-library-search-path` is documented, not fixed.
- deb `depends` are **explicit** (`libc6, libgcc-s1, libstdc++6,
  libusb-1.0-0, libudev1`) — dpkg-shlibdeps cannot map SONAME-less private
  blobs and `$auto` would fail.
- Build staging: blobs downloaded into gitignored
  `services/zwo-camera/pkg/lib/`; `ZWO_SDK_LIB_DIR` points there for the
  link; `cargo deb` picks them up as plain assets.

### phd2-guider (PR-3)

CLI package: binary asset (`/usr/bin/rusty-photon-phd2-guider`) only. No
unit, no user, no maintainer scripts beyond defaults.

### plate-solver note

ASTAP is an external runtime dependency (Recommends-level, not a hard dep):
the operator installs it separately (arm64 `.deb` from the ASTAP site) and
points the service config at the binary. Documented in `docs/packaging.md`.

### On-device build — `scripts/build-packages.sh` (PR-6)

Runs on Debian arm64 (the rig) and x86_64 dev boxes:

1. apt prereqs (idempotent): `build-essential pkg-config curl git libssl-dev
   libcfitsio-dev libusb-1.0-0-dev libudev-dev clang libclang-dev dpkg-dev
   ca-certificates`; `cargo install --locked cargo-deb` (3.6.x);
   `cargo-generate-rpm` only with `--rpm`.
2. Stage QHY SDK into `~/.cache/rusty-photon-pkg/` (same pinned URL + sha256
   as the firmware helper — the checker asserts the pins match), export
   `QHYCCD_SDK_DIR=<extracted>/usr/local/lib`.
3. Stage ZWO blobs into `services/zwo-camera/pkg/lib/`, export
   `ZWO_SDK_LIB_DIR` to it.
4. `RUSTFLAGS="-C link-arg=-Wl,-rpath,/usr/lib/rusty-photon" cargo build
   --release -p <all service crates>`; strip binaries.
5. Per service: `cargo deb -p <crate> --no-build --no-strip` (`--no-build`
   is essential — a rebuild would lose the staged env/RUSTFLAGS). With
   `--rpm` on x86_64: `cargo generate-rpm -p services/<dir>`.
6. Collect into `dist/<version>/` + `SHA256SUMS.txt`.
7. Flags: `--services a,b,c` (subset), `--rpm`, `--skip-sdk-staging`
   (offline rebuild from cache).

## Verification

- `lintian` on all debs (documented expected findings:
  `custom-library-search-path` on zwo-camera; `no-changelog` /
  `no-manual-page` / `copyright-without-copyright-notice` accepted pre-1.0;
  `empty-field Depends` + `unstripped-binary` appear only on ad-hoc non-Debian
  host builds — Debian/CI builds run dpkg-shlibdeps and strip). All daemon
  packages carry `depends = "$auto, adduser"` (postinst uses adduser).
  `rpmlint` on rpms.
- **Rootless-container caveat (PR-2 finding):** rootless podman cannot apply
  the units' sandboxing (mount-namespace + seccomp setup across the `User=`
  switch fails with `217/USER` / `226/NAMESPACE`). `verify-packages.sh` must
  install a drop-in resetting the whole hardening block inside the container
  — packaging lifecycle is what containers verify; the hardening itself is
  verified on real hosts (`systemd-analyze security` + active unit on the
  rig).
- `scripts/verify-packages.sh` in a podman `--systemd=always` `debian:trixie`
  container (arm64 natively on the rig, x86_64 on the dev box), per service:
  install → unit `active` → config self-created at
  `/var/lib/rusty-photon/.config/rusty-photon/<svc>.json` with a minted
  `unique_id` → port probe (`/management/apiversions` for Alpaca services;
  service-specific endpoint otherwise) → remove (config survives) → purge
  (config + state gone; user/home/symlink stay).
- Cameras start with zero devices attached (qhy-camera contract C0:
  warn-and-serve). zwo-camera: `ldd` resolves `libASICamera2.so` to
  `/usr/lib/rusty-photon/` (RUNPATH proof).
- `config.apply` write-back on dsd-fp2 under the unit sandbox.
- Upgrade path: rebuild one service with `--deb-version 0.1.1-1`, install
  over: live config untouched, no prompts, unit restarted
  (`restart-after-upgrade`).
- `systemd-analyze security` once per class; record scores here.
- On-rig only: QHY firmware helper end-to-end with a real camera (download,
  sha256, replug, enumeration under plugdev/0660); serial access via
  dialout; Alpaca UDP discovery from another host.

## Flagged unknowns (resolve during PR-2/PR-4)

- [ ] Firmware directory the static libqhyccd expects at runtime
      (`strings libqhyccd.a | grep -i firmware`); adjust the helper's
      destination if not `/lib/firmware/qhy`.
- [ ] Whether libqhyccd uploads firmware in-process via libusb (firmware
      files + our udev rule suffice) or depends on the SDK's udev/fxload
      rules for cold devices; whether the rig's camera cold-enumerates under
      the Cypress VID `04b4` (add a second rule line if so).
- [ ] sha256 pins for both SDK archives (capture on first download).
- [ ] `cargo-generate-rpm` support for a package `name` override (if
      missing: document and keep rpm crate-named until upstream supports it).
- [ ] `dirs`-crate home resolution under systemd `User=` without `$HOME`
      (units set `HOME` explicitly as belt-and-braces; confirm the fallback
      works anyway).

## Future considerations

- PR-7: generalize `release.yml` to a service matrix (x86_64 deb/rpm),
  rename tarballs/Homebrew formula (`Formula/rusty-photon-filemonitor.rb`,
  class `RustyPhotonFilemonitor`); finish the tap setup carried over from
  the superseded plan.
- CI-built arm64 packages (Pi runner or cross with staged arm64 SDKs).
- An apt repository (and/or dnf copr) instead of GitHub-release attachments.
- `sky-survey-camera` and other simulators could later split into a
  `-simulators` meta-package if rig installs want to exclude them.
