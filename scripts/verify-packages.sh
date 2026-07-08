#!/bin/sh
# verify-packages.sh — lifecycle-verify the built .deb packages in a podman
# systemd container (debian:trixie). Per service: install → unit active →
# config self-created at /var/lib/rusty-photon/.config/rusty-photon/<svc>.json
# → HTTP probe → remove (config survives) → purge (config + state gone; the
# shared user, home dir, and /etc/rusty-photon symlink stay). Class
# exceptions: ConditionPathExists-gated services (sky-survey-camera,
# plate-solver, calibrator-flats) verify enabled-but-inactive-and-not-failed;
# serial drivers verify config + handshake-attempted instead of active (see
# is_serial); the cameras never self-create a config (see
# self_creates_config). Runs natively on arm64 (the rig) and x86_64 (dev box).
#
# Rootless-container caveat (docs/plans/service-packaging.md): rootless
# podman cannot apply the units' sandboxing across the User= switch
# (217/USER, 226/NAMESPACE), so this script pre-installs a drop-in that
# resets the whole hardening block inside the container. Containers verify
# the packaging lifecycle; the hardening is verified on real hosts
# (systemd-analyze security + an active unit on the rig).
#
# Usage: scripts/verify-packages.sh [--services a,b,c] [--dist DIR] [--keep]
#   --services a,b,c  verify only these (default: every rusty-photon-*.deb in DIST)
#   --dist DIR        package directory (default: dist/<workspace version>)
#   --keep            keep the container on exit (debugging)

set -eu

die() { echo "verify-packages: $*" >&2; exit 1; }

usage() {
    sed -n '/^# Usage:/,/^$/{s/^# \{0,1\}//p}' "$0"
}

KEEP=0
DIST=""
ONLY_SERVICES=""
while [ $# -gt 0 ]; do
    case "$1" in
        --services)
            shift
            [ $# -gt 0 ] || die "--services needs a comma-separated list"
            ONLY_SERVICES="$1"
            ;;
        --dist)
            shift
            [ $# -gt 0 ] || die "--dist needs a directory"
            DIST="$1"
            ;;
        --keep) KEEP=1 ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; die "unknown option: $1" ;;
    esac
    shift
done

[ -f packaging/postinst.common ] || die "run from the repo root"
command -v podman > /dev/null 2>&1 || die "podman not found"

if [ -z "$DIST" ]; then
    version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
    DIST="dist/$version"
fi
[ -d "$DIST" ] || die "$DIST not found — run scripts/build-packages.sh first"
DIST_ABS=$(cd "$DIST" && pwd)

# Service list: from the debs actually present, or --services.
if [ -n "$ONLY_SERVICES" ]; then
    SERVICES=$(echo "$ONLY_SERVICES" | tr ',' ' ')
else
    SERVICES=$(for f in "$DIST_ABS"/rusty-photon-*.deb; do
        [ -e "$f" ] || continue
        b=$(basename "$f")
        b=${b#rusty-photon-}
        echo "${b%%_*}"
    done | sort -u | tr '\n' ' ')
fi
[ -n "$SERVICES" ] || die "no rusty-photon-*.deb packages in $DIST_ABS"
for s in $SERVICES; do
    # Exactly one deb per service: a multi-match glob (e.g. two revisions
    # left over from an upgrade test) would make the in-container
    # dpkg-deb/apt-get invocations below misbehave in confusing ways.
    set -- "$DIST_ABS/rusty-photon-${s}_"*.deb
    [ -e "$1" ] || die "no rusty-photon-${s}_*.deb in $DIST_ABS"
    [ $# -eq 1 ] || die "multiple rusty-photon-${s}_*.deb in $DIST_ABS — remove stale builds first"
done
echo "Verifying: $SERVICES"

port_of() {
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
        plate-solver) echo 11131 ;;
        calibrator-flats) echo 11170 ;;
        *) echo "" ;;
    esac
}

probe_path() {
    # Alpaca services answer the management API; the plain-HTTP services
    # (sentinel dashboard, rp orchestrator, ui-htmx BFF) expose /health.
    case "$1" in
        sentinel|rp|ui-htmx) echo /health ;;
        *) echo /management/apiversions ;;
    esac
}

is_gated() {
    # No defaultable config → unit gated on ConditionPathExists (see plan).
    case "$1" in
        sky-survey-camera|plate-solver|calibrator-flats) return 0 ;;
        *) return 1 ;;
    esac
}

is_serial() {
    # Serial-device drivers validate hardware eagerly at startup and exit
    # when their device is absent (deliberate: fail startup instead of
    # advertising a broken device on the network); systemd then retries
    # every 5s until the device appears. The container has no serial
    # devices, so "active" is not the contract to verify for these.
    case "$1" in
        ppba-driver|qhy-focuser|pa-falcon-rotator|pa-scops-oag|dsd-fp2|star-adventurer-gti) return 0 ;;
        *) return 1 ;;
    esac
}

