#!/bin/sh
# build-packages.sh — build the rusty-photon .deb (and optionally .rpm)
# packages natively on the target machine: the Debian arm64 rig or an
# x86_64 box. Operator guide: docs/packaging.md; design:
# docs/plans/service-packaging.md + ADR-012/ADR-013.
#
# Steps:
#   1. install build prerequisites (apt when available; cargo-deb always,
#      cargo-generate-rpm only with --rpm),
#   2. stage the native camera SDKs into ~/.cache/rusty-photon-pkg/:
#      QHYCCD's static libqhyccd.a for the link (exported as
#      QHYCCD_SDK_DIR), and the ZWO MIT blobs into the gitignored
#      services/zwo-camera/pkg/lib/ — used both for the link
#      (ZWO_SDK_LIB_DIR) and as package payload (ADR-013),
#   3. one release build of every selected service with the RUNPATH the
#      zwo-camera package needs (-Wl,-rpath,/usr/lib/rusty-photon;
#      uniform across binaries, harmless where unused), then strip,
#   4. per service: cargo deb --no-build --no-strip (a rebuild would lose
#      the staged env/RUSTFLAGS); with --rpm also cargo generate-rpm,
#   5. collect artifacts into dist/<version>/ + SHA256SUMS.txt.
#
# Usage: scripts/build-packages.sh [--services a,b,c] [--rpm] [--skip-sdk-staging]
#   --services a,b,c    build only these services (default: every packaged one)
#   --rpm               also build .rpm packages (x86_64 dev-box convenience)
#   --skip-sdk-staging  offline rebuild: no downloads; requires the SDK
#                       cache from a previous run

set -eu

die() { echo "build-packages: $*" >&2; exit 1; }

# ---- pins -------------------------------------------------------------
# The QHY pins must match services/qhy-camera/pkg/rusty-photon-qhy-firmware-install
# (scripts/check-pkg-assets.sh enforces it): the SDK linked at build time and
# the firmware the helper installs on the target must be the same release.
QHY_SDK_VERSION="26.06.04"
QHY_SHA256_X86_64="cbfcec159809e6984c5013a587fed88c892afae9d834019e820213f64616a308"
QHY_SHA256_AARCH64="d28795977311fba1cb7a4fdc48bbd6d5f994716674b2154d9a860d1fdf2f5e0e"
# QHY's download layout for >= 26.06.04: the directory is the dotless version.
QHY_URL_BASE="https://www.qhyccd.com/file/repository/publish/SDK/$(echo "$QHY_SDK_VERSION" | tr -d .)"

# Must match the `ref` default in .github/actions/install-zwo-sdk/action.yml
# (checker-enforced): the blobs shipped in the deb and the blobs CI links
# against must come from the same indi-3rdparty commit. The immutable commit
# SHA in the download URL is the integrity statement, same as in the action.
ZWO_SDK_REF="b0802f28055b67aa6a99580d260c3bb4c27eba4b"

usage() {
    sed -n '/^# Usage:/,/^$/{s/^# \{0,1\}//p}' "$0"
}

RPM=0
SKIP_STAGING=0
ONLY_SERVICES=""
while [ $# -gt 0 ]; do
    case "$1" in
        --services)
            shift
            [ $# -gt 0 ] || die "--services needs a comma-separated list"
            ONLY_SERVICES="$1"
            ;;
        --rpm) RPM=1 ;;
        --skip-sdk-staging) SKIP_STAGING=1 ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; die "unknown option: $1" ;;
    esac
    shift
done

[ "$(uname -s)" = Linux ] || die "deb/rpm packages are Linux-only (see release.yml for the other targets)"
[ -f packaging/postinst.common ] || die "run from the repo root"

