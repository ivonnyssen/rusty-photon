# Proxmox PR Routing Plan — real CI legs on the ephemeral runner pool

## Goal

Route real `pull_request`-triggered CI legs of this public repository to the
Proxmox ephemeral runner pool ([skill doc](../skills/proxmox-runner-pool.md)):
`bazel / ubuntu-latest`, `bazel coverage` (bazel-coverage.yml), and
`bazel / windows-latest` — the three required Bazel checks — plus msi.yml's
`build-verify` if its measurement pans out. Fork PRs stay on GitHub-hosted
runners and every layer of the security contract in
[ADR-020](../decisions/020-ephemeral-self-hosted-runners-for-pr-checks.md)
holds. Measured baseline: the pool completes the Linux Bazel steps in ~16 s
on an unchanged tree with a warm LAN cache versus 4–10 minutes hosted.

This deliberately supersedes the blanket "dispatch/schedule triggers only"
rule for the **ephemeral pool only**. Persistent self-hosted runners (the
Raspberry Pi nightly runner) keep the old rule unchanged — see ADR-020 for
the full layered contract and its rationale.

## Implementation Status

| Phase | Description | Status | Branch / PR |
|-------|-------------|--------|-------------|
| R1 | Isolation + credential hardening: runner VLAN (router + tagged template NIC), write credential removed from runner `.env`, fencing verified by dispatch job | **Done** (2026-08-02: probe matrix from inside a clone — GitHub + cache:8080 reachable, all other RFC1918 dropped; acceptance dispatch green through the fence) | infra only |
| R2 | Route Linux: conditional `runs-on` in bazel.yml (push + same-repo PRs), LAN write secret gated on push, skip provisioning steps on the pool, kill-switch variable, doc updates | **Done** (2026-08-02: PR's own ubuntu leg self-tested on the pool pre-merge; first push-to-main ran on the pool and grew the LAN cache +894 objects via the write secret) | PR #849 |
| R3 | Windows runner template: one-job service, template hygiene, orchestrator multi-slot rework (Windows slot + second Linux slot), validation dispatch job, msi compile timing measurement | **Template done** (2026-08-03: Server 2025 template VMID 904 builds `//...` green — 228/228 — and a linked clone boots to a responding agent in 10 s, waits for injection, consumes an injected config and powers off; msi measurement still outstanding) | PR #856, PR #857 |
| R4 | Route Windows: `bazel / windows-latest` via the same expression, Windows kill switch; msi `build-verify` only if the R3 measurement beats hosted | Planned | — |
| R5 | Route `bazel coverage`: same expression + provisioning guards in bazel-coverage.yml, Linux template warmed for the nightly/instrumented graph, shared `RP_POOL_LINUX` kill switch | Planned | — |

R1 is strictly first (it closes residual risk before exposure increases).
R2 depends on R1. R3 is independent of R2 and can proceed in parallel once
R1 lands. R4 depends on R2 (the proven expression) and R3. R5 depends on
R2 and on R3's orchestrator multi-slot rework (a PR event fires both Linux
legs at once, so routing coverage before the second Linux slot exists would
queue one of them behind the other on every PR) — it does not depend on the
Windows template itself and can land before or after R4.

