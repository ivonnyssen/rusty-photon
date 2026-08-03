# Skill: Proxmox Ephemeral Runner Pool (x86_64 Linux)

## When to Read This

Read this before touching `.github/workflows/proxmox-runner-test.yml`,
`tools/ci/rp-runner-pool.sh`, the runner VM template, or before pointing any
new workflow at the `proxmox-ephemeral` runner label.

## What This Is

A self-hosted, ephemeral GitHub Actions runner pool on a Proxmox VE host on
the operator's LAN, plus a LAN `bazel-remote` cache next to it. Measured
against the GitHub-hosted `bazel / ubuntu-latest` leg (which runs 4-core
runners and re-fetches its remote cache over the operator's WAN link every
run), the pool's 16-vCPU clones complete the same three Bazel steps in
roughly 16 seconds on an unchanged tree with a warm LAN cache, about 6.5
minutes on a fully cold cache — all with zero WAN traffic after the first
population.

Components:

* **Linux template VM** (`runner-template`): Ubuntu 24.04 provisioned exactly
  like `bazel.yml`'s Linux leg (lld, pinned bazelisk, the patched OmniSim fork,
  Pebble, QHYCCD SDK, ZWO SDK blobs — same pins, same SHA checks), plus the
  GitHub Actions runner and a `gha-runner` systemd unit that waits for a JIT
  config file, runs **exactly one job**, and powers the VM off. The
  template's machine-id and cloud-init state are wiped so every clone boots
  with a fresh identity.
* **Windows template VM** (`runner-template-win`): Windows Server 2025 — the
  same build as GitHub's `windows-latest` image — provisioned like
  `bazel.yml`'s Windows leg. Everything installs **machine-wide** under
  `C:\ci` and is located by MACHINE environment variables
  (`OMNISIM_PATH`, `PEBBLE_PATH`, `PEBBLE_CHALLTESTSRV_PATH`,
  `QHYCCD_SDK_DIR`, `ZWO_SDK_LIB_DIR`, `LIBCLANG_PATH`, `BAZELISK_HOME`,
  `CARGO_HOME`, `RUSTUP_HOME`, `RP_LAN_CACHE_URL`), which is the Windows
  analogue of the Linux runner's `.env`. The one exception is the runner
  itself, which lives at `C:\actions-runner` (mirroring the Linux
  `/home/ci/actions-runner`) — that is where the orchestrator injects
  `.jitconfig`, so a template rebuild must keep the path. A `gha-runner`
  scheduled task (AtStartup, SYSTEM) plays the systemd unit's role. Two Windows-only
  requirements: `BAZEL_SH` must point at Git's `bash.exe` or Bazel reports
  "No suitable shell toolchain found", and `QHYCCD_SDK_DIR` only reaches
  build actions because `.bazelrc` forwards it (a machine-wide SDK is
  invisible to the `GITHUB_WORKSPACE` fallback hosted runners rely on).
* **Pool orchestrator** (`tools/ci/rp-runner-pool.sh`): runs on the Proxmox
  host; keeps one warm linked clone per **pool slot** registered just-in-time
  and destroys it after its single job. Slots are declared in the script's
  `SLOTS` array (name, template VMID, clone VMID, guest OS, labels) and each
  runs its clone/register/wait/destroy loop concurrently. Two slots sharing a
  label set are interchangeable — that is how the two Linux slots keep
  `bazel.yml` and `bazel-coverage.yml`, which fire on the same PR event, from
  queueing behind each other. Every slot holds one powered-on clone, so host
  memory must cover their sum. See the script header for deployment.
* **LAN build cache**: a `bazel-remote` instance in a container on the same
  host — anonymous reads, credential-gated writes (same public-read /
  token-write model as the cloud R2 cache). Jobs receive the endpoint from
  the runner's `.env` (`RP_LAN_CACHE_URL`), never from workflow files, and
  mask it before use so it cannot appear in public logs. The **write**
  credential deliberately does not exist on the runner VM: it is a GitHub
  Actions secret (`BAZEL_LAN_CACHE_WRITE_AUTH`) that bazel.yml attaches
  only on push-to-main events, mirroring the cloud cache's poisoning
  defense (ADR-020 layer 4).

## Security Model — DO NOT WEAKEN

This repo is public, and on `pull_request` events Actions executes the
PR's copy of the workflow YAML — so self-hosted runners and fork PRs are a
dangerous combination. The rule bifurcates by runner kind
([ADR-020](../decisions/020-ephemeral-self-hosted-runners-for-pr-checks.md)):

* **Persistent** self-hosted runners (the Raspberry Pi nightly runner)
  keep the binary rule: `workflow_dispatch:`/`schedule:` triggers only,
  never `pull_request` or `push`. Non-negotiable.