# ---- service selection --------------------------------------------------
# Packaged services are exactly the crates with a pkg/ dir (same discovery
# as check-pkg-assets.sh). All of them are daemons — phd2-guider gained its
# unit with the HTTP service mode (issue #464).
ALL_SERVICES=$(for d in services/*/pkg; do [ -d "$d" ] && basename "$(dirname "$d")"; done | tr '\n' ' ')

SERVICES=""
if [ -n "$ONLY_SERVICES" ]; then
    for s in $(echo "$ONLY_SERVICES" | tr ',' ' '); do
        case " $ALL_SERVICES " in
            *" $s "*) SERVICES="$SERVICES $s" ;;
            *) die "unknown or unpackaged service: $s (packaged: $ALL_SERVICES)" ;;
        esac
    done
else
    SERVICES="$ALL_SERVICES"
fi

needs_qhy=0
needs_zwo=0
case " $SERVICES " in *" qhy-camera "*) needs_qhy=1 ;; esac
case " $SERVICES " in *" zwo-camera "*) needs_zwo=1 ;; esac

case "$(uname -m)" in
    x86_64)
        QHY_FILE="sdk_linux64_${QHY_SDK_VERSION}.tar.gz"
        QHY_SHA256="$QHY_SHA256_X86_64"
        ZWO_ARCH=x64
        ;;
    aarch64)
        QHY_FILE="sdk_linux_arm64_${QHY_SDK_VERSION}.tar.gz"
        QHY_SHA256="$QHY_SHA256_AARCH64"
        ZWO_ARCH=armv8
        ;;
    *) die "unsupported architecture $(uname -m) (need x86_64 or aarch64)" ;;
esac

# ---- prerequisites ------------------------------------------------------
SUDO=""
[ "$(id -u)" = 0 ] || SUDO="sudo"

# libusb-1.0-0-dev + libudev-dev: the camera SDK links (-lusb-1.0, -ludev);
# clang/libclang-dev: bindgen in libzwo-sys; dpkg-dev: dpkg-shlibdeps for
# cargo-deb's $auto dependency resolution.
APT_PKGS="build-essential pkg-config curl git ca-certificates dpkg-dev libusb-1.0-0-dev libudev-dev clang libclang-dev"
if command -v apt-get > /dev/null 2>&1; then
    missing=""
    for p in $APT_PKGS; do
        dpkg -s "$p" > /dev/null 2>&1 || missing="$missing $p"
    done
    if [ -n "$missing" ]; then
        echo "Installing build prerequisites:$missing"
        $SUDO apt-get update -qq
        # shellcheck disable=SC2086 # word-splitting the package list is intended
        $SUDO apt-get install -y --no-install-recommends $missing
    fi
else
    echo "build-packages: apt-get not found; make sure equivalents of these are installed: $APT_PKGS" >&2
fi

command -v cargo > /dev/null 2>&1 || die "cargo not found (install Rust via rustup)"
command -v cargo-deb > /dev/null 2>&1 || cargo install --locked cargo-deb
if [ "$RPM" = 1 ]; then
    command -v cargo-generate-rpm > /dev/null 2>&1 || cargo install --locked cargo-generate-rpm
fi

# ---- SDK staging --------------------------------------------------------
CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/rusty-photon-pkg"
mkdir -p "$CACHE"

# fetch URL DEST — atomic download (no half-written file poisoning the cache).
fetch() {
    # Explicit check: on non-apt hosts nothing above installed curl, and a
    # bare `curl: not found` under set -e would be low-signal.
    command -v curl > /dev/null 2>&1 || die "curl not found (needed to download the SDKs)"
    echo "Downloading $1"
    curl -fsSL -o "$2.part" "$1"
    mv "$2.part" "$2"
}

if [ "$needs_qhy" = 1 ]; then
    QHY_EXTRACT="$CACHE/qhy-sdk-$QHY_SDK_VERSION-$(uname -m)"
    if [ ! -d "$QHY_EXTRACT" ]; then
        [ "$SKIP_STAGING" = 0 ] || die "--skip-sdk-staging set but $QHY_EXTRACT is missing"
        [ -f "$CACHE/$QHY_FILE" ] || fetch "$QHY_URL_BASE/$QHY_FILE" "$CACHE/$QHY_FILE"
        echo "$QHY_SHA256  $CACHE/$QHY_FILE" | sha256sum -c --status - \
            || die "sha256 mismatch for $QHY_FILE (expected $QHY_SHA256)"
        tmp="$CACHE/extract.$$"
        mkdir -p "$tmp"
        tar -xzf "$CACHE/$QHY_FILE" -C "$tmp"
        mv "$tmp/${QHY_FILE%.tar.gz}" "$QHY_EXTRACT"
        # rm -rf, not rmdir: any extra top-level entry in a future archive
        # layout would make rmdir abort the build under set -e.
        rm -rf "$tmp"
    fi
    # Locate the static lib rather than hardcoding the archive layout.
    qhy_lib=$(find "$QHY_EXTRACT" -name libqhyccd.a | head -1)
    [ -n "$qhy_lib" ] || die "libqhyccd.a not found under $QHY_EXTRACT"
    QHYCCD_SDK_DIR=$(dirname "$qhy_lib")
    export QHYCCD_SDK_DIR
    echo "QHYCCD SDK $QHY_SDK_VERSION staged: QHYCCD_SDK_DIR=$QHYCCD_SDK_DIR"
fi

if [ "$needs_zwo" = 1 ]; then
    ZWO_CACHE="$CACHE/zwo-$ZWO_SDK_REF-$ZWO_ARCH"
    mkdir -p "$ZWO_CACHE"
    ZWO_BASE="https://github.com/indilib/indi-3rdparty/raw/$ZWO_SDK_REF/libasi"
    for blob in libASICamera2 libEFWFilter; do
        if [ ! -f "$ZWO_CACHE/$blob.so" ]; then
            [ "$SKIP_STAGING" = 0 ] || die "--skip-sdk-staging set but $ZWO_CACHE/$blob.so is missing"
            fetch "$ZWO_BASE/$ZWO_ARCH/$blob.bin" "$ZWO_CACHE/$blob.so"
        fi
    done
    # Both the link search dir and the cargo-deb/generate-rpm asset source.
    mkdir -p services/zwo-camera/pkg/lib
    cp "$ZWO_CACHE/libASICamera2.so" "$ZWO_CACHE/libEFWFilter.so" services/zwo-camera/pkg/lib/
    ZWO_SDK_LIB_DIR="$(pwd)/services/zwo-camera/pkg/lib"
    export ZWO_SDK_LIB_DIR
    echo "ZWO SDK blobs (indi-3rdparty $ZWO_SDK_REF) staged: ZWO_SDK_LIB_DIR=$ZWO_SDK_LIB_DIR"
fi

# ---- build ----------------------------------------------------------------
# The RUNPATH lets the SONAME-less bundled ZWO blobs resolve from
# /usr/lib/rusty-photon at runtime. Deliberately set here, not in a build.rs
# (which would ripple into Bazel/repin). Overrides any ambient RUSTFLAGS so
# the produced binaries do not depend on the invoking shell's environment.
RUSTFLAGS="-C link-arg=-Wl,-rpath,/usr/lib/rusty-photon"
export RUSTFLAGS

build_args=""
for s in $SERVICES; do
    build_args="$build_args -p $s"
done
echo "Building release binaries:$build_args"
# shellcheck disable=SC2086 # word-splitting the -p list is intended
cargo build --release $build_args

# Same reasoning as the curl check in fetch(): make a missing binutils on a
# non-apt host fail with an actionable message, not a bare `strip: not found`.
command -v strip > /dev/null 2>&1 || die "strip not found (install binutils)"
for s in $SERVICES; do
    strip "target/release/$s"
done

# ---- package -----------------------------------------------------------
VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
[ -n "$VERSION" ] || die "could not read the workspace version from Cargo.toml"
DIST="dist/$VERSION"
mkdir -p "$DIST"

for s in $SERVICES; do
    echo "Packaging $s"
    # --no-build: reusing the staged-env build above is essential; a rebuild
    # here would drop QHYCCD_SDK_DIR/ZWO_SDK_LIB_DIR/RUSTFLAGS.
    cargo deb -p "$s" --no-build --no-strip --output "$DIST/"
    if [ "$RPM" = 1 ]; then
        cargo generate-rpm -p "services/$s" -o "$DIST/"
    fi
done

(
    cd "$DIST"
    rm -f SHA256SUMS.txt
    # shellcheck disable=SC2035
    sha256sum *.deb *.rpm > SHA256SUMS.txt 2>/dev/null || sha256sum *.deb > SHA256SUMS.txt
)

echo ""
echo "Packages in $DIST/:"
ls -1 "$DIST"
