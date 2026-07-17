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

    if [ ! -f "$unit" ]; then
        err "$svc: missing $unit"
    else
        grep -q "^ExecStart=/usr/bin/$name\$" "$unit" \
            || err "$svc: ExecStart must be exactly /usr/bin/$name (config is XDG-resolved; no --config flag)"
        # Reload-capable services (ServiceRunner::with_reload) expose SIGHUP.
        case "$svc" in
            filemonitor|ppba-driver|qhy-focuser|sky-survey-camera|pa-falcon-rotator|pa-scops-oag|dsd-fp2|star-adventurer-gti|qhy-camera|zwo-camera|zwo-focuser)
                grep -q '^ExecReload=/bin/kill -HUP \$MAINPID$' "$unit" \
                    || err "$svc: reload-capable service must have ExecReload=/bin/kill -HUP \$MAINPID"
                ;;
        esac
        # Services with no defaultable config gate on the config file existing
        # instead of crash-looping on a fresh install.
        case "$svc" in
            sky-survey-camera|plate-solver|calibrator-flats)
                grep -q "^ConditionPathExists=/var/lib/rusty-photon/\.config/rusty-photon/$svc\.json\$" "$unit" \
                    || err "$svc: no-default-config service must gate on ConditionPathExists=<XDG config path>"
                ;;
        esac
    fi

    # postrm is byte-identical everywhere. postinst is byte-identical too,
    # except the udev-shipping packages: theirs must equal postinst.common
    # with the canonical udev stanza inserted before #DEBHELPER# (still
    # deterministic). zwo-focuser reuses the same camera-class stanza even
    # though its firmware lives in onboard flash (like the ASI camera and
    # EFW) — the stanza's fw_helper check is a harmless no-op for it (the
    # package name doesn't end in "-camera", so the helper path can never
    # exist), and a byte-identical shared file beats a third variant.
    cmp -s "packaging/postrm.common" "$pkgdir/postrm" \
        || err "$svc: pkg/postrm differs from packaging/postrm.common"
    case "$svc" in
        qhy-camera|zwo-camera|zwo-focuser)
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

    # Camera packages ship their own udev rule (the postinst udev stanza
    # reloads rules on install, so the rule file must actually be there).
    # Sentinel ships the polkit rule that authorizes its restart commands
    # (no postinst stanza needed — polkitd hot-reloads rules.d).
    case "$svc" in
        sentinel)
            [ -f "$pkgdir/50-rusty-photon-sentinel.rules" ] \
                || err "$svc: missing pkg/50-rusty-photon-sentinel.rules"
            ;;
        qhy-camera)
            [ -f "$pkgdir/90-rusty-photon-qhy.rules" ] \
                || err "$svc: missing pkg/90-rusty-photon-qhy.rules"
            ;;
        zwo-camera)
            [ -f "$pkgdir/90-rusty-photon-zwo.rules" ] \
                || err "$svc: missing pkg/90-rusty-photon-zwo.rules"
            ;;
        zwo-focuser)
            # Uniquely named (not zwo-camera's 90-rusty-photon-zwo.rules) so
            # both packages can install their udev rule on the same host
            # without a dpkg/rpm filename collision.
            [ -f "$pkgdir/90-rusty-photon-zwo-focuser.rules" ] \
                || err "$svc: missing pkg/90-rusty-photon-zwo-focuser.rules"
            ;;
    esac

    toml_section "$toml" "package.metadata.deb" | grep -q "^name = \"$name\"" \
        || err "$svc: [package.metadata.deb] name must be \"$name\""
    toml_section "$toml" "package.metadata.deb.systemd-units" | grep -q "^unit-name = \"$name\"" \
        || err "$svc: [package.metadata.deb.systemd-units] unit-name must be \"$name\""
    toml_section "$toml" "package.metadata.generate-rpm" | grep -q "^name = \"$name\"" \
        || err "$svc: [package.metadata.generate-rpm] name must be \"$name\""
done

