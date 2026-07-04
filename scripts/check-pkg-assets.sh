#!/bin/sh
# Asserts the per-service packaging invariants documented in
# packaging/README.md and docs/plans/service-packaging.md.
# Run from the repo root; exits non-zero on any violation.
set -eu

fail=0
err() { echo "check-pkg-assets: $*" >&2; fail=1; }

[ -f packaging/postinst.common ] || { echo "check-pkg-assets: run from the repo root" >&2; exit 2; }

for pkgdir in services/*/pkg; do
    [ -d "$pkgdir" ] || continue
    svc=$(basename "$(dirname "$pkgdir")")
    name="rusty-photon-$svc"
    unit="$pkgdir/$name.service"
    toml="services/$svc/Cargo.toml"

    # CLI-only packages: binary asset only, no daemon invariants.
    case "$svc" in
        phd2-guider) continue ;;
    esac

    if [ ! -f "$unit" ]; then
        err "$svc: missing $unit"
    else
        grep -q "^ExecStart=/usr/bin/$name\$" "$unit" \
            || err "$svc: ExecStart must be exactly /usr/bin/$name (config is XDG-resolved; no --config flag)"
    fi

    for s in postinst postrm; do
        cmp -s "packaging/$s.common" "$pkgdir/$s" \
            || err "$svc: pkg/$s differs from packaging/$s.common"
    done

    grep -A 2 '^\[package\.metadata\.deb\]' "$toml" | grep -q "name = \"$name\"" \
        || err "$svc: [package.metadata.deb] name must be \"$name\""
    grep -q "unit-name = \"$name\"" "$toml" \
        || err "$svc: [package.metadata.deb.systemd-units] unit-name must be \"$name\""
    grep -A 2 '^\[package\.metadata\.generate-rpm\]' "$toml" | grep -q "name = \"$name\"" \
        || err "$svc: [package.metadata.generate-rpm] name must be \"$name\""
done

# The QHY SDK version is pinned in two shipped places (ADR-013); they must match.
bp=scripts/build-packages.sh
fw=services/qhy-camera/pkg/rusty-photon-qhy-firmware-install
if [ -f "$bp" ] && [ -f "$fw" ]; then
    v1=$(sed -n 's/^QHY_SDK_VERSION="\(.*\)"$/\1/p' "$bp" | head -1)
    v2=$(sed -n 's/^VERSION="\(.*\)"$/\1/p' "$fw" | head -1)
    if [ -z "$v1" ] || [ "$v1" != "$v2" ]; then
        err "QHY SDK version pin mismatch: build-packages.sh='$v1' vs firmware-install='$v2'"
    fi
fi

if [ "$fail" -eq 0 ]; then
    echo "check-pkg-assets: OK"
fi
exit "$fail"
