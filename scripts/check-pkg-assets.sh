#!/bin/sh
# Asserts the per-service packaging invariants documented in
# packaging/README.md and docs/plans/service-packaging.md.
# Run from the repo root; exits non-zero on any violation.
set -eu

fail=0
err() { echo "check-pkg-assets: $*" >&2; fail=1; }

# Print the body of one TOML table (from its [header] to the next [header]),
# so key checks are position-independent within the section.
toml_section() { # $1=file $2=exact table name
    awk -v want="[$2]" '
        /^\[/ { in_section = ($0 == want); next }
        in_section { print }
    ' "$1"
}

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

    # postrm is byte-identical everywhere. postinst is byte-identical too,
    # except the camera packages: theirs must equal postinst.common with the
    # canonical udev stanza inserted before #DEBHELPER# (still deterministic).
    cmp -s "packaging/postrm.common" "$pkgdir/postrm" \
        || err "$svc: pkg/postrm differs from packaging/postrm.common"
    case "$svc" in
        qhy-camera|zwo-camera)
            expected=$(mktemp)
            awk '/^#DEBHELPER#/ { while ((getline line < "packaging/postinst.udev-stanza") > 0) print line } { print }' \
                packaging/postinst.common > "$expected"
            cmp -s "$expected" "$pkgdir/postinst" \
                || err "$svc: pkg/postinst must be postinst.common + udev stanza before #DEBHELPER#"
            rm -f "$expected"
            ;;
        *)
            cmp -s "packaging/postinst.common" "$pkgdir/postinst" \
                || err "$svc: pkg/postinst differs from packaging/postinst.common"
            ;;
    esac

    toml_section "$toml" "package.metadata.deb" | grep -q "^name = \"$name\"" \
        || err "$svc: [package.metadata.deb] name must be \"$name\""
    toml_section "$toml" "package.metadata.deb.systemd-units" | grep -q "^unit-name = \"$name\"" \
        || err "$svc: [package.metadata.deb.systemd-units] unit-name must be \"$name\""
    toml_section "$toml" "package.metadata.generate-rpm" | grep -q "^name = \"$name\"" \
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