# The QHY SDK pins (version + archive sha256s) live in two shipped places
# (ADR-013); they must match.
bp=scripts/build-packages.sh
fw=services/qhy-camera/pkg/rusty-photon-qhy-firmware-install
if [ -f "$bp" ] && [ -f "$fw" ]; then
    v1=$(sed -n 's/^QHY_SDK_VERSION="\(.*\)"$/\1/p' "$bp" | head -1)
    v2=$(sed -n 's/^VERSION="\(.*\)"$/\1/p' "$fw" | head -1)
    if [ -z "$v1" ] || [ "$v1" != "$v2" ]; then
        err "QHY SDK version pin mismatch: build-packages.sh='$v1' vs firmware-install='$v2'"
    fi
    for arch in X86_64 AARCH64; do
        s1=$(sed -n "s/^QHY_SHA256_$arch=\"\(.*\)\"\$/\1/p" "$bp" | head -1)
        s2=$(sed -n "s/^SHA256_$arch=\"\(.*\)\"\$/\1/p" "$fw" | head -1)
        if [ -z "$s1" ] || [ "$s1" != "$s2" ]; then
            err "QHY SDK sha256 pin mismatch ($arch): build-packages.sh='$s1' vs firmware-install='$s2'"
        fi
    done
fi

# The packaged ZWO SDK license must match the copy vendored with the SDK
# headers (single upstream source; the pkg/ copy exists only because cargo-deb
# assets should stay inside the crate directory).
zv=crates/zwo-rs/libzwo-sys/sdk/include/license.txt
for svc in zwo-camera zwo-focuser; do
    zl="services/$svc/pkg/ZWO-SDK-LICENSE.txt"
    cmp -s "$zv" "$zl" \
        || err "$svc: pkg/ZWO-SDK-LICENSE.txt differs from $zv"
done

# The ZWO blob ref is pinned in two places once the build script exists:
# build-packages.sh (stages pkg/lib/ for the deb) and the CI action's default
# ref (provisions the link-time SDK) — the shipped blobs and the CI-linked
# blobs must come from the same indi-3rdparty commit.
act=.github/actions/install-zwo-sdk/action.yml
if [ -f "$bp" ] && [ -f "$act" ]; then
    z1=$(sed -n 's/^ZWO_SDK_REF="\(.*\)"$/\1/p' "$bp" | head -1)
    z2=$(awk '$1 == "ref:" { in_ref = 1 } in_ref && $1 == "default:" { print $2; exit }' "$act")
    if [ -z "$z1" ] || [ "$z1" != "$z2" ]; then
        err "ZWO SDK ref pin mismatch: build-packages.sh='$z1' vs install-zwo-sdk default='$z2'"
    fi
fi

# ---- macOS tarballs (scripts/build-tarballs.sh, nightly-releases.md N4) -----
# The mac build script carries its own copies of the SDK pins (its sha256
# pins the mac arm64 archives specifically, so only the versions/refs are
# shared): the QHY SDK version must match build-packages.sh (same release
# linked on every OS) and the ZWO blob ref must match the CI action (shipped
# dylibs and CI-linked dylibs from the same indi-3rdparty commit).
bt=scripts/build-tarballs.sh
if [ -f "$bp" ] && [ -f "$bt" ]; then
    v1=$(sed -n 's/^QHY_SDK_VERSION="\(.*\)"$/\1/p' "$bp" | head -1)
    vm=$(sed -n 's/^QHY_SDK_VERSION="\(.*\)"$/\1/p' "$bt" | head -1)
    if [ -z "$vm" ] || [ "$v1" != "$vm" ]; then
        err "QHY SDK version pin mismatch: build-packages.sh='$v1' vs build-tarballs.sh='$vm'"
    fi
fi
if [ -f "$bt" ] && [ -f "$act" ]; then
    z1=$(sed -n 's/^ZWO_SDK_REF="\(.*\)"$/\1/p' "$bt" | head -1)
    z2=$(awk '$1 == "ref:" { in_ref = 1 } in_ref && $1 == "default:" { print $2; exit }' "$act")
    if [ -z "$z1" ] || [ "$z1" != "$z2" ]; then
        err "ZWO SDK ref pin mismatch: build-tarballs.sh='$z1' vs install-zwo-sdk default='$z2'"
    fi
fi

# ---- Windows suite MSI (installer/, ADR-015) --------------------------------
# The Windows package set is the Linux one (services/*/pkg) plus
# session-runner, which ships on Windows from day one while its Linux .deb
# remains an open follow-up (docs/plans/windows-packaging.md).
WIN_SERVICES="$(for d in services/*/pkg; do [ -d "$d" ] && basename "$(dirname "$d")"; done | tr '\n' ' ')session-runner"

