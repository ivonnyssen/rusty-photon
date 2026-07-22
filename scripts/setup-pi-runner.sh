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
  libusb-1.0-0 \
  clang \
  libclang-dev \
  unzip \
  ca-certificates

# === 1b. QHYCCD SDK — now provisioned per-run by the workflow, not here ===
#
# qhy-camera links libqhyccd-sys (static=qhyccd + libusb-1.0 + stdc++), so the
# Pi5 arm64 nightly needs the SDK to build the full workspace. This used to be
# pre-installed into /usr/local here, because the published install action wrote
# to /usr/local with sudo and this runner is intentionally sudo-less. That is no
# longer necessary: `pi-nightly.yml` now provisions the SDK per-run with
# `ivonnyssen/qhyccd-sdk-install@v4` `install: env` — a sudo-free mode that
# extracts the SDK under the workspace and exports QHYCCD_SDK_DIR, which
# libqhyccd-sys's build.rs reads to find libqhyccd.a (linked statically).
# Provisioning in the workflow (rather than at setup time) makes the runner
# self-healing: a new native-SDK service or an SDK version bump no longer
# requires re-running this script by hand. The build.rs fallback to
# /usr/local/lib still works if an operator has installed the SDK there
# manually, so nothing breaks if it is.
#
# The static libqhyccd.a still pulls in a *dynamic* `-lusb-1.0`. On a sudo-less
# runner there is no libusb-1.0-0-dev (no unversioned `libusb-1.0.so` for the
# linker), so `pi-nightly.yml` symlinks the linker name to the libusb-1.0
# *runtime* `.so.0` inside QHYCCD_SDK_DIR per-run — exactly like §1.5's ZWO
# step. That is why §1 above installs only the libusb-1.0 *runtime* package
# (libusb-1.0-0), not the -dev package: it is just the symlink target, shared
# by both the QHYCCD and ZWO sudo-free link paths.

# === 1.5. ZWO ASI/EFW SDK — now provisioned sudo-free per-run by the workflow ===
#
# zwo-camera links the MIT-licensed ZWO SDK unconditionally via
# zwo-rs -> libzwo-sys (required even for `--features simulation`), so the
# aarch64 Pi nightly needs it at link time to build/test the workspace. This
# used to be pre-installed into /usr/local here (with sudo). That is no longer
# necessary: `pi-nightly.yml` now provisions it per-run with the local
# `./.github/actions/install-zwo-sdk` action in its sudo-free mode
# (`sudo: "false"`) — it stages the INDI-vendored blobs under $RUNNER_TEMP,
# symlinks the system libusb/libudev *runtime* libs to satisfy the unversioned
# `-lusb-1.0`/`-ludev` link names (no -dev package), and exports ZWO_SDK_LIB_DIR
# (which libzwo-sys' build.rs puts ahead of /usr/local/lib) + LD_LIBRARY_PATH.
# Like the QHYCCD §1b move, provisioning in the workflow makes the runner
# self-healing: a ZWO SDK bump only needs the action's `ref` updated, not a
# manual re-run of this script. The two prerequisites the per-run step cannot
# install without sudo are stable host packages installed once in §1 above:
# clang/libclang-dev (bindgen) and the libusb-1.0 runtime (the symlink target,
# and the blob's own runtime dependency); libudev.so.1 ships with systemd.
#
# Only the udev rule for real ZWO USB devices stays here (device *access* for
# on-Pi hardware testing — orthogonal to CI linking, and it genuinely needs root).

# === 1c. SVBony camera SDK — provisioned sudo-free per-run by the workflow ===
#
# svbony-camera links the SVBony camera SDK unconditionally via
# svbony-rs -> libsvbony-sys (required even for `--features simulation`,
# mirroring ZWO — see docs/services/svbony-camera.md "Native dependency &
# build gating"), so the aarch64 Pi nightly needs it at link time to
# build/test the workspace. Like QHYCCD/ZWO, this is never installed here:
# `pi-nightly.yml` provisions it per-run with the local
# `./.github/actions/install-svbony-sdk` action in its sudo-free mode
# (`sudo: "false"`) — it stages the INDI-vendored blob under $RUNNER_TEMP,
# symlinks the system libusb-1.0 *runtime* lib to satisfy the unversioned
# `-lusb-1.0` link name (no -dev package), and exports SVBONY_SDK_LIB_DIR
# (which libsvbony-sys' build.rs puts ahead of /usr/local/lib) +
# LD_LIBRARY_PATH. As with the QHYCCD/ZWO moves, provisioning in the
# workflow makes the runner self-healing: an SVBony SDK bump only needs the
# action's `ref` updated, not a manual re-run of this script. The only
# prerequisite the per-run step cannot install without sudo is the same
# libusb-1.0 runtime package installed once in §1 above (the symlink
# target); unlike ZWO, no clang/libclang-dev is needed — libsvbony-sys' FFI
# is hand-written, not bindgen'd.
#
# No udev rule here yet: no physical SVBony camera is available in this
# environment to test against (hardware is on order — see
# docs/services/svbony-camera.md "Real-hardware validation"). Add a
# `99-svbony.rules` block here (mirroring the ZWO one below, VID `f266` per
# services/svbony-camera/pkg/90-rusty-photon-svbony.rules) once real
# on-Pi hardware testing is set up.

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
