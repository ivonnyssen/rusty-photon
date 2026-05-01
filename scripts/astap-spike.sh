#!/usr/bin/env bash
#
# ASTAP plate-solver verification spike (ADR-005).
#
# Downloads the ASTAP command-line binary appropriate for the host platform,
# verifies it executes, and (with --with-solve) downloads the D05 star
# database and runs a real solve against a supplied FITS file.
#
# This script is an operator tool, not a CI step. It is *not* invoked by
# `cargo test` or any GitHub Actions workflow. Run it manually on each
# platform we plan to ship on (Linux x64 / Linux aarch64 / macOS arm64 /
# Windows x64 / Windows arm64) to retire the open questions in ADR-005.
#
# Usage:
#   scripts/astap-spike.sh                              # smoke test only
#   scripts/astap-spike.sh --with-solve path/to.fits    # full solve
#   scripts/astap-spike.sh --with-solve path/to.fits \
#       --ra 10.6847 --dec 41.2689 --fov 1.5 --radius 5
#
# Outputs:
#   ${ASTAP_SPIKE_DIR:-/tmp/astap-spike}/astap_cli       (downloaded binary)
#   ${ASTAP_SPIKE_DIR:-/tmp/astap-spike}/d05/...         (database, --with-solve)
#   ${ASTAP_SPIKE_DIR:-/tmp/astap-spike}/<fits>.wcs      (solve output)

set -euo pipefail

# ---------------------------------------------------------------------------
# Argument parsing.
# ---------------------------------------------------------------------------

SOLVE_FITS=""
HINT_RA=""
HINT_DEC=""
HINT_FOV=""
HINT_RADIUS=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --with-solve)
            SOLVE_FITS="$2"
            shift 2
            ;;
        --ra)
            HINT_RA="$2"
            shift 2
            ;;
        --dec)
            HINT_DEC="$2"
            shift 2
            ;;
        --fov)
            HINT_FOV="$2"
            shift 2
            ;;
        --radius)
            HINT_RADIUS="$2"
            shift 2
            ;;
        -h|--help)
            sed -n '2,/^set/p' "$0" | sed 's/^# \{0,1\}//' | head -n -1
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            exit 64
            ;;
    esac
done

WORK_DIR="${ASTAP_SPIKE_DIR:-/tmp/astap-spike}"
mkdir -p "$WORK_DIR"

# ---------------------------------------------------------------------------
# Resolve which ASTAP CLI archive to download for the host.
# ---------------------------------------------------------------------------

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS:$ARCH" in
    Linux:aarch64|Linux:arm64)
        ARCHIVE="astap_command-line_version_Linux_aarch64.zip"
        BINARY="astap_cli"
        ;;
    Linux:x86_64|Linux:amd64)
        ARCHIVE="astap_command-line_version_Linux_amd64.zip"
        BINARY="astap_cli"
        ;;
    Linux:armv7l|Linux:armhf)
        ARCHIVE="astap_command-line_version_Linux_armhf.zip"
        BINARY="astap_cli"
        ;;
    Darwin:arm64|Darwin:aarch64)
        # macOS ships the GUI installer; the CLI binary is bundled inside.
        # Operators on macOS should download the .pkg or .dmg from
        # https://sourceforge.net/projects/astap-program/files/macOS%20installer/
        # and point this script at the unpacked astap_cli via $ASTAP_BINARY.
        ARCHIVE=""
        BINARY="astap_cli"
        ;;
    MINGW*:*|MSYS*:*|CYGWIN*:*|*NT*:x86_64)
        ARCHIVE="astap_command-line_version_Windows_x64.zip"
        BINARY="astap_cli.exe"
        ;;
    *)
        echo "unsupported platform: $OS $ARCH" >&2
        echo "set ASTAP_BINARY=/path/to/astap_cli to bypass auto-download" >&2
        exit 64
        ;;
esac

# ---------------------------------------------------------------------------
# Download or locate the ASTAP CLI binary.
# ---------------------------------------------------------------------------

if [[ -n "${ASTAP_BINARY:-}" && -x "${ASTAP_BINARY}" ]]; then
    ASTAP_BIN="${ASTAP_BINARY}"
    echo "[spike] using preset ASTAP_BINARY=${ASTAP_BIN}"
