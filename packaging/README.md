# Packaging assets

Canonical shared files for the per-service `.deb`/`.rpm` packages described
in [`docs/plans/service-packaging.md`](../docs/plans/service-packaging.md)
and [ADR-012](../docs/decisions/012-service-packaging-architecture.md).

## Layout

Every packaged daemon owns a `services/<svc>/pkg/` directory containing:

| File | Rule |
|------|------|
| `rusty-photon-<svc>.service` | systemd unit; `ExecStart=/usr/bin/rusty-photon-<svc>` with **no** `--config` flag (config is user-based XDG, self-created on first start) |
| `postinst` | byte-identical copy of [`postinst.common`](postinst.common) |
| `postrm` | byte-identical copy of [`postrm.common`](postrm.common) |

The maintainer scripts are deliberately service-agnostic: they derive the
service name from `$DPKG_MAINTSCRIPT_PACKAGE`, so the copies never diverge.
The camera packages (`qhy-camera`, `zwo-camera`) are the one sanctioned
variant: their `postinst` must be exactly `postinst.common` with
[`postinst.udev-stanza`](postinst.udev-stanza) inserted before the
`#DEBHELPER#` line (they ship udev rules). The checker verifies that exact
construction, byte for byte.

`scripts/check-pkg-assets.sh` enforces all of this — run it after touching
anything under `packaging/` or a service's `pkg/` directory. It discovers
packaged services by the presence of `pkg/` dirs, so adding a service means:

1. copy `postinst.common` / `postrm.common` into `services/<svc>/pkg/`,
2. write `rusty-photon-<svc>.service` (start from filemonitor's and apply
   the service-class delta from the plan: serial → `SupplementaryGroups=dialout`;
   camera → `plugdev` + `AF_NETLINK`; network-only → `PrivateDevices=yes` +
   `MemoryDenyWriteExecute=yes`),
3. add the `[package.metadata.deb]` / `[package.metadata.generate-rpm]`
   blocks to the service's `Cargo.toml` (names all `rusty-photon-<svc>`),
4. run `scripts/check-pkg-assets.sh`.
