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

R8 extends the same contract past Proxmox to the one leg it cannot host:
`bazel / macos-latest` needs Apple hardware, so it gets a second hypervisor
rather than another node. Everything else — JIT single-use runners, the
fork-excluding `runs-on` expression, a per-OS kill switch, the LAN cache's
read-anonymous/token-write split — is meant to be shared, not reimplemented.

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
| R8 | Route `bazel / macos-latest` to a Mac mini (#893) | Planned, gated on a purchase |

Current state: `bazel / ubuntu-latest` and `bazel / windows-latest` run on
the pool for push-to-main and same-repo PRs; `bazel coverage` and the macOS
leg do not. Templates and clone VMIDs live in the `SLOTS` array in
`tools/ci/rp-runner-pool.sh`, which is the source of truth for what the pool
runs — check there, not this document.

R5 is ready to start: it needs the routing expression and a second Linux
slot (a PR event fires both Linux legs at once, so routing coverage without
one would queue one behind the other on every PR), and both are in place.
R4b needs a measurement first. R8 needs hardware this project does not own;
it is scoped here because the measurements below make it, not more x86
capacity, the next thing worth buying.

### Host capacity — the Proxmox host (14 cores / 20 threads, 94 GB)

R8's Mac is a second host with its own, much simpler envelope (two slots,
fixed by Apple's licence); this section is about the x86 pool only.

Slot RAM is **16 GB on both OSes**, measured rather than estimated. Method:
`bazel clean` then the full `build` + `test` + `bdd` sequence on a real slot,
sampling every 2s, run at two sizes so the elastic component is visible.

| | Linux (peak anon) | Windows (peak committed) |
|---|---|---|
| large slot | 9.35 GiB @ 32 GB | 14.63 GiB @ 20 GB |
| **16 GB slot** | **8.96 GiB** | **13.90 GiB** |
| headroom at 16 GB | 5.23 GiB available | 6.29 GiB available, 3% pagefile |
| wall clock | 479s → 459s | 578s → 578s |

Two things that make "peak vs slot size" the wrong way to budget:

* **Demand is elastic.** Bazel sizes its JVM heap and its action concurrency
  (`--local_ram_resources` defaults to 67% of visible RAM) from the box it is
  given, so halving the slot *lowered* peak demand on both OSes. Shrinking a
  slot partly shrinks the workload.
* **The two numbers are not comparable to each other.** Linux `AnonPages` and
  Windows *committed bytes* are each the metric that governs their own OS's
  failure mode — an OOM kill on Linux, commit exhaustion on Windows. Windows
  genuinely costs more (see #874: `--jobs=64` permits 64 heavyweight processes
  on a 16-core guest, and the peak lands in the link phase), but the ~1.5×
  ratio is indicative, not arithmetic.

The rule: **slot RAM ≥ 1.5× the measured peak of the heaviest workload,
re-measured when that workload changes.** The bazel job is the heaviest — the
MSI packaging job peaks near 9 GiB, well under it, because cargo self-limits
to core count.

**Disk, not RAM, is the binding constraint on slot count.** Clone disks belong
on `cipool` (the 4 TB NVMe), not the root mirror. Measured with fio, ZFS
file-based, mixed 70/30 16k — one job per simulated slot:

| concurrent jobs | rpool (500 GB QLC mirror) | cipool (4 TB) |
|---|---|---|
| 1 | 2,595 IOPS | 6,733 IOPS |
| 2 | 3,336 IOPS | 9,846 IOPS |
| 3 | **3,259 IOPS — declines** | **11,258 IOPS — still scaling** |

The root mirror saturates between one and two concurrent jobs and gets *worse*
at three, with p99.9 latency reaching 3.5s; 1.27% of random writes exceed two
seconds. That is QLC past its SLC cache, and it is why a slot count above two
is only useful once clone disks live on `cipool`.

vCPU is deliberately overcommitted, but the ceiling is real: the host is a
mobile i9-13900H with 14 cores / 20 threads. Three 16-vCPU slots is 2.4×;
a fourth slot should drop per-slot vCPU (~12) rather than hold 16, since
adding slots adds queueing capacity, not CPU.

A PR event fires at most three pool jobs (ubuntu, coverage, windows); msi — if
routed — queues briefly behind the Windows bazel leg. A second Windows slot is
gated on #872, not on capacity.

**This host has headroom, and that is the argument against buying another
one.** Over a day at 2-minute resolution the host's CPU sits at p50 2.8% /
p90 18.2%, and above 80% for 12 of 1440 samples — 0.8% of the day. Over four
days the Linux slot pair was **completely idle 93% of the time** and both
slots busy 1.0%; the Windows pair, 91% and 0.7%. Pool queue time is 2s at the
median. The one number near a wall is RAM: 78.5 GiB peak of 94 GiB, which is
four 16 GiB slots plus a 16 GiB ARC — so a *fifth* slot does not fit without
cutting ARC, and nothing measured here asks for one. R5 adds roughly one
Linux-slot job per PR, which the 93% idle absorbs.

Re-run this before concluding the host is short of anything: `pvesh get
/nodes/<node>/rrddata --timeframe day` for utilization, and per-job
`created_at`/`started_at` from the Actions API for queue time. Slot occupancy
is the honest capacity metric — CPU averages hide bursts, and a busy *slot*
is what actually makes the next job wait.

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
| macOS leg (until R8) | GitHub-hosted | cloud | as today |

Each OS carries its own kill switch — `RP_POOL_LINUX` and `RP_POOL_WINDOWS`,
joined by `RP_POOL_MACOS` at R8 — because the venues fail independently: a
wedged Windows slot or a stale Windows template should not cost Linux its
speed, and vice versa. Flipping one moves only that OS back to GitHub-hosted
runners.

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

### R8 — Route `bazel / macos-latest` to a Mac mini

The last hosted required check, and — now that Linux and Windows are on the
pool — the one that decides when a PR goes green.

**Why this and not another x86 box.** Across 27 `bazel.yml` runs after the
four-slot cutover, **macOS was the long pole in 25 of them (93%)**:

| leg | venue | queue p50 | run p50 | worst seen | long pole in |
|---|---|---|---|---|---|
| `bazel / ubuntu-latest` | pool | 2s | 190s | 721s | 1/27 |
| `bazel / windows-latest` | pool | 2s | 104s | 743s | 1/27 |
| `bazel / macos-latest` | hosted | 3s | **334s** | **1888s run, 1056s queue** | **25/27** |

The pool legs finish in a third of the macOS leg's median and queue for two
seconds. Spending on x86 buys idle slots (see the host-capacity section);
spending on a Mac attacks the leg that is actually holding PRs open, and it
is the only remaining lever on #765 — see the wedge note below.

Four phases, mirroring R1–R4. The security contract is unchanged: every
ADR-020 layer must hold on the new host before the leg is routed.

**R8a — measure before buying.** GitHub's hosted `macos-latest` gives public
repos a 3-core M1; a base M4 mini is 10 cores. That should be decisive, but
"should" is not a measurement and this project's rule is to measure first.
Borrow or rent an Apple Silicon machine for an afternoon, run the three
Bazel steps against the LAN cache, and compare with the 334s median above.
Acceptance: the whole job beats hosted by enough to matter after a
cold-cache first run — the same bar R2 and R4 cleared. If it does not, stop
here; the rest of this phase is wasted money.

**R8b — host and template.** macOS guests may only be virtualized on Apple
hardware, so the Mac is a second hypervisor rather than another Proxmox
node. [Tart](https://tart.run/) (Virtualization.framework) is the analogue
of `qm`: it clones, boots, and destroys macOS VMs, and it is what the
ephemeral-macOS-runner projects are built on.

* **Two slots, hard ceiling.** Apple's licence permits two macOS VMs per
  host and the framework enforces it. That matches the measured peak — the
  Linux and Windows pairs hit both-busy 1% of the time — so one mini is
  enough, and a third concurrent macOS job is not a tuning decision but a
  second machine.
* **The template supplies what the hosted image supplies.** Same rule that
  broke this venue's Windows sibling on its first real job: bazelisk at the
  pinned version, OmniSim, Pebble, `libusb` (currently `brew install`ed
  *ungated* on every macOS run — R8 must add the
  `runner.environment == 'github-hosted'` guard the other install steps
  carry), the QHYCCD SDK, and the ZWO SDK with `LIBCLANG_PATH` and
  `ZWO_SDK_LIB_DIR`. Extend `proxmox-runner-test.yml` with a macOS
  template-parity job and dispatch it after every template rebuild.
* **The QHY SDK needs the Windows treatment.** On macOS `build.rs` resolves
  `QHYCCD_SDK_DIR` before falling back to `$GITHUB_WORKSPACE/sdk_mac_arm_*`,
  exactly as on Windows — but `.bazelrc` forwards that variable only under
  `build:windows`. A machine-wide SDK on the template is therefore invisible
  to build actions until `build:macos --action_env=QHYCCD_SDK_DIR` is added
  alongside it. Unset on hosted runners, so their action keys do not move.
* **`GITHUB_WORKSPACE` is in the macOS action env too** (`build:macos
  --action_env=GITHUB_WORKSPACE`), so macOS inherits the Windows cache-key
  coupling: change the runner's `_work` layout and every macOS action goes
  cold at once, with no error to explain it. Pin the layout at template
  build time and treat it as load-bearing.

**R8c — orchestrator.** `rp-runner-pool.sh` calls `qm` directly throughout,
so it does not merely need new `SLOTS` entries — it needs a seam. Factor the
five hypervisor operations the slot loop performs (clone, start, wait-for-
agent, inject `.jitconfig`, destroy, plus the `qm status` probe) behind a
per-slot driver, keep the existing calls as the `qm` implementation, and add
a `tart` one. Everything above that seam — JIT minting, the runner-liveness
reclaim, the marker lifecycle, the registration DELETE on teardown — is
hypervisor-agnostic and must stay shared; those are the parts that took
eleven review rounds to get right, and a forked second script would rot.

Two decisions this phase must make explicitly, because both touch ADR-020:

* **Where the orchestrator runs.** On the Mac, as a second instance of the
  same script. The alternative — pve driving `tart` over SSH — reintroduces
  exactly what the Proxmox design avoided by using the guest agent: a
  network path in the pool's control plane, which the guest VLAN fencing
  could then break. The cost is that the PAT now lives on two hosts. That
  generalizes layer 4 from "the hypervisor" to "each pool hypervisor,
  root-only, never inside a guest" rather than weakening it, but it is an
  **ADR-020 amendment and must be written as one**, not assumed.
* **How the guests are fenced (layer 5).** The Mac has one NIC and is itself
  the control plane, so the Proxmox arrangement — tagged vNIC onto VLAN 67,
  control over a network-free guest agent — does not port directly. Two
  candidates: a VLAN sub-interface with bridged guests, or
  [softnet](https://github.com/cirruslabs/softnet), Tart's userspace packet
  filter. Whichever is chosen, the acceptance test is the one R1 used —
  probe from inside a clone and confirm GitHub and the cache answer while
  every other RFC1918 address does not, expecting timeouts rather than
  refusals if the policy drops.

**R8d — route it.** A third `runs-on` branch and a third kill switch,
`RP_POOL_MACOS`. The trusted-event test is duplicated again, verbatim: it
cannot be factored out (`runs-on` is evaluated before the job exists) and
the copies are a security boundary. Fork PRs and the nightly schedule stay
hosted, as on the other two venues.

**What this does and does not do for #765.** The `build:macos` remote-cache
settings in `.bazelrc` — `--remote_max_connections=0`,
`--remote_download_outputs=minimal`, `--nobuild_runfile_links` — are
mitigations for a wedge whose connection axis is exhausted. They must stay:
fork PRs and the schedule keep running hosted against the cloud cache over
the WAN, which is where the wedge lives. But a LAN cache is a different
failure surface, so re-measure them on the pool venue and split a
`build:macos-pool` config if they prove to be pure WAN workarounds there.
Be honest about the claim: **R8 does not fix #765.** What it changes is that
a wedged macOS build becomes a machine on the operator's desk that can be
inspected while it is wedged, which a hosted runner never allows — and that
is the diagnostic the issue says it has run out of.

## References

- [ADR-020](../decisions/020-ephemeral-self-hosted-runners-for-pr-checks.md)
  — the security contract this plan implements
- [Proxmox runner pool skill](../skills/proxmox-runner-pool.md) — pool
  architecture, ops, template rebuild procedure
- [Raspberry Pi runner skill](../skills/raspberry-pi-runner.md) — the
  unchanged rule for persistent runners