is_cli() {
    case "$1" in
        phd2-guider) return 0 ;;
        *) return 1 ;;
    esac
}

self_creates_config() {
    # The cameras deliberately do NOT materialize a config on start (no
    # materialize_identity — ASCOM UniqueIDs derive from camera serials;
    # they run on defaults until config.apply / an operator writes a file).
    # Gated services never start without one.
    if is_gated "$1"; then return 1; fi
    case "$1" in
        qhy-camera|zwo-camera) return 1 ;;
        *) return 0 ;;
    esac
}

# ---- container ----------------------------------------------------------
IMG=localhost/rusty-photon-pkg-verify
CNAME="rusty-photon-verify-$$"

echo "Building the verification image ($IMG)..."
podman build -q -t "$IMG" - <<'EOF' > /dev/null
FROM debian:trixie
# The stock Debian image ships a policy-rc.d that exits 101, silently
# blocking every unit start from maintainer scripts — exactly what this
# script exists to verify. Remove it; we run a real systemd via /sbin/init.
RUN apt-get update && apt-get install -y --no-install-recommends \
    systemd systemd-sysv udev ca-certificates curl \
    && rm -f /usr/sbin/policy-rc.d
CMD ["/sbin/init"]
EOF

cleanup() {
    if [ "$KEEP" = 1 ]; then
        echo "Container kept: podman exec -it $CNAME bash (remove: podman rm -f $CNAME)"
    else
        podman rm -f "$CNAME" > /dev/null 2>&1 || true
    fi
}
trap cleanup EXIT INT TERM

podman run -d --name "$CNAME" --systemd=always \
    -v "$DIST_ABS:/dist:ro,Z" "$IMG" > /dev/null

cx() { podman exec "$CNAME" "$@"; }

fail() {
    svc="$1"
    shift
    echo "verify-packages: FAIL [$svc]: $*" >&2
    cx journalctl -u "rusty-photon-$svc" --no-pager -n 40 >&2 || true
    exit 1
}

echo "Waiting for systemd in the container..."
i=0
while :; do
    state=$(cx systemctl is-system-running 2> /dev/null || true)
    case "$state" in running | degraded) break ;; esac
    i=$((i + 1))
    [ "$i" -lt 60 ] || die "systemd did not come up in the container (state: $state)"
    sleep 1
done

# Hardening-reset drop-ins must exist BEFORE the debs install (postinst
# starts the units immediately).
for s in $SERVICES; do
    is_cli "$s" && continue
    cx mkdir -p "/etc/systemd/system/rusty-photon-$s.service.d"
    podman exec -i "$CNAME" sh -c \
        "cat > /etc/systemd/system/rusty-photon-$s.service.d/99-container-verify.conf" <<'EOF'
# Rootless podman cannot set up the sandbox across the User= switch.
# Containers verify packaging lifecycle only; hardening is verified on hosts.
[Service]
NoNewPrivileges=no
ProtectSystem=off
ReadWritePaths=
ProtectHome=no
PrivateTmp=no
ProtectKernelTunables=no
ProtectKernelModules=no
ProtectControlGroups=no
RestrictSUIDSGID=no
LockPersonality=no
RestrictRealtime=no
RestrictAddressFamilies=
PrivateDevices=no
MemoryDenyWriteExecute=no
UMask=0022
EOF
done

echo "Refreshing apt package lists in the container..."
cx sh -c "apt-get update -qq"

# Debs built on a non-Debian host carry an empty `$auto` Depends
# (dpkg-shlibdeps needs Debian's shlibs database; cargo-deb warns and moves
# on). Preinstall the known runtime libs so lifecycle verification still
# works there; on Debian-host (rig) builds Depends is populated and apt
# resolves it strictly, so this branch stays cold.
compensate=0
for s in $SERVICES; do
    dep=$(cx sh -c "dpkg-deb -f /dist/rusty-photon-${s}_*.deb Depends" 2> /dev/null || true)
    [ -n "$dep" ] || compensate=1
done
if [ "$compensate" = 1 ]; then
    echo "verify-packages: WARNING: deb(s) with empty Depends (non-Debian-host build); preinstalling runtime libs"
    cx sh -c "DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends libusb-1.0-0 libudev1 libstdc++6" > /dev/null
fi

