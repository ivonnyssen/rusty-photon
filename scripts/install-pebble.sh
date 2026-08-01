#!/bin/sh
# install-pebble.sh — install Pebble (Let's Encrypt's ACME test server) and
# its pebble-challtestsrv DNS sidecar, so doctor's @pebble BDD scenarios run
# locally instead of being skipped (docs/skills/testing.md §5.6).
#
# The version and per-platform SHA-256 table below mirror
# .github/actions/install-pebble — CI and dev boxes must run the same
# binaries, so bumping one means bumping the other. Verification fails
# closed: a mismatch aborts before anything is installed.
#
# Usage:
#   scripts/install-pebble.sh [--prefix DIR]   # default: ~/.local/opt/pebble
#
# On success it prints the two exports the suite reads (.bazelrc forwards
# both into test actions by name); add them to your shell profile to make
# them stick.

set -eu

VERSION="v2.10.1"
PREFIX="${HOME}/.local/opt/pebble"

while [ $# -gt 0 ]; do
    case "$1" in
        --prefix)
            [ $# -ge 2 ] || { echo "install-pebble: --prefix needs a directory" >&2; exit 2; }
            PREFIX="$2"
            shift 2
            ;;
        -h | --help)
            echo "usage: install-pebble.sh [--prefix DIR]   # default: ~/.local/opt/pebble"
            exit 0
            ;;
        *)
            echo "install-pebble: unknown argument '$1'" >&2
            exit 2
            ;;
    esac
done

# Pebble publishes no darwin-x64 build past v2.8, and the CI table carries
# the same five platforms; anything else has to build from source.
case "$(uname -s)-$(uname -m)" in
    Linux-x86_64)
        PLATFORM="linux-amd64"
        PEBBLE_SHA="4f2fcb5bca8c85c9cf73ad140fccfc0d2be40bd81ab99879c79b7b8a0b4f70ed"
        CHALL_SHA="e93a5aa25ecdf3af2f9fbb2de32b0173e64a2eae81002a4ccfe35fa6f4f60b92"
        ;;
    Linux-aarch64 | Linux-arm64)
        PLATFORM="linux-arm64"
        PEBBLE_SHA="b53fd072a69eb7692451de4e8b0667e0bdf5cccd7e36fc51b8eaf2fcc135ed9f"
        CHALL_SHA="db8e1a79ccdb2195c489fbe4f40fddb7f30e86f9cd8a07912566ee5025094d6c"
        ;;
    Darwin-arm64)
        PLATFORM="darwin-arm64"
        PEBBLE_SHA="09a3a4e6ebed71e8d83294a26d361232262f45a7488f5de7bccb5887b395217f"
        CHALL_SHA="59bf917fe39c96e2edca980fc2899f4f04aa1ce5485f28d419d18237b902cf82"
        ;;
    Darwin-x86_64)
        PLATFORM="darwin-amd64"
        PEBBLE_SHA="e670ff869886022637e077502a62e7f23be693c45a5a6727ebd76da8fdce64dc"
        CHALL_SHA="796bd923f2c595dd7bf15ae693096abfb1df962cb3673c7981ff306daa5c4a52"
        ;;
    *)
        echo "install-pebble: unsupported platform $(uname -s)-$(uname -m)" >&2
        echo "  supported: Linux x86_64/arm64, macOS arm64/x86_64." >&2
        echo "  On Windows use .github/actions/install-pebble's table directly." >&2
        exit 1
        ;;
esac

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

for PAIR in "pebble ${PEBBLE_SHA}" "pebble-challtestsrv ${CHALL_SHA}"; do
    NAME="${PAIR%% *}"
    EXPECTED="${PAIR##* }"
    ASSET="${NAME}-${PLATFORM}.tar.gz"
    URL="https://github.com/letsencrypt/pebble/releases/download/${VERSION}/${ASSET}"

    echo "Downloading ${URL}"
    curl -fsSL -o "${TMP}/${ASSET}" "${URL}"

    ACTUAL="$(sha256_of "${TMP}/${ASSET}")"
    if [ "${ACTUAL}" != "${EXPECTED}" ]; then
        echo "install-pebble: SHA-256 mismatch for ${ASSET}" >&2
        echo "  expected: ${EXPECTED}" >&2
        echo "  actual:   ${ACTUAL}" >&2
        echo "Upstream may have rotated the asset, or VERSION was bumped without" >&2
        echo "refreshing the table here and in .github/actions/install-pebble." >&2
        exit 1
    fi
    echo "  sha256 ok: ${ACTUAL}"

    tar xzf "${TMP}/${ASSET}" -C "${TMP}"
    # Tarballs unpack to <name>-<platform>/<os>/<arch>/<name> — flatten.
    BIN="$(find "${TMP}/${NAME}-${PLATFORM}" -type f -name "${NAME}*" | head -1)"
    [ -n "${BIN}" ] || { echo "install-pebble: no ${NAME} binary in ${ASSET}" >&2; exit 1; }
    # Staged first, moved into place only once both downloads have verified,
    # so a failure halfway can't leave a half-installed prefix.
    mv "${BIN}" "${TMP}/staged-${NAME}"
done

mkdir -p "${PREFIX}"
for NAME in pebble pebble-challtestsrv; do
    mv "${TMP}/staged-${NAME}" "${PREFIX}/${NAME}"
    chmod +x "${PREFIX}/${NAME}"
    # Gatekeeper quarantines downloaded binaries on macOS; without this the
    # first spawn dies with a signal instead of running.
    if [ "$(uname -s)" = "Darwin" ]; then
        xattr -d com.apple.quarantine "${PREFIX}/${NAME}" 2>/dev/null || true
    fi
done

echo
echo "Pebble ${VERSION} installed in ${PREFIX}"
echo "Add these to your shell profile (~/.bashrc, ~/.zshrc):"
echo
echo "  export PEBBLE_PATH=\"${PREFIX}/pebble\""
echo "  export PEBBLE_CHALLTESTSRV_PATH=\"${PREFIX}/pebble-challtestsrv\""
echo
echo "Then 'bazel test //services/doctor:bdd' runs the @pebble scenarios too."
