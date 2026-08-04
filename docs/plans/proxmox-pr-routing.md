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

| Phase | Description | Status |
|-------|-------------|--------|
| R1 | Isolation + credential hardening: runner VLAN, write credential removed from runner `.env` | Done |
| R2 | Route Linux: conditional `runs-on` in bazel.yml, LAN write secret gated on push, provisioning guards, kill switch | Done |
| R3 | Windows runner template + orchestrator pool slots (Windows slot, second Linux slot) | Done |
| R4 | Route Windows: `bazel / windows-latest` with the `RP_POOL_WINDOWS` kill switch | Done |
| R4b | Route msi.yml `build-verify` | Blocked on a timing measurement |
| R5 | Route `bazel coverage` | Planned |

Current state: `bazel / ubuntu-latest` and `bazel / windows-latest` run on
the pool for push-to-main and same-repo PRs; `bazel coverage` and the macOS
leg do not. Templates and clone VMIDs live in the `SLOTS` array in
`tools/ci/rp-runner-pool.sh`, which is the source of truth for what the pool
runs — check there, not this document.

R5 is ready to start: it needs the routing expression and a second Linux
slot (a PR event fires both Linux legs at once, so routing coverage without
one would queue one behind the other on every PR), and both are in place.
R4b needs a measurement first.

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
colliding with the phase identifiers.

| Event | Linux + Windows legs run on | Cache | Cache writes |
|---|---|---|---|
| `pull_request`, same-repo branch | pool | LAN | no (anonymous read) |
| `pull_request`, fork (after approval) | GitHub-hosted | cloud | no |
| `push` to main | pool | LAN | yes (repo secret) |
| nightly `schedule` | GitHub-hosted | cloud | yes (as today) |
| macOS leg (always) | GitHub-hosted | cloud | as today |

Each OS carries its own kill switch — `RP_POOL_LINUX` and `RP_POOL_WINDOWS`
— because the two venues fail independently: a wedged Windows slot or a
stale Windows template should not cost Linux its speed, and vice versa.
Flipping one moves only that OS back to GitHub-hosted runners.

The nightly schedule staying **hosted** is deliberate: it is what keeps the
cloud cache's Linux entries warm, so a fork PR (which always runs hosted)
still gets a warm cache. The LAN cache is instead warmed by every push to
main.

## How routing works today

1. **Conditional `runs-on`** on bazel.yml's matrix job, one branch per pool
   OS:

   ```yaml
   runs-on: >-
     ${{ (matrix.os == 'ubuntu-latest'
          && vars.RP_POOL_LINUX == 'on'
          && (github.event_name == 'push'
              || (github.event_name == 'pull_request'
                  && github.event.pull_request.head.repo.full_name == github.repository))
          && fromJSON('["self-hosted", "proxmox-ephemeral"]'))
         || (matrix.os == 'windows-latest'
             && vars.RP_POOL_WINDOWS == 'on'
             && ...same trusted-event test...
             && fromJSON('["self-hosted", "proxmox-ephemeral-windows"]'))
         || matrix.os }}
   ```

   Every falsy branch resolves to `matrix.os` (hosted) — a fork PR, a
   schedule run, a deleted variable, or a null `head.repo` all land on the
   safe side. The trusted-event test is spelled out once per OS because
   `runs-on` is evaluated before the job exists, so neither the `env` context
   nor a job output can hold it; **the copies must stay identical — a
   divergence there is a security boundary moving.**

2. **Check names stay `bazel / <os>`** on both venues, so the
   `main_protection` ruleset needs no changes and a fork PR satisfies the
   same required check from a hosted runner.

3. **Provisioning steps are gated** on `runner.environment ==
   'github-hosted'`: both templates ship those tools at the same pins, and
   re-downloading them per ephemeral clone would put the WAN traffic back.
   Two Windows steps stay deliberately **ungated** — the long-paths registry
   key (downloads nothing, idempotent, and on the pool it guards against
   template drift) and `--output_base=C:/b`, which is load-bearing there
   because `C:\b` is the output base the template pre-warmed its external
   repos into.

4. **Cache flags.** Pool jobs override the cloud cache with
   `--remote_cache="$RP_LAN_CACHE_URL"`; hosted jobs keep
   `--config=remote-cache`. The LAN write credential
   (`BAZEL_LAN_CACHE_WRITE_AUTH`) attaches only on `push`, mirroring the
   cloud cache's public-read/token-write defense — fork PRs get no secrets at
   all, and same-repo PR events are excluded by the event gate.

5. **Runner VMs are VLAN-fenced** and carry no cache write credential; the
   security contract is ADR-020 and the skill doc, not this plan.

## Remaining work

### R4b — Route msi.yml `build-verify`

Blocked on a measurement that has not been taken. `build-verify` is Cargo,
not Bazel, so the LAN Bazel cache does not help it; the open questions are
raw cores versus hosted, and whether `Swatinem/rust-cache` (which pulls over
the WAN) helps or hurts on this link. Measure the release compile on the
Windows template first and route it only if it beats hosted. It is
`pull_request` path-triggered, so the same fork exclusion applies, and it is
not a required check, so a pool hiccup has a smaller blast radius.

### R5 — Route `bazel coverage`

The third required Bazel check (bazel-coverage.yml), same recipe as the
Linux leg above with three coverage-specific points:

1. **Expression.** Not a matrix job, so the routing expression above with a
   literal fallback: `… && fromJSON('["self-hosted", "proxmox-ephemeral"]') ||
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