Deferred beyond this plan: the macOS leg (requires physical Apple hardware;
the strongest motivation — the remote-cache wedge ladder — is tracked in
#765).

### Host capacity (20 cores / 94 GB)

Target steady state after R5: three warm slots — 2× Linux (16 vCPU,
24 GB each; shrunk from 32 GB, to be confirmed by measuring peak RAM during
a real job before the resize) + 1× Windows (16 vCPU, 28 GB) — ~76 GB
committed, leaving headroom for the host and the cache LXC. vCPU is
deliberately overcommitted (48 vCPU on 20 cores): jobs are bursty, and even
two full builds landing together get ~10 effective cores each, still a
multiple of the hosted runners' 4. A PR event fires at most three pool jobs
(ubuntu, coverage, windows); msi — if routed — queues briefly behind the
Windows bazel leg rather than earning a fourth slot.

## Venue and cache matrix

The single behavioral contract everything below implements:

In the table and throughout this plan, "cloud cache" means the Cloudflare
R2-backed remote cache (`--config=remote-cache`) — spelled out to avoid
colliding with the R1–R4 phase identifiers.

| Event | Linux leg runs on | Cache | Cache writes |
|---|---|---|---|
| `pull_request`, same-repo branch | pool | LAN | no (anonymous read) |
| `pull_request`, fork (after approval) | GitHub-hosted | cloud | no |
| `push` to main | pool | LAN | yes (repo secret) |
| nightly `schedule` | GitHub-hosted | cloud | yes (as today) |
| macOS / Windows legs (until R4) | GitHub-hosted | cloud | as today |

The nightly schedule staying **hosted** is deliberate: it is what keeps the
cloud cache's Linux entries warm, so a fork PR (which always runs hosted)
still gets a warm cache. The LAN cache is instead warmed by every push to
main.

## R1 — Isolation and credential hardening

Host-side only; no repository behavior changes.

1. **Runner VLAN.** Create a dedicated VLAN + subnet for runner clones on
   the router (DHCP on). The switch port carrying the Proxmox host trunks
   the existing LAN untagged plus the runner VLAN tagged. Router firewall:
   the runner VLAN may reach (a) the LAN build cache host on its cache port,
   (b) DNS, and (c) the WAN; all other RFC1918 destinations are blocked.
   The hypervisor's management interface stays untagged on the existing LAN
   and is unaffected.
2. **Proxmox side.** Make the bridge VLAN-aware; set the VLAN tag on the
   template VM's NIC so every linked clone inherits it. The orchestrator
   controls clones via the QEMU guest agent, which uses no network path, so
   isolation cannot break pool mechanics.
3. **Credential removal.** Delete the cache **write** credential
   (`RP_CACHE_AUTH`) from the template's runner `.env`; keep
   `RP_LAN_CACHE_URL` (needed for anonymous reads, still masked by every
   workflow that uses it). Any job on a self-hosted runner can read the
   machine's filesystem, so nothing a PR-reachable job could exfiltrate to
   poison the cache may live on the VM. `proxmox-runner-test.yml` already
   degrades gracefully (its write path is conditional on the variable being
   present).
4. **Acceptance.** A dispatched pool job proves: GitHub reachable, cache
   readable, and representative LAN addresses (hypervisor, rig, other
   hosts) unreachable from inside the clone.

## R2 — Route the Linux leg

One PR to bazel.yml plus repo settings; check names stay `bazel / <os>` so
the `main_protection` ruleset needs no changes and a fork PR satisfies the
same required check from a hosted runner.

1. **Conditional `runs-on`** on the matrix job:

   ```yaml
   runs-on: >-
     ${{ matrix.os == 'ubuntu-latest'
         && vars.RP_POOL_LINUX == 'on'
         && (github.event_name == 'push'
             || (github.event_name == 'pull_request'
                 && github.event.pull_request.head.repo.full_name == github.repository))
         && fromJSON('["self-hosted", "proxmox-ephemeral"]')
         || matrix.os }}
   ```

   Every falsy branch resolves to `matrix.os` (hosted) — the safe direction.
   A fork PR, a schedule run, a deleted variable, or a null `head.repo` all
   land on GitHub-hosted runners.
2. **Kill switch.** `RP_POOL_LINUX` is a repo Actions *variable*, opt-in
   (`== 'on'`): flippable in the UI or via `gh api` with no commit when the
   pool host is down. Failure symptom if it isn't flipped: the required
   check sits queued until GitHub's documented job-queue limit ends it (a
   self-hosted job queued for 24 hours is automatically cancelled — see
   [GitHub Actions limits](https://docs.github.com/en/actions/reference/limits)).
   Documented in the skill doc's ops section.
3. **LAN cache write secret.** New repo secret carrying the cache write
   credential, attached to the Bazel steps only when
   `github.event_name == 'push'` — the same event-gating pattern the cloud
   cache's `CACHE_WRITE_TOKEN` already uses. Fork PRs receive no
   secrets at all; same-repo PR events are excluded by the event gate. Pool
   jobs use `--remote_cache="$RP_LAN_CACHE_URL"`; hosted jobs keep
   `--config=remote-cache` (the cloud cache) exactly as today.
4. **Skip provisioning on the pool.** The install steps (lld, bazelisk,
   OmniSim, Pebble, QHY SDK, ZWO SDK) get
   `if: runner.environment == 'github-hosted'` — the template already ships
   all of them at the same pins, and re-downloading them per ephemeral clone
   would put the WAN traffic right back. **Coupling:** when a pin changes in
   bazel.yml, the runner template must be rebuilt (procedure in the skill
   doc); the workflow header and skill doc both carry a "pins live in two
   places" warning.
5. **Docs in the same PR:** amend the skill docs' trigger rule per ADR-020,
   update bazel.yml's header comments, and the skill doc's security model.
6. **Acceptance.** Per the operator's decision, push and same-repo PR
   routing ship together (no staged soak): first merged PR proves the PR
   path; the merge itself proves the push path and the first LAN cache
   write.

## R3 — Windows runner template

Mirrors the Linux template build (P2), with Windows specifics:

1. **Guest.** Windows Server evaluation ISO is sufficient (ephemeral clones
   live minutes; activation state is irrelevant to CI output). VirtIO
   drivers + QEMU guest agent installed so the orchestrator's
   guest-exec/JIT-injection path works unchanged.
2. **Provisioning** = union of what the two Windows legs need: MSVC Build
   Tools, stable Rust toolchain, bazelisk (pinned), long-paths registry key
   + `git config --system core.longpaths true`, OmniSim, Pebble, QHY SDK
   (win64), ZWO SDK, libclang, and the WiX toolset msi.yml uses.
3. **One-job service.** Windows equivalent of the Linux `gha-runner` unit: a
   service/scheduled task that waits for the injected JIT config, runs
   exactly one job (`run.cmd` with the JIT config), then shuts the VM down.
4. **Template hygiene.** Clear per-run state before converting to a template:
   any leftover `.jitconfig` (a clone must never start with a spent single-use
   config), the runner's `.runner`/`.credentials` pair, `_work`, `_diag`,
   temp dirs and event logs. NIC tagged with the runner VLAN.

   **Deliberately NOT `sysprep /generalize`,** despite it being the obvious
   analogue of the Linux template's `machine-id` wipe: its specialize pass
   runs on every clone boot and would add minutes to a pool whose value is
   fast pickup, and none of what it buys applies here — Proxmox assigns each
   clone a fresh MAC (so DHCP identity is already unique), only one Windows
   clone exists at a time (so the duplicate computer name cannot collide),
   the guests are not domain-joined, and the runner's identity comes from the
   injected JIT config rather than the host name.

   **Clock:** a linked clone inherits the template's RTC and boots badly out
   of date (~9 hours in testing). That breaks TLS to GitHub, and it broke the
   first one-job loop outright — a wall-clock wait deadline was already in
   the past once Windows corrected the time, so clones powered themselves off
   before the orchestrator could inject. The in-guest loop therefore forces a
   time resync before touching the network and measures its wait with a
   monotonic timer.
5. **Orchestrator.** `rp-runner-pool.sh` is reworked from one hard-coded
   warm clone into pool *slots* (per-slot template VMID, clone VMID, name
   prefix, labels), one maintenance loop per slot: a Windows slot
   (`self-hosted`, `Windows`, `X64`, `proxmox-ephemeral-windows`) and a
   second Linux slot (same labels as the first — GitHub dispatches a queued
   job to any idle matching runner), which R5 requires. Host sizing per the
   capacity section above.
6. **Validation + measurement.** Extend `proxmox-runner-test.yml` with a
   dispatch-only Windows job (same masked-env cache pattern). Also measure
   the msi `build-verify` release compile on the template: it is Cargo, not
   Bazel, so the LAN Bazel cache does not help it — the candidate wins are
   raw cores versus hosted, and whether `Swatinem/rust-cache` (which pulls
   the cache over the WAN) helps or hurts on this link. Route it in R4 only
   if the measurement beats hosted.

## R4 — Route the Windows legs

1. `bazel / windows-latest` gets the R2 expression with
   `vars.RP_POOL_WINDOWS` and the Windows pool labels; provisioning steps
   gain the same `runner.environment` guards.
2. msi.yml `build-verify` gets the same treatment (it is `pull_request`
   path-triggered, so the same fork exclusion applies; it is not a required
   check, so the blast radius of a pool hiccup is smaller).
3. Cache flags: the Windows Bazel leg uses the LAN cache like Linux
   (bazel-remote is platform-agnostic; Windows actions populate under
   distinct action keys). Write gating identical to R2.

## R5 — Route `bazel coverage`

The third required Bazel check (bazel-coverage.yml), same recipe as R2 with
three coverage-specific points:

1. **Expression.** Not a matrix job, so the R2 expression with the literal
   fallback: `… && fromJSON('["self-hosted", "proxmox-ephemeral"]') ||
   'ubuntu-latest'`. The event gate already sends the nightly `schedule` run
   to hosted runners — deliberate for the same reason as bazel.yml's: the
   schedule is what keeps the **cloud** cache's coverage entries warm for
   fork PRs. Kill switch: the same `RP_POOL_LINUX` variable — both Linux
   legs are healthy or unhealthy together (it is the same pool), and one
   flip must evacuate everything Linux.
2. **Cache split.** Same REMOTE_FLAGS block as bazel.yml: LAN URL override
   on the pool, LAN write auth (`BAZEL_LAN_CACHE_WRITE_AUTH`) on `push`
   only, `--remote_upload_local_results=false` otherwise. Push-to-main then
   warms the LAN cache's *coverage* namespace exactly as it does the
   build/test namespace. The provisioning steps (lld, bazelisk, OmniSim,
   Pebble, libusb, QHY SDK, ZWO SDK) get the same
   `runner.environment == 'github-hosted'` guards.
3. **Template warmup.** Coverage builds the whole graph **instrumented on
   the nightly toolchain** — a distinct action + external-repo namespace
   from the stable build/test the template was benched with. Before routing,
   the Linux template gets a one-time warmup (`bazel coverage` run to
   completion during template rebuild) so the nightly toolchain and the
   instrumented externals live in the template's output base; without it,
   every ephemeral clone would re-fetch the nightly toolchain over the WAN
   on every job. The codecov CLI download (~small, rolling `latest`) stays
   per-run; the Codecov upload runs from the pool over the WAN like any
   other egress.

## References

- [ADR-020](../decisions/020-ephemeral-self-hosted-runners-for-pr-checks.md)
  — the security contract this plan implements
- [Proxmox runner pool skill](../skills/proxmox-runner-pool.md) — pool
  architecture, ops, template rebuild procedure
- [Raspberry Pi runner skill](../skills/raspberry-pi-runner.md) — the
  unchanged rule for persistent runners
