# Skill: Raspberry Pi 5 Self-Hosted Nightly Runner

## When to Read This

- Setting up the Raspberry Pi 5 self-hosted runner for the first time
- Re-registering the runner after a token expiry or factory reset
- Debugging a red `pi-nightly` workflow run
- Decommissioning the runner (removing from GitHub + the Pi itself)
- Auditing the security posture of self-hosted runners on this repo

## Prerequisites

- A Raspberry Pi 5 (Linux/ARM64) running Ubuntu 24.04 LTS or newer
- SSH access to the Pi as a sudo-capable user
- Owner or admin access to the `ivonnyssen/rusty-photon` GitHub repo
- A network position that lets the Pi reach `github.com` and `*.actions.githubusercontent.com` over HTTPS

## Why a Self-Hosted Runner (and Why It Is Safe Here)

GitHub-hosted runners only cover x86_64 (`ubuntu-latest`, `macos-latest`,
`windows-latest`). The Pi 5 is ARM64, so a Pi nightly catches arch-specific
regressions the rest of CI cannot: atomics, alignment, vendored C
dependencies such as `cfitsio` / `fitsio-sys`, cross-arch feature
unification breaks, and BDD scenarios that exercise endian-sensitive
serialisation paths.

This repo is **public**, which is the case GitHub's own docs warn against:
"We recommend that you only use self-hosted runners with private
repositories. This is because forks of your public repository can
potentially run dangerous code on your self-hosted runner machine by
creating a pull request that executes the code in a workflow."

The threat is concrete: a malicious PR can edit the workflow YAML, and on
`pull_request` events GitHub Actions runs the **PR's version** of that
YAML, not main's. So if any workflow on a self-hosted runner triggers on
`pull_request`, a fork can execute arbitrary commands on the Pi during PR
validation.

`pi-nightly.yml` neutralises this by triggering **only** on `schedule` and
`workflow_dispatch`. Scheduled runs always use the workflow file from the
default branch (main), and only the repo owner can push to main, so PRs
cannot influence what executes on the Pi. The job adds two more belts:
`ref: main` on `actions/checkout` and `if: github.ref == 'refs/heads/main'`
at the job level.

### Why no `pull_request` trigger

The "I'd like ARM coverage on PRs too" temptation must be resisted on a
public repo until either (a) the runner is moved to a private mirror, or
(b) a Just-In-Time (JIT) ephemeral runner pool with PR-approval gating is
set up. Both are substantial changes. For now, the rule is binary: this
file gets `schedule:` and `workflow_dispatch:` only.