# Documented family ports (mirrors verify-packages.sh port_of + session-runner).
win_port_of() {
    case "$1" in
        filemonitor) echo 11111 ;;
        ppba-driver) echo 11112 ;;
        qhy-focuser) echo 11113 ;;
        sentinel) echo 11114 ;;
        rp) echo 11115 ;;
        sky-survey-camera) echo 11116 ;;
        star-adventurer-gti) echo 11117 ;;
        pa-falcon-rotator) echo 11118 ;;
        dsd-fp2) echo 11119 ;;
        ui-htmx) echo 11120 ;;
        qhy-camera) echo 11121 ;;
        zwo-camera) echo 11122 ;;
        pa-scops-oag) echo 11123 ;;
        zwo-focuser) echo 11124 ;;
        phd2-guider) echo 11130 ;;
        plate-solver) echo 11131 ;;
        calibrator-flats) echo 11170 ;;
        session-runner) echo 11171 ;;
        *) echo "" ;;
    esac
}

# Feature/component-group ids are the PascalCase service dir name.
win_feature_id() {
    echo "$1" | awk -F- '{ for (i = 1; i <= NF; i++) printf "%s%s", toupper(substr($i, 1, 1)), substr($i, 2) }'
}

pkg_wxs=installer/Package.wxs
seed_ps1=installer/seed-ui-htmx-config.ps1
if [ -f "$pkg_wxs" ]; then
    for svc in $WIN_SERVICES; do
        frag="installer/fragments/$svc.wxs"
        feat=$(win_feature_id "$svc")
        port=$(win_port_of "$svc")
        [ -n "$port" ] || { err "$svc: no Windows port mapping — extend win_port_of()"; continue; }
        if [ ! -f "$frag" ]; then
            err "$svc: missing $frag"
            continue
        fi
        # The element's own Name is the first Name= attribute line after the
        # opener (the fragments put one attribute per line; Id sits on the
        # opener). A bare Name= grep would also match fw:FirewallException.
        for elem in ServiceInstall ServiceControl; do
            got=$(awk -v elem="<$elem " '
                index($0, elem) { in_elem = 1; next }
                in_elem && /Name="/ { sub(/^ */, ""); print; exit }
            ' "$frag")
            [ "$got" = "Name=\"rusty-photon-$svc\"" ] \
                || err "$svc: $elem name must be rusty-photon-$svc (got: ${got:-none})"
        done
        grep -Fq "Name=\"rusty-photon-$svc.exe\" Source=\"!(bindpath.bin)\\$svc.exe\"" "$frag" \
            || err "$svc: fragment must install bindpath.bin\\$svc.exe as rusty-photon-$svc.exe"
        grep -q 'Arguments="--service"' "$frag" \
            || err "$svc: ServiceInstall must pass --service (SCM mode)"
        grep -q 'RestartServiceDelayInSeconds="5"' "$frag" \
            || err "$svc: util:ServiceConfig must restart after 5 s (systemd RestartSec=5 parity)"
        grep -q 'FailureActionsWhen="failedToStopOrReturnedError"' "$frag" \
            || err "$svc: native ServiceConfig must set the failure-actions flag (ServiceSpecific(1) exits must count as failures)"
        grep -q "Port=\"$port\"" "$frag" \
            || err "$svc: firewall exception port must be $port"
        # Demand-start on exactly the no-defaultable-config services (the
        # ConditionPathExists= translation); everything else auto-starts on
        # install. session-runner joins the Linux-gated three: workflows_dir/
        # state_dir are required config fields with no usable defaults (it has
        # no Linux package yet, so no ConditionPathExists= unit to mirror).
        case "$svc" in
            sky-survey-camera | plate-solver | calibrator-flats | session-runner)
                grep -q 'Start="demand"' "$frag" \
                    || err "$svc: gated service must install with Start=\"demand\""
                grep -q 'Start="install"' "$frag" \
                    && err "$svc: gated service must not be started by the installer"
                ;;
            *)
                grep -q 'Start="auto"' "$frag" \
                    || err "$svc: service must install with Start=\"auto\""
                grep -q 'Start="install"' "$frag" \
                    || err "$svc: ServiceControl must start the service on install"
                ;;
        esac
        grep -q "ComponentGroupRef Id=\"${feat}Components\"" "$pkg_wxs" \
            || err "$svc: Package.wxs does not reference ${feat}Components (missing feature wiring)"
    done

    # Every fragment belongs to a packaged service (or the shared-payload
    # allowlist) — an orphan fragment would ship an unowned service.
    for frag in installer/fragments/*.wxs; do
        svc=$(basename "$frag" .wxs)
        case " $WIN_SERVICES " in
            *" $svc "*) ;;
            *)
                case "$svc" in
                    zwo-sdk-license) ;; # shared by the two zwo features
                    *) err "installer: orphan fragment $frag (no matching packaged service)" ;;
                esac
                ;;
        esac
    done

    # The ui-htmx seed table must cover exactly the Drivers-tree services with
    # the documented ports (the Automation/Core services never appear in the
    # BFF's drivers map).
    if [ -f "$seed_ps1" ]; then
        for svc in $WIN_SERVICES; do
            case "$svc" in
                sentinel | ui-htmx | rp | session-runner | plate-solver | phd2-guider | calibrator-flats)
                    grep -q "'$svc' *= *@{ port" "$seed_ps1" \
                        && err "$svc: non-driver service must not appear in the ui-htmx seed table"
                    ;;
                *)
                    grep -q "'$svc' *= *@{ port = $(win_port_of "$svc");" "$seed_ps1" \
                        || err "$svc: ui-htmx seed table missing or wrong port (expected $(win_port_of "$svc"))"
                    ;;
            esac
        done
    else
        err "installer: missing $seed_ps1"
    fi
fi

# The QHY Windows pin lives in four shipped places; they must all match the
# Linux pin (same SDK release linked and reported everywhere): build-msi.ps1,
# libqhyccd-sys build.rs (sdk_win64_<ver> search path), and qhy-camera's
# PINNED_SDK_VERSION (what the doctor reports as the build-time ABI).
bm=scripts/build-msi.ps1
if [ -f "$bp" ] && [ -f "$bm" ]; then
    v1=$(sed -n 's/^QHY_SDK_VERSION="\(.*\)"$/\1/p' "$bp" | head -1)
    vw=$(sed -n 's/^\$QhySdkVersion = "\(.*\)"$/\1/p' "$bm" | head -1)
    if [ -z "$vw" ] || [ "$v1" != "$vw" ]; then
        err "QHY SDK version pin mismatch: build-packages.sh='$v1' vs build-msi.ps1='$vw'"
    fi
    sysrs=crates/qhyccd-rs/libqhyccd-sys/build.rs
    grep -q "sdk_win64_$v1" "$sysrs" \
        || err "QHY SDK version pin mismatch: libqhyccd-sys build.rs has no sdk_win64_$v1 search path"
    pf=services/qhy-camera/src/preflight.rs
    vp=$(awk '/PINNED_SDK_VERSION: PinnedSdkVersion = / , /};/ {
            if ($1 == "year:") y = $2 + 0
            if ($1 == "month:") m = $2 + 0
            if ($1 == "day:") d = $2 + 0
        } END { if (y) printf "%02d.%02d.%02d", y, m, d }' "$pf")
    if [ -z "$vp" ] || [ "$v1" != "$vp" ]; then
        err "QHY SDK version pin mismatch: build-packages.sh='$v1' vs preflight PINNED_SDK_VERSION='$vp'"
    fi
fi

# The ZWO Windows SDK URLs are pinned in two shipped places (rolling CDN
# "latest" — no commit ref exists for Windows, so URL identity is the whole
# reproducibility statement): the CI action defaults and build-msi.ps1.
if [ -f "$bm" ] && [ -f "$act" ]; then
    for pair in "windows_camera_sdk_url ZwoCameraSdkUrl" "windows_eaf_sdk_url ZwoEafSdkUrl"; do
        input=${pair% *}
        var=${pair#* }
        u1=$(awk -v want="$input:" '$1 == want { in_input = 1 } in_input && $1 == "default:" { print $2; exit }' "$act")
        u2=$(sed -n "s/^\\\$$var = \"\(.*\)\"\$/\1/p" "$bm" | head -1)
        if [ -z "$u1" ] || [ "$u1" != "$u2" ]; then
            err "ZWO Windows SDK URL mismatch ($input): install-zwo-sdk='$u1' vs build-msi.ps1='$u2'"
        fi
    done
fi

if [ "$fail" -eq 0 ]; then
    echo "check-pkg-assets: OK"
fi
exit "$fail"
