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

* **Template VM** (`runner-template`): Ubuntu 24.04 provisioned exactly like
  `bazel.yml`'s Linux leg (lld, pinned bazelisk, the patched OmniSim fork,
  Pebble, QHYCCD SDK, ZWO SDK blobs — same pins, same SHA checks), plus the
  GitHub Actions runner and a `gha-runner` systemd unit that waits for a JIT
  config file, runs **exactly one job**, and powers the VM off. The
  template's machine-id and cloud-init state are wiped so every clone boots
  with a fresh identity.
* **Pool orchestrator** (`tools/ci/rp-runner-pool.sh`): runs on the Proxmox
  host; keeps one warm linked clone registered just-in-time and destroys it
  after its single job. See the script header for deployment.
* **LAN build cache**: a `bazel-remote` instance in a container on the same
  host — anonymous reads, credential-gated writes (same public-read /
  token-write model as the cloud R2 cache). Jobs receive the endpoint and
  write credential from the runner's `.env` (`RP_LAN_CACHE_URL`,
  `RP_CACHE_AUTH`), never from workflow files, and mask both before use so
  neither can appear in public logs.

## Security Model — DO NOT WEAKEN

The same reasoning as `docs/skills/raspberry-pi-runner.md` §"Why a
Self-Hosted Runner": this repo is public, and on `pull_request` events
Actions executes the PR's copy of the workflow YAML, so any workflow
triggerable by a fork must never target self-hosted runners.

* Workflows targeting `proxmox-ephemeral` get `workflow_dispatch:` (and at
  most `schedule:`) triggers — **never `pull_request` or `push`**. Dispatch
  requires write access to the repository.
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
* If PR-triggered coverage is ever added on top of this pool, fork PRs must
  be routed to GitHub-hosted runners (conditional `runs-on`), and the
  repo-level "require approval for all outside collaborators" fork-PR policy
  must stay enabled — approval remains the checkpoint for a fork PR that
  edits workflow YAML.

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
* The orchestrator logs to the journal of its systemd unit
  (`rp-runner-pool.service`) on the Proxmox host.
* An idle registered runner is a warm clone waiting for a dispatch; pickup
  is immediate. Replacement after a job takes under a minute (linked clone +
  boot + JIT registration).
* To update the runner toolchain (new SDK pin, new runner release), boot a
  fresh clone of the template, apply the change, wipe `/etc/machine-id`, run
  `cloud-init clean`, power off, and convert to the new template — then roll
  the template VMID forward in `rp-runner-pool.sh`.
* Host and template identifiers (VMIDs, addresses, credentials) are operator
  infrastructure and deliberately absent from this repo; the runner's `.env`
  carries what jobs need.

## Bootstrapping a Runner Manually (no orchestrator)

For one-off validation without the pool service: clone the template, start
it, mint a JIT config (`POST /orgs/<org>/actions/runners/generate-jitconfig`
with the labels above and the default runner group's id), and write it to
`/home/ci/actions-runner/.jitconfig` in the guest
(write to a temp file and `mv` — the service polls for the file). The clone
registers, waits for one job, runs it, and powers off.
