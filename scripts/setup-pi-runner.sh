#!/usr/bin/env bash
#
# Provision a Raspberry Pi 5 (Linux/ARM64) as the rusty-photon `pi-nightly`
# self-hosted runner. Idempotent: safe to re-run after a partial setup or
# when bumping RUNNER_VERSION.
#
# Operator-facing runbook (security model, decommissioning, troubleshooting):
#   docs/skills/raspberry-pi-runner.md
#
# Usage:
#   RUNNER_TOKEN=<token-from-github> ./scripts/setup-pi-runner.sh
#
#   Optional env vars:
#     RUNNER_VERSION   Runner release tag (default below, pinned at script edit time)
#     RUNNER_SHA256    Expected SHA-256 of the runner tarball. If unset, the
#                      script downloads without integrity verification; if set,
#                      mismatch aborts the install.
#     RUNNER_NAME      Display name in the GitHub UI (default: pi5-nightly)
#     RUNNER_LABEL     Custom label matched by .github/workflows/pi-nightly.yml
#                      (default: raspberry-pi — must match the workflow's
#                      runs-on list or jobs will never schedule)
#     RUNNER_USER      System user that owns the runner (default: gh-runner)
#     REPO_URL         Repository URL (default: ivonnyssen/rusty-photon)
#
# To obtain RUNNER_TOKEN:
#   GitHub → Repo Settings → Actions → Runners → "New self-hosted runner"
#   The token is shown in the displayed `./config.sh` snippet. It expires in
#   ~1 hour, so generate it just before running this script.

set -euo pipefail

# === Defaults ===

# Bump RUNNER_VERSION + refresh RUNNER_SHA256 in tandem when upgrading.
# Find current releases at https://github.com/actions/runner/releases — the
# release body includes a per-arch SHA-256 table. The pinned values below
# were captured on 2026-05-14 against the v2.334.0 release.
RUNNER_VERSION="${RUNNER_VERSION:-2.334.0}"
RUNNER_SHA256="${RUNNER_SHA256:-f44255bd3e80160eb25f71bc83d06ea025f6908748807a584687b3184759f7e4}"
RUNNER_NAME="${RUNNER_NAME:-pi5-nightly}"
RUNNER_LABEL="${RUNNER_LABEL:-raspberry-pi}"
RUNNER_USER="${RUNNER_USER:-gh-runner}"
REPO_URL="${REPO_URL:-https://github.com/ivonnyssen/rusty-photon}"

ARCH_TAG="linux-arm64"
RUNNER_TARBALL="actions-runner-${ARCH_TAG}-${RUNNER_VERSION}.tar.gz"
RUNNER_URL="https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}/${RUNNER_TARBALL}"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31mxx\033[0m %s\n' "$*" >&2; exit 1; }

# === Pre-flight ===

[[ $EUID -ne 0 ]] || die "Do not run this script as root. It will use sudo for the parts that need it."

case "$(uname -sm)" in
  "Linux aarch64") ;;
  *) die "Expected Linux aarch64 (Pi 5 64-bit OS). Got: $(uname -sm)" ;;
esac

command -v sudo >/dev/null || die "sudo is required but not installed."
command -v curl >/dev/null || sudo apt-get install -y curl
command -v jq >/dev/null   || true   # jq is installed below if missing

# === 1. System dependencies ===

log "Installing system dependencies (apt-get)..."
sudo apt-get update -qq
sudo apt-get install -y \
  build-essential \
  pkg-config \
  curl \
  git \
  jq \
  libssl-dev \
  libcfitsio-dev \
  libusb-1.0-0-dev \
  unzip \
  ca-certificates