If you ever need ARM-on-PR coverage, prefer GitHub's free `ubuntu-24.04-arm`
runner (free for public repos) — see
[github.com/actions/runner-images](https://github.com/actions/runner-images).

## One-Time Setup

The setup script `scripts/setup-pi-runner.sh` is the canonical, idempotent
path. The sections below explain what the script does so an operator can
audit or reproduce it manually.

### 1. System dependencies

The Pi needs a small set of packages that GitHub-hosted runners pre-install
but Ubuntu Server does not:

```bash
sudo apt-get update
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
```

`libcfitsio-dev` is required by the `fitsio-sys` build script. Without it
the `rp-fits`, `filemonitor`, and `sky-survey-camera` packages fail to
compile (this is the "use `-p <package>`" caveat that the user-level
`MEMORY.md` references). `libssl-dev` is required by transitive C-FFI
crates in the workspace. `libusb-1.0-0-dev` is required by the QHYCCD SDK
that `qhy-camera` links (next paragraph).

#### QHYCCD SDK (for `qhy-camera`)

`qhy-camera` links the proprietary QHYCCD SDK (`libqhyccd-sys` →
`static=qhyccd` + `libusb-1.0` + `stdc++`). The Pi5 arm64 nightly builds the
full workspace, so the SDK (pinned **25.09.29**, aarch64) must be installed —
the `setup-pi-runner.sh` `=== 1b. QHYCCD SDK ===` section downloads it from
**qhyccd.com** (publicly, no auth) and runs the SDK's `install.sh`, landing the
libs in `/usr/local/lib` (where `libqhyccd-sys`'s `build.rs` hard-codes the
linker search path). The published `ivonnyssen/qhyccd-sdk-install@v2` action used
by the GitHub-hosted jobs covers x86_64-linux, macOS, and Windows but **not**
linux-arm64, hence the Pi-side install; the arm64 archive name on qhyccd.com
differs from the x86_64 `sdk_linux64_*` one, so set `QHY_SDK_FILE` to the correct
aarch64 archive (or `QHY_SDK_SKIP=1` if the SDK is already installed by hand).
The GitHub-hosted **ubuntu, macOS, and Windows** jobs all build qhy-camera with
the SDK installed via the action; only the sanitizer job (`safety.yml`) excludes
it. The Pi covers linux-arm64.

### 2. Dedicated unprivileged user

The runner must not run as root. Create a dedicated user with no sudo
rights, no shell login (only the runner's `actions-runner` directory
matters), and a fresh home dir:

```bash
sudo useradd -m -s /usr/sbin/nologin -U gh-runner
```

The `nologin` shell prevents interactive logins; the runner's `run.sh` is
invoked by systemd, which doesn't need a login shell.

If the runner needs `rustup` (which it does — `dtolnay/rust-toolchain@stable`
calls it), the toolchain lives under `~gh-runner/.rustup` and
`~gh-runner/.cargo`. That's fine — both are inside the dedicated user's
home and isolated from any other user on the Pi.

### 3. Rustup + stable toolchain

`dtolnay/rust-toolchain@stable` installs rustup on first call if missing,
but it's faster to pre-install:

```bash
sudo -u gh-runner bash -c '
  curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
  echo "source $HOME/.cargo/env" >> $HOME/.bashrc
'
```

`cargo-nextest` is installed by the workflow via `taiki-e/install-action`,
so no pre-install needed.

### 4. Download and register the runner

GitHub's runner ships as a single tarball per OS/arch combo. Get the
latest ARM64 Linux runner from
[github.com/actions/runner/releases](https://github.com/actions/runner/releases).

```bash
sudo -u gh-runner bash -c '
  mkdir -p $HOME/actions-runner
  cd $HOME/actions-runner
  RUNNER_VERSION=2.334.0   # check releases page for current
  curl -fsSL -o actions-runner.tar.gz \
    https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}/actions-runner-linux-arm64-${RUNNER_VERSION}.tar.gz
  tar xzf actions-runner.tar.gz
  rm actions-runner.tar.gz
'
```

Registration requires a **runner registration token**, which is
short-lived (expires in ~1 hour) and must be fetched from GitHub:

> Repo → Settings → Actions → Runners → New self-hosted runner → copy the
> token shown in the `./config.sh ...` snippet

Then on the Pi:

```bash
sudo -u gh-runner bash -c '
  cd $HOME/actions-runner
  ./config.sh \
    --url https://github.com/ivonnyssen/rusty-photon \
    --token <TOKEN_FROM_GITHUB_UI> \
    --name pi5-nightly \
    --labels raspberry-pi \
    --work _work \
    --unattended \
    --replace
'
```

The `--labels raspberry-pi` value must match the workflow's
`runs-on: [self-hosted, Linux, ARM64, raspberry-pi]` (the first three
labels are auto-applied by GitHub based on the runner's environment).

`--replace` lets re-registration overwrite a stale entry without manual
deregistration in the UI — useful if the Pi is reimaged.

### 5. Install as a systemd service

GitHub ships an installer for this. Note the `sudo bash -c '...'` wrapping:
`svc.sh` writes to `/etc/systemd/system/` (root-only) and reads template
files from its own directory, but Ubuntu Server 24.04 creates
`/home/gh-runner` with mode `0750` so your regular sudo user can't `cd`
into it. Running the whole compound under `sudo` lets root do both the
directory entry and the install:

```bash
sudo bash -c 'cd /home/gh-runner/actions-runner && ./svc.sh install gh-runner && ./svc.sh start'
```

The service is named `actions.runner.ivonnyssen-rusty-photon.pi5-nightly.service`
(or similar). Verify:

```bash
systemctl status actions.runner.ivonnyssen-rusty-photon.pi5-nightly.service
sudo journalctl -u actions.runner.ivonnyssen-rusty-photon.pi5-nightly.service -f
```

From GitHub: Repo → Settings → Actions → Runners — the runner should show
as **Idle** within a few seconds.

## Operational Notes

### Triggering a run manually

`pi-nightly.yml` exposes `workflow_dispatch`. From the Actions tab:

> Actions → pi-nightly → Run workflow → Branch: main → Run workflow

`workflow_dispatch` enforces the same `if: github.ref == 'refs/heads/main'`
gate as the scheduled trigger, so this can only run against main.

### Reading logs

Live job logs appear in the Actions tab as usual. The runner-side daemon
log (start/stop, job pickup, deregistration events) is in journald:

```bash
sudo journalctl -u actions.runner.ivonnyssen-rusty-photon.pi5-nightly.service -f
```

The job's working tree lives under `~gh-runner/actions-runner/_work/rusty-photon/rusty-photon`
between runs. The runner's default behaviour is to wipe the workspace
between jobs but preserve cached toolchains (`.rustup`, `.cargo`) in the
user's home dir.

### Disk usage

Cargo's `target/` directory under `_work/` can grow to 5-10 GB across a
mixed workspace + all-features build. The Swatinem cache only restores
incremental state, so the active `target/` is unavoidable. Plan for at
least 32 GB of free disk on the Pi (an external SSD is strongly
recommended over an SD card for write durability).

### Notifications

`pi-nightly.yml` includes a `notify-on-failure` job that opens or updates
a `pi-nightly`-labelled issue in the repo on every scheduled failure.
This runs on a GitHub-hosted runner, so it still fires when the Pi itself
is offline (the `arm64-stable` job will fail with "runner offline" and
`notify-on-failure` reports it).

GitHub also sends an email by default to the workflow author when a
scheduled run fails — controlled at
github.com → Settings → Notifications → Actions.

## Re-Registering the Runner

The runner registration token GitHub gave you at setup time was one-time;
it cannot be re-used. To re-register (typically after a reimage or moving
the Pi):

1. On GitHub: Repo → Settings → Actions → Runners → click the runner row →
   "Remove runner" (or do nothing — `--replace` at config time handles a
   stale entry).
2. Generate a fresh token from the "New self-hosted runner" UI on that same
   page.
3. On the Pi (the `sudo -u gh-runner bash -c '...'` wrapping is for the
   same `0750` home-directory reason as §5 — `config.sh` expects to run
   from inside its own directory):
   ```bash
   sudo systemctl stop actions.runner.ivonnyssen-rusty-photon.pi5-nightly.service
   sudo -u gh-runner bash -c 'cd /home/gh-runner/actions-runner && ./config.sh remove --token <REMOVAL_TOKEN>'
   sudo -u gh-runner bash -c 'cd /home/gh-runner/actions-runner && ./config.sh \
     --url https://github.com/ivonnyssen/rusty-photon \
     --token <FRESH_REGISTRATION_TOKEN> \
     --name pi5-nightly \
     --labels raspberry-pi \
     --work _work \
     --unattended \
     --replace'
   sudo systemctl start actions.runner.ivonnyssen-rusty-photon.pi5-nightly.service
   ```

The removal token and registration token are different and are both shown
in the GitHub UI when needed.

## Decommissioning

If the Pi is going away or the workflow is being retired:

1. On the Pi:
   ```bash
   sudo bash -c 'cd /home/gh-runner/actions-runner && ./svc.sh stop && ./svc.sh uninstall'
   sudo -u gh-runner bash -c 'cd /home/gh-runner/actions-runner && ./config.sh remove --token <REMOVAL_TOKEN>'
   sudo userdel -r gh-runner
   ```
2. On GitHub: confirm the runner is gone from
   Settings → Actions → Runners.
3. Delete `.github/workflows/pi-nightly.yml`, this runbook, and
   `scripts/setup-pi-runner.sh`. Update `README.md` and `.github/DOCS.md`
   to drop the references.

## Troubleshooting

### Runner shows as "Offline" in the GitHub UI

Most common cause: systemd service stopped or network outage. Check:

```bash
systemctl status actions.runner.ivonnyssen-rusty-photon.pi5-nightly.service
sudo journalctl -u actions.runner.ivonnyssen-rusty-photon.pi5-nightly.service -n 200
ping -c 3 github.com
```

If the service is `failed`, look for "Token has expired" — that means the
registration was revoked from the UI side. Re-register (see above).

### `cargo build` fails with "could not find pkg-config" or "openssl-sys"

System deps were not installed for the `gh-runner` user's PATH. Re-run the
apt-get block from §"System dependencies". Confirm `pkg-config --version`
works as the `gh-runner` user:

```bash
sudo -u gh-runner pkg-config --version
```

### `fitsio-sys` build script fails

`libcfitsio-dev` missing. Install it; no rebuild flags needed.

### nextest runs but BDD hangs / OmniSim crashes

The Pi may be hitting one of the same intermittent OmniSim issues
addressed in `test.yml`. The workflow uploads OmniSim logs on failure to
the `omnisim-logs-pi-nightly` artifact — download it from the failed run
and investigate per `crates/bdd-infra/src/rp_harness/omnisim.rs`.

### Cache misses every night

Confirm the `shared-key` in `pi-nightly.yml` matches across runs
(`linux-arm64-stable`). GitHub's Actions cache has a 10 GB cap per repo,
so a very full cache namespace can also evict ARM entries — check repo
Settings → Actions → Caches.

## References

- [CLAUDE.md / AGENTS.md](../../CLAUDE.md) — operating rules (rules 4–6 govern
  pre-push gates and commit format)
- [docs/skills/pre-push.md](pre-push.md) — quality-gate suite this nightly
  approximates
- [.github/workflows/pi-nightly.yml](../../.github/workflows/pi-nightly.yml) —
  the workflow itself (read the header comment for the security model)
- [scripts/setup-pi-runner.sh](../../scripts/setup-pi-runner.sh) — idempotent
  setup script
- [GitHub: Self-hosted runners — security](https://docs.github.com/en/actions/hosting-your-own-runners/managing-self-hosted-runners/about-self-hosted-runners#self-hosted-runner-security)
  — the upstream warning this skill mitigates
- [GitHub Actions runner releases](https://github.com/actions/runner/releases)
  — current `linux-arm64` tarballs and changelogs