# ---- install + per-service checks ----------------------------------------
for s in $SERVICES; do
    echo "== $s: install"
    cx sh -c "DEBIAN_FRONTEND=noninteractive apt-get install -y /dist/rusty-photon-${s}_*.deb" \
        > /dev/null || fail "$s" "apt-get install failed"

    if is_cli "$s"; then
        cx test -x "/usr/bin/rusty-photon-$s" || fail "$s" "binary missing"
        cx "/usr/bin/rusty-photon-$s" --help > /dev/null || fail "$s" "--help failed"
        echo "== $s: OK (CLI)"
        continue
    fi

    # Shared layout, created by the first daemon postinst and stable after.
    cx test -L /etc/rusty-photon || fail "$s" "/etc/rusty-photon symlink missing"
    cx sh -c "getent passwd rusty-photon > /dev/null" || fail "$s" "rusty-photon user missing"

    cfg="/var/lib/rusty-photon/.config/rusty-photon/$s.json"
    if is_gated "$s"; then
        # No config → unit must be enabled but neither active nor failed.
        cx systemctl is-enabled --quiet "rusty-photon-$s" || fail "$s" "unit not enabled"
        if cx systemctl is-active --quiet "rusty-photon-$s"; then
            fail "$s" "gated unit is active without a config"
        fi
        if cx systemctl is-failed --quiet "rusty-photon-$s"; then
            fail "$s" "gated unit failed instead of waiting on ConditionPathExists"
        fi
        cx test ! -e "$cfg" || fail "$s" "config exists on a fresh install of a gated service"
        echo "== $s: OK (gated on config)"
        continue
    fi

    if is_serial "$s"; then
        # Verifiable without hardware: the binary starts, self-creates its
        # config, and fails on the absent device — not earlier (loader,
        # user, or config problems would die before the handshake).
        i=0
        until cx sh -c "journalctl -u rusty-photon-$s --no-pager 2>/dev/null | grep -q 'eager startup handshake'"; do
            i=$((i + 1))
            [ "$i" -lt 30 ] || fail "$s" "no eager-handshake attempt in the journal"
            sleep 1
        done
        i=0
        until cx sh -c "test -s $cfg"; do
            i=$((i + 1))
            [ "$i" -lt 15 ] || fail "$s" "config not self-created at $cfg"
            sleep 1
        done
        if cx systemctl is-active --quiet "rusty-photon-$s"; then
            # In case a serial driver ever gains warn-and-serve, hold it to
            # the full contract.
            port=$(port_of "$s")
            [ -n "$port" ] || fail "$s" "no port mapping — add $s to port_of()"
            path=$(probe_path "$s")
            cx curl -fsS -o /dev/null "http://127.0.0.1:$port$path" 2> /dev/null \
                || fail "$s" "active but no HTTP response on port $port ($path)"
            echo "== $s: OK (active, config, port $port)"
        else
            echo "== $s: OK (config self-created; retrying on absent serial device)"
        fi
        continue
    fi

    i=0
    until cx systemctl is-active --quiet "rusty-photon-$s"; do
        i=$((i + 1))
        [ "$i" -lt 30 ] || fail "$s" "unit not active after 30s"
        sleep 1
    done

    if self_creates_config "$s"; then
        i=0
        until cx sh -c "test -s $cfg"; do
            i=$((i + 1))
            [ "$i" -lt 15 ] || fail "$s" "config not self-created at $cfg"
            sleep 1
        done
    fi

    port=$(port_of "$s")
    [ -n "$port" ] || fail "$s" "no port mapping — add $s to port_of()"
    path=$(probe_path "$s")
    i=0
    until cx curl -fsS -o /dev/null "http://127.0.0.1:$port$path" 2> /dev/null; do
        i=$((i + 1))
        [ "$i" -lt 30 ] || fail "$s" "no HTTP response on port $port ($path)"
        sleep 1
    done

    if [ "$s" = zwo-camera ]; then
        # RUNPATH proof: the SONAME-less bundled blobs resolve from the
        # package's private lib dir, not from a global path.
        cx sh -c "ldd /usr/bin/rusty-photon-zwo-camera | grep -q /usr/lib/rusty-photon/libASICamera2.so" \
            || fail "$s" "libASICamera2.so does not resolve to /usr/lib/rusty-photon (RUNPATH)"
    fi
    echo "== $s: OK (active, config, port $port)"
done

# ---- remove / purge lifecycle ---------------------------------------------
for s in $SERVICES; do
    echo "== $s: remove + purge"
    cx sh -c "DEBIAN_FRONTEND=noninteractive apt-get remove -y rusty-photon-$s" > /dev/null \
        || fail "$s" "apt-get remove failed"
    if is_cli "$s"; then
        continue
    fi
    cfg="/var/lib/rusty-photon/.config/rusty-photon/$s.json"
    if self_creates_config "$s"; then
        cx test -f "$cfg" || fail "$s" "config did not survive remove (must only go on purge)"
    fi
    cx dpkg --purge "rusty-photon-$s" > /dev/null || fail "$s" "dpkg --purge failed"
    cx test ! -e "$cfg" || fail "$s" "config survived purge"
    cx test ! -e "/var/lib/rusty-photon/$s" || fail "$s" "state dir survived purge"
done

# The shared pieces are never removed (Debian convention; shared across pkgs).
cx sh -c "getent passwd rusty-photon > /dev/null" || die "shared user removed by purge"
cx test -d /var/lib/rusty-photon || die "shared home removed by purge"
cx test -L /etc/rusty-photon || die "/etc/rusty-photon symlink removed by purge"

echo ""
echo "verify-packages: OK ($(echo "$SERVICES" | wc -w | tr -d ' ') packages)"