# === 1b. QHYCCD SDK (proprietary, pinned 25.09.29) ===
#
# qhy-camera links libqhyccd-sys (links = "qhyccd") + libusb-1.0 + stdc++. The
# Pi5 arm64 nightly builds the full workspace, so the SDK must be present and
# discoverable or qhy-camera fails to link. On Linux libqhyccd-sys hard-codes the
# linker search path `/usr/local/lib`, so the SDK's own install.sh (which copies
# into /usr/local) is what we run. The SDK is publicly downloadable from
# qhyccd.com (the `ivonnyssen/qhyccd-sdk-install` action does the same for the
# x86_64 GitHub runners; that action does NOT cover linux-arm64, hence this).
#
# The arm64 tarball name on qhyccd.com differs from the x86_64 `sdk_linux64_*`
# one — set QHY_SDK_FILE to the correct aarch64 archive for this release (the
# default is a best guess; confirm it on qhyccd.com's SDK page). Set
# QHY_SDK_SKIP=1 to skip (e.g. when the SDK is already installed by hand).
QHY_SDK_VERSION="${QHY_SDK_VERSION:-25.09.29}"
QHY_SDK_BASE="${QHY_SDK_BASE:-https://www.qhyccd.com/file/repository/publish/SDK}"
QHY_SDK_FILE="${QHY_SDK_FILE:-sdk_Arm64_${QHY_SDK_VERSION}.tgz}"
if [[ "${QHY_SDK_SKIP:-0}" == "1" ]]; then
  log "QHY_SDK_SKIP=1; skipping QHYCCD SDK install."
elif [[ -f /usr/local/lib/libqhyccd.a ]]; then
  log "QHYCCD SDK already installed at /usr/local/lib; skipping."