* **This ephemeral pool** may serve `push` and **same-repo**
  `pull_request` jobs under ADR-020's six-layer contract: a fork-excluding
  `runs-on` expression (every falsy branch lands on GitHub-hosted), the
  fork-PR approval checkpoint, JIT single-use VMs, no credentials on the
  runner, VLAN fencing, and the `RP_POOL_LINUX` kill-switch variable.
  bazel.yml's Linux leg is the implementation. **Approving a fork PR's
  workflow runs is the human layer: review the workflow-file diff first —
  a fork can only reach this pool by editing `runs-on`.**
* Runners are **JIT-registered and single-use**: the config injected into a
  clone registers one runner for one job; a compromised job cannot mint
  further registrations, and the GitHub-side runner entry disappears after
  the job.
* The GitHub PAT (fine-grained, resource owner: the `rusty-photon`
  organization, sole permission "Self-hosted runners: Read and write") lives
  root-only on the hypervisor at `/etc/rp-runner/github-token`. It is never
  present inside any VM. Runners register at **org level** precisely because
  that permission exists only there — repo-level registration would require
  the far broader Administration permission (settings, deletion, teams).
  Free-plan orgs have only the default runner group, so org runners are
  usable by every repo in the org; the org is kept essentially
  single-project for that reason.
* Every job runs on a **fresh linked clone**; the clone powers off and is
  destroyed after its job. The only state shared between jobs is the build
  cache, whose writes are credential-gated.
* The runner VMs live on a dedicated VLAN whose router firewall allows
  exactly three things: the WAN, DNS, and the LAN cache's port. Everything
  else on RFC1918 is dropped — verified by probing from inside a clone.
  Pool control runs over the QEMU guest agent (no network path), so the
  fencing cannot break pool mechanics.
* The repo-level "require approval for all outside collaborators" fork-PR
  policy must stay enabled — approval is the checkpoint for a fork PR that
  edits workflow YAML (ADR-020 layer 2).

## Operational Notes

* **Org runner groups ship with "Allow public repositories" disabled.** A
  freshly registered org runner then sits Idle while jobs from this (public)
  repo stay queued forever — no error anywhere. Either check the box on the
  Default group under the org's Actions → Runner groups settings, or use
  the pool's own token against the GitHub REST API (find the group id via
  `GET` on the same path):

  ```sh
  curl -X PATCH \
    -H "Authorization: Bearer $(cat /etc/rp-runner/github-token)" \
    -H "Accept: application/vnd.github+json" \
    https://api.github.com/orgs/<org>/actions/runner-groups/<group-id> \
    -d '{"allows_public_repositories": true}'
  ```

  This is part of the one-time setup contract.
* **Kill switch:** routing of bazel.yml's Linux leg to the pool is gated on
  the repo Actions variable `RP_POOL_LINUX` being `on`. If the pool host is
  down, required checks sit queued with no error anywhere (GitHub cancels a
  self-hosted job only after 24 hours in queue) — unset or flip the
  variable (`gh variable set RP_POOL_LINUX --body off`) and re-run; jobs
  route back to GitHub-hosted runners with no commit needed.
* **Pins live in two places:** the hosted install steps in bazel.yml and
  the pool template carry the same toolchain pins (bazelisk, OmniSim,
  Pebble, camera SDKs). Bumping a pin in the workflow requires rebuilding
  the template (procedure below) — the pool otherwise keeps running the old
  pin silently.
* The orchestrator logs to the journal of its systemd unit
  (`rp-runner-pool.service`) on the Proxmox host.
* An idle registered runner is a warm clone waiting for a dispatch; pickup
  is immediate. Replacement after a job takes under a minute (linked clone +
  boot + JIT registration).
* To update the runner toolchain (new SDK pin, new runner release), boot a
  fresh clone of the template, apply the change, wipe `/etc/machine-id`, run
  `cloud-init clean`, power off, and convert to the new template — then roll
  the template VMID forward in `rp-runner-pool.sh`.
* What lives where: **VMIDs are in the repo**, in `rp-runner-pool.sh`'s
  `SLOTS` array — they are local to one hypervisor, meaningless anywhere else,
  and the orchestrator needs them to do its job. What is deliberately absent
  is anything that identifies or unlocks infrastructure: **addresses**
  (this repo is public — see the LAN cache endpoint, which reaches jobs only
  via the runner's `.env` and is masked before use) and **credentials** (the
  PAT lives on the hypervisor at `/etc/rp-runner/github-token`, the cache
  write credential only as a GitHub Actions secret). A reader should be able
  to see exactly which VM does what, and nothing about where it is or how to
  reach it.

## Bootstrapping a Runner Manually (no orchestrator)

For one-off validation without the pool service: clone the template, start
it, mint a JIT config (`POST /orgs/<org>/actions/runners/generate-jitconfig`
with the labels above and the default runner group's id), and write it to
`/home/ci/actions-runner/.jitconfig` in the guest
(write to a temp file and `mv` — the service polls for the file). The clone
registers, waits for one job, runs it, and powers off.
