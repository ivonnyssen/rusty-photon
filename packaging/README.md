# Packaging assets

Canonical shared files for the per-service `.deb`/`.rpm` packages described
in [`docs/plans/service-packaging.md`](../docs/plans/service-packaging.md)
and [ADR-012](../docs/decisions/012-service-packaging-architecture.md).
Operator-facing docs (building, installing, configuring) live in
[`docs/packaging.md`](../docs/packaging.md).

## Layout

Every packaged daemon owns a `services/<svc>/pkg/` directory containing:

| File | Rule |
|------|------|
| `rusty-photon-<svc>.service` | systemd unit; `ExecStart=/usr/bin/rusty-photon-<svc>` with **no** `--config` flag (config is user-based XDG, self-created on first start) |
| `postinst` | byte-identical copy of [`postinst.common`](postinst.common) |
| `postrm` | byte-identical copy of [`postrm.common`](postrm.common) |

The maintainer scripts are deliberately service-agnostic: they derive the
service name from `$DPKG_MAINTSCRIPT_PACKAGE`, so the copies never diverge.
The camera packages (`qhy-camera`, `zwo-camera`) plus `zwo-focuser` are the
udev-shipping variant: their `postinst` must be exactly `postinst.common`
with [`postinst.udev-stanza`](postinst.udev-stanza) inserted before the
`#DEBHELPER#` line (they ship udev rules). The checker verifies that exact
construction, byte for byte. The stanza also prints a pointer to
`/usr/sbin/<pkg-minus-camera>-firmware-install` when the package ships one
(qhy-camera does — proprietary firmware is downloaded by the operator, never
packaged, per ADR-013; zwo-camera and zwo-focuser bundle their MIT blobs
instead, so the pointer is a no-op for both — `zwo-focuser`'s package name
doesn't even end in `-camera`, so the substitution leaves the helper path
unchanged and permanently missing, which is exactly the desired no-op).
These `pkg/` dirs additionally hold the udev rule file and the native-SDK
payload pieces, all listed as plain `assets` in the metadata blocks:
qhy-camera the firmware helper; zwo-camera and zwo-focuser each their own
committed `ZWO-SDK-LICENSE.txt` (checker-`cmp`'d against the copy vendored
with the libzwo-sys headers) plus a gitignored `lib/` dir into which
`scripts/build-packages.sh` stages that service's ONE MIT SDK blob right
before `cargo deb` runs — libzwo-sys links per device feature (ADR-014), so
zwo-camera's `lib/` carries exactly `libASICamera2.so` and zwo-focuser's
exactly `libEAFFocuser.so` (the link-search dir during the build is the
blob cache itself, not a pkg/lib dir). No blob appears in two packages, so
the zwo debs co-install without file conflicts.

`scripts/check-pkg-assets.sh` enforces all of this — run it after touching
anything under `packaging/` or a service's `pkg/` directory. It discovers
packaged services by the presence of `pkg/` dirs, so adding a service means:

1. copy `postinst.common` / `postrm.common` into `services/<svc>/pkg/`,
2. write `rusty-photon-<svc>.service` (start from filemonitor's and apply
   the service-class delta from the plan: serial → `SupplementaryGroups=dialout`;
   camera → `plugdev` + `AF_NETLINK`; network-only → `PrivateDevices=yes` +
   `MemoryDenyWriteExecute=yes`). Reload-capable services (those calling
   `ServiceRunner::with_reload`) add `ExecReload=/bin/kill -HUP $MAINPID`;
   services with no defaultable config gate on
   `ConditionPathExists=/var/lib/rusty-photon/.config/rusty-photon/<svc>.json`
   instead of crash-looping on a fresh install (both lists are enforced by
   the checker),
3. add the `[package.metadata.deb]` / `[package.metadata.generate-rpm]`
   blocks to the service's `Cargo.toml` (names all `rusty-photon-<svc>`),
4. run `scripts/check-pkg-assets.sh`.