else
  log "Installing QHYCCD SDK $QHY_SDK_VERSION (aarch64) from qhyccd.com into /usr/local..."
  TMP=$(mktemp -d)
  if curl -fsSL --retry 3 --retry-delay 2 -o "$TMP/qhy-sdk.tgz" \
       "${QHY_SDK_BASE%/}/${QHY_SDK_VERSION}/${QHY_SDK_FILE}"; then
    tar xzf "$TMP/qhy-sdk.tgz" -C "$TMP"
    SDK_DIR=$(find "$TMP" -maxdepth 1 -type d -name 'sdk_*' | head -n1)
    if [[ -n "$SDK_DIR" && -f "$SDK_DIR/install.sh" ]]; then
      (cd "$SDK_DIR" && sudo sh install.sh)   # copies into /usr/local + ldconfig
    else
      # Fall back to copying the lib/include tree directly.
      LIB_SRC=$(find "$TMP" -type f -name 'libqhyccd.a' -printf '%h\n' | head -n1)
      INC_SRC=$(find "$TMP" -type f -name 'qhyccd.h' -printf '%h\n' | head -n1)
      [[ -n "$LIB_SRC" ]] && sudo cp -a "$LIB_SRC"/libqhyccd.* /usr/local/lib/
      [[ -n "$INC_SRC" ]] && sudo cp -a "$INC_SRC"/*.h /usr/local/include/
      sudo ldconfig
    fi
    log "QHYCCD SDK installed into /usr/local/lib."
  else
    log "Could not download $QHY_SDK_FILE from qhyccd.com; set QHY_SDK_FILE to the"
    log "  correct aarch64 archive (or install the SDK by hand). qhy-camera will not"
    log "  link on this runner until the SDK is present in /usr/local/lib."
  fi
  rm -rf "$TMP"
fi

# === 2. Dedicated unprivileged user ===

if id -u "$RUNNER_USER" >/dev/null 2>&1; then
  log "User $RUNNER_USER already exists; skipping creation."
else
  log "Creating user $RUNNER_USER (nologin shell, no sudo)..."
  sudo useradd -m -s /usr/sbin/nologin -U "$RUNNER_USER"
fi

RUNNER_HOME=$(getent passwd "$RUNNER_USER" | cut -d: -f6)
[[ -n "$RUNNER_HOME" && -d "$RUNNER_HOME" ]] || die "Could not resolve $RUNNER_USER home directory."

# === 3. Rustup + stable toolchain (as RUNNER_USER) ===

if sudo -u "$RUNNER_USER" test -x "$RUNNER_HOME/.cargo/bin/cargo"; then
  log "rustup already installed for $RUNNER_USER; running rustup update stable."
  sudo -u "$RUNNER_USER" bash -lc '. $HOME/.cargo/env && rustup update stable'
else
  log "Installing rustup + stable toolchain for $RUNNER_USER..."
  sudo -u "$RUNNER_USER" bash -lc '
    curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --default-toolchain stable --profile minimal
  '
fi

# === 4. Download + extract the runner tarball ===

RUNNER_DIR="$RUNNER_HOME/actions-runner"

if sudo -u "$RUNNER_USER" test -f "$RUNNER_DIR/config.sh"; then
  log "Runner files already present at $RUNNER_DIR; skipping download/extract."
else
  log "Downloading runner $RUNNER_VERSION ($ARCH_TAG)..."
  sudo -u "$RUNNER_USER" mkdir -p "$RUNNER_DIR"
  sudo -u "$RUNNER_USER" bash -lc "
    set -euo pipefail
    cd '$RUNNER_DIR'
    curl -fsSL --retry 3 --retry-delay 2 -o '$RUNNER_TARBALL' '$RUNNER_URL'
  "

  if [[ -n "$RUNNER_SHA256" ]]; then
    log "Verifying SHA-256 of $RUNNER_TARBALL..."
    ACTUAL=$(sudo -u "$RUNNER_USER" sha256sum "$RUNNER_DIR/$RUNNER_TARBALL" | awk '{print $1}')
    if [[ "$ACTUAL" != "$RUNNER_SHA256" ]]; then
      die "SHA-256 mismatch.
  expected: $RUNNER_SHA256
  actual:   $ACTUAL
Refusing to extract. Verify RUNNER_VERSION + RUNNER_SHA256 against the GitHub releases page."
    fi
    log "SHA-256 match."
  else
    warn "RUNNER_SHA256 not set — skipping integrity check. Set it on production setup."
  fi

  sudo -u "$RUNNER_USER" bash -lc "
    set -euo pipefail
    cd '$RUNNER_DIR'
    tar xzf '$RUNNER_TARBALL'
    rm '$RUNNER_TARBALL'
  "
fi

# === 5. Register the runner with GitHub ===

if [[ -z "${RUNNER_TOKEN:-}" ]]; then
  cat >&2 <<EOF

RUNNER_TOKEN is not set. To register the runner, generate a registration
token at:

  ${REPO_URL}/settings/actions/runners/new

(scroll to the \`./config.sh\` snippet; copy the value after \`--token\`).

Then re-run:

  RUNNER_TOKEN=<token> $0

System packages, the $RUNNER_USER user, rustup, and the runner binaries are
already in place — re-running with RUNNER_TOKEN set will skip them and only
register + install the service.

EOF
  exit 1
fi

# If a previous registration exists, --replace lets config.sh overwrite it
# without us having to call `config.sh remove` first.
log "Configuring runner $RUNNER_NAME (label: $RUNNER_LABEL)..."
sudo -u "$RUNNER_USER" bash -lc "
  set -euo pipefail
  cd '$RUNNER_DIR'
  ./config.sh \
    --url '$REPO_URL' \
    --token '$RUNNER_TOKEN' \
    --name '$RUNNER_NAME' \
    --labels '$RUNNER_LABEL' \
    --work _work \
    --unattended \
    --replace
"

# === 6. Install + start the systemd service ===
#
# svc.sh writes to /etc/systemd/system/ (root-only) AND reads template files
# from its own directory (under $RUNNER_DIR). Ubuntu Server 24.04 creates
# $RUNNER_HOME with mode 0750 by default, so the calling sudo-capable user
# typically cannot `cd` into $RUNNER_DIR; the cd must happen *inside* the
# sudo invocation so root does both the directory entry and the install.

log "Installing systemd service..."
sudo bash -c "cd '$RUNNER_DIR' && ./svc.sh install '$RUNNER_USER'"

log "Starting service..."
sudo bash -c "cd '$RUNNER_DIR' && ./svc.sh start"

# svc.sh names the unit something like
# actions.runner.<owner>-<repo>.<runner-name>.service. Derive it for status.
REPO_SLUG="$(echo "$REPO_URL" | sed -E 's|https?://github.com/||; s|/|-|')"
UNIT="actions.runner.${REPO_SLUG}.${RUNNER_NAME}.service"

log "Service status:"
sudo systemctl --no-pager status "$UNIT" || true

cat <<EOF

Runner setup complete. The runner should appear as "Idle" within seconds at:

  ${REPO_URL}/settings/actions/runners

Follow live logs with:

  sudo journalctl -u $UNIT -f

Trigger an immediate verification run from the Actions tab:

  ${REPO_URL}/actions/workflows/pi-nightly.yml  ->  "Run workflow"

EOF