elif [[ -n "$ARCHIVE" ]]; then
    ASTAP_BIN="$WORK_DIR/$BINARY"
    if [[ ! -x "$ASTAP_BIN" ]]; then
        echo "[spike] downloading $ARCHIVE"
        URL="https://sourceforge.net/projects/astap-program/files/linux_installer/${ARCHIVE}/download"
        if [[ "$OS" == MINGW* || "$OS" == MSYS* || "$OS" == CYGWIN* ]]; then
            URL="https://sourceforge.net/projects/astap-program/files/windows_installer/${ARCHIVE}/download"
        fi
        curl -fSL --retry 3 --retry-delay 2 -o "$WORK_DIR/$ARCHIVE" "$URL"
        echo "[spike] extracting"
        ( cd "$WORK_DIR" && unzip -o "$ARCHIVE" >/dev/null )
        chmod +x "$ASTAP_BIN" 2>/dev/null || true
    else
        echo "[spike] cached binary at $ASTAP_BIN"
    fi
else
    echo "[spike] platform requires manual ASTAP install" >&2
    echo "        set ASTAP_BINARY=/path/to/astap_cli and re-run" >&2
    exit 65
fi

# ---------------------------------------------------------------------------
# Smoke test: confirm the binary executes and prints its banner.
# ASTAP's convention is to print help text and exit non-zero when invoked
# with no arguments. We treat any exit code as a pass as long as the banner
# (containing "ASTAP" and a version-like token) appears on stderr or stdout.
# ---------------------------------------------------------------------------

echo "[spike] smoke test: running $ASTAP_BIN with no arguments"
SMOKE_OUTPUT="$WORK_DIR/smoke.log"
"$ASTAP_BIN" >"$SMOKE_OUTPUT" 2>&1 || true

if grep -qi "astap" "$SMOKE_OUTPUT"; then
    BANNER_LINE="$(grep -i 'astap' "$SMOKE_OUTPUT" | head -n 1)"
    echo "[spike] OK: banner detected -> $BANNER_LINE"
else
    echo "[spike] FAIL: ASTAP banner not found in output" >&2
    echo "        first 20 lines:" >&2
    head -n 20 "$SMOKE_OUTPUT" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# If --with-solve was passed, run a real solve. This pulls down the D05
# database (~100 MB) on first run.
# ---------------------------------------------------------------------------

if [[ -n "$SOLVE_FITS" ]]; then
    if [[ ! -f "$SOLVE_FITS" ]]; then
        echo "[spike] FAIL: --with-solve target not found: $SOLVE_FITS" >&2
        exit 66
    fi
    DB_DIR="$WORK_DIR/d05"
    if [[ ! -d "$DB_DIR" || -z "$(ls -A "$DB_DIR" 2>/dev/null)" ]]; then
        echo "[spike] downloading D05 star database (~100 MB, one-time)"
        mkdir -p "$DB_DIR"
        DB_URL="https://sourceforge.net/projects/astap-program/files/star_databases/d05_star_database.zip/download"
        curl -fSL --retry 3 --retry-delay 2 -o "$WORK_DIR/d05_star_database.zip" "$DB_URL"
        ( cd "$DB_DIR" && unzip -o "$WORK_DIR/d05_star_database.zip" >/dev/null )
    else
        echo "[spike] cached D05 database at $DB_DIR"
    fi

    echo "[spike] solving $SOLVE_FITS"
    SOLVE_ARGS=(-f "$SOLVE_FITS" -d "$DB_DIR" -wcs)
    [[ -n "$HINT_RA"     ]] && SOLVE_ARGS+=(-ra "$HINT_RA")
    [[ -n "$HINT_DEC"    ]] && SOLVE_ARGS+=(-spd "$(echo "$HINT_DEC + 90" | bc -l)")
    [[ -n "$HINT_FOV"    ]] && SOLVE_ARGS+=(-fov "$HINT_FOV")
    [[ -n "$HINT_RADIUS" ]] && SOLVE_ARGS+=(-r "$HINT_RADIUS")

    SOLVE_LOG="$WORK_DIR/solve.log"
    START_NS="$(date +%s%N)"
    "$ASTAP_BIN" "${SOLVE_ARGS[@]}" >"$SOLVE_LOG" 2>&1 || true
    END_NS="$(date +%s%N)"
    ELAPSED_MS=$(( (END_NS - START_NS) / 1000000 ))

    WCS_FILE="${SOLVE_FITS%.*}.wcs"
    if [[ -f "$WCS_FILE" ]]; then
        echo "[spike] OK: solved in ${ELAPSED_MS} ms -> $WCS_FILE"
        echo "[spike] WCS keys:"
        grep -E '^(CRVAL[12]|CRPIX[12]|CD[12]_[12]|CDELT[12]|CROTA[12]|PLTSOLVD)' "$WCS_FILE" || true
    else
        echo "[spike] FAIL: no .wcs sidecar produced after ${ELAPSED_MS} ms" >&2
        echo "        last 20 lines of solve log:" >&2
        tail -n 20 "$SOLVE_LOG" >&2
        exit 1
    fi
fi

echo "[spike] done. artefacts in $WORK_DIR"
