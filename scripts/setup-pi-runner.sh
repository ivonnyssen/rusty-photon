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
  clang \
  libclang-dev \
  unzip \
  ca-certificates

# === 1b. QHYCCD SDK (proprietary, pinned 26.06.04) ===
#
# qhy-camera links libqhyccd-sys (links = "qhyccd") + libusb-1.0 + stdc++. The
# Pi5 arm64 nightly builds the full workspace, so the SDK must be present and
# discoverable or qhy-camera fails to link. On Linux libqhyccd-sys hard-codes the
# linker search path `/usr/local/lib`; the 26.x packaging ships no install.sh, so
# we copy the staged lib/include tree into /usr/local (the fallback below). The
# SDK is publicly downloadable from qhyccd.com. The
# `ivonnyssen/qhyccd-sdk-install@v3` action installs it on the GitHub runners
# (x86_64 Linux/macOS/Windows AND, as of v3, linux-arm64). We still provision it
# here at setup time rather than per-run via the action because this self-hosted
# runner is intentionally sudo-less (see the runner user below) and the action's
# install needs root to write /usr/local — pre-provisioning keeps the runner
# unprivileged.
#
# The 26.x scheme publishes under a dot-stripped directory (260604) with archive
# `sdk_linux_arm64_<version>.tar.gz` (the arm64 name differs from the x86_64
# `sdk_linux64_*`). Override QHY_SDK_DIR / QHY_SDK_FILE for other releases (the
# legacy <=25.x scheme used a dotted dir + `sdk_Arm64_*.tgz`); confirm names on
# qhyccd.com's SDK page. Set QHY_SDK_SKIP=1 to skip (SDK already installed).
QHY_SDK_VERSION="${QHY_SDK_VERSION:-26.06.04}"
QHY_SDK_BASE="${QHY_SDK_BASE:-https://www.qhyccd.com/file/repository/publish/SDK}"
# 26.x repository dir is the version with dots stripped (26.06.04 -> 260604).
QHY_SDK_DIR="${QHY_SDK_DIR:-${QHY_SDK_VERSION//./}}"
QHY_SDK_FILE="${QHY_SDK_FILE:-sdk_linux_arm64_${QHY_SDK_VERSION}.tar.gz}"
if [[ "${QHY_SDK_SKIP:-0}" == "1" ]]; then
  log "QHY_SDK_SKIP=1; skipping QHYCCD SDK install."
elif [[ -f /usr/local/lib/libqhyccd.a ]]; then
  log "QHYCCD SDK already installed at /usr/local/lib; skipping."
else
  log "Installing QHYCCD SDK $QHY_SDK_VERSION (aarch64) from qhyccd.com into /usr/local..."
  TMP=$(mktemp -d)
  if curl -fsSL --retry 3 --retry-delay 2 -o "$TMP/qhy-sdk.tgz" \
       "${QHY_SDK_BASE%/}/${QHY_SDK_DIR}/${QHY_SDK_FILE}"; then
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
    # Fail fast: a silent skip leaves the runner partially provisioned and
    # qhy-camera silently un-linkable on the next nightly. The operator opts out
    # of the SDK explicitly with QHY_SDK_SKIP=1; an *unexpected* download failure
    # must stop setup so it is noticed and fixed now.
    rm -rf "$TMP"
    die "Could not download $QHY_SDK_FILE from ${QHY_SDK_BASE%/}/${QHY_SDK_DIR}/. Set QHY_SDK_FILE to the correct aarch64 archive (or install the SDK by hand), or re-run with QHY_SDK_SKIP=1 to provision the rest of the runner without it. qhy-camera will not link on this runner until the SDK is in /usr/local/lib."
  fi
  rm -rf "$TMP"
fi

# === 1.5. ZWO ASI/EFW SDK (for the zwo-camera service) ===
#
# zwo-camera links the MIT-licensed ZWO SDK unconditionally via
# zwo-rs -> libzwo-sys (required even for `--features simulation`), so the
# aarch64 Pi runner needs it installed to build/test the workspace. INDI
# vendors ZWO's upstream prebuilt blobs under indi-3rdparty/libasi; install the
# armv8 libraries under the linker name plus the udev rule for ZWO USB devices.
# Pinned to a commit SHA (not a moving branch) for reproducible native blobs,
# matching the CI `install-zwo-sdk` action's `ref` default; bump both together to
# adopt a newer ZWO SDK. Override via the env var only for a deliberate one-off.
ZWO_SDK_REF="${ZWO_SDK_REF:-b0802f28055b67aa6a99580d260c3bb4c27eba4b}"
ZWO_SDK_BASE="https://github.com/indilib/indi-3rdparty/raw/${ZWO_SDK_REF}/libasi"

log "Installing ZWO ASI/EFW SDK (armv8) from indi-3rdparty@${ZWO_SDK_REF}..."
sudo install -d /usr/local/lib /usr/local/include
for header in ASICamera2.h EFW_filter.h EAF_focuser.h license.txt; do
  sudo curl -fsSL "${ZWO_SDK_BASE}/${header}" -o "/usr/local/include/${header}"
done
sudo curl -fsSL "${ZWO_SDK_BASE}/armv8/libASICamera2.bin" -o /usr/local/lib/libASICamera2.so
sudo curl -fsSL "${ZWO_SDK_BASE}/armv8/libEFWFilter.bin" -o /usr/local/lib/libEFWFilter.so
# EAF focuser is not linked yet (Future Work), but install it so the runner is
# ready when focuser support lands — mirrors zwo-rs/ci/install-zwo-sdk.sh.
sudo curl -fsSL "${ZWO_SDK_BASE}/armv8/libEAFFocuser.bin" -o /usr/local/lib/libEAFFocuser.so || true
sudo ldconfig

log "Installing ZWO udev rule (/etc/udev/rules.d/99-asi.rules)..."
sudo tee /etc/udev/rules.d/99-asi.rules >/dev/null <<'EOF'
# ZWO ASI cameras + EFW filter wheels (VID 0x03c3). MODE=0666 so the runner user
# can claim the device; raise the USB buffer for USB3 throughput.
SUBSYSTEMS=="usb", ATTR{idVendor}=="03c3", MODE="0666"
ACTION=="add", SUBSYSTEMS=="usb", ATTR{idVendor}=="03c3", RUN+="/bin/sh -c 'echo 200 > /sys/module/usbcore/parameters/usbfs_memory_mb'"
EOF
sudo udevadm control --reload-rules
sudo udevadm trigger || true

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
