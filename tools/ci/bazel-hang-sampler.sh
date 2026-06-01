#!/usr/bin/env bash
# CI hang diagnostic for bazel runs (used by bazel-coverage.yml and bazel.yml,
# Linux only). Launch it backgrounded right before a possibly-hanging
# `bazel ...` invocation; the caller traps EXIT to kill it.
#
# Why this exists: `bazel --profile` only records COMPLETED actions, so a
# hung-then-killed action (e.g. //services/rp:bdd's post-scenario teardown, or
# a wedged OmniSim BDD scenario) is INVISIBLE in the profile. This streams a
# snapshot of the machine every HANG_SAMPLER_INTERVAL seconds to the LIVE CI
# log, so the trail survives even a job/timeout cancel.
#
# What the rp:bdd hang looks like (measured 2026-05-31): all 265 scenarios pass
# by ~+423s, then the rp:bdd *action* wall keeps climbing for 20-37 min while
# the machine is near-idle (load 0.00, nothing on CPU but the bazel JVM), then
# it either recovers or hits the 50-min cap. The open question this sampler
# must answer: during that idle park, is the test binary (`bdd-<hash>`) STILL
# ALIVE (hung in its tokio runtime-drop / blocking-pool join — hypothesis B) or
# already GONE (a bazel-server-side post-spawn stall, e.g. a remote-cache RPC —
# hypothesis A)? The previous sampler filtered the process list to a fixed set
# of names and so could see neither the test binary (named `bdd-<hash>`, not in
# the filter) nor an idle process at 0% CPU. This version dumps the FULL
# non-kernel process tree with each process's STATE (`stat`) and kernel
# wait-channel (`wchan` — the function it is blocked in: e.g. `pipe_read`,
# `do_wait`, `futex`, `ep_poll`, `unix_stream_recvmsg`), which names both the
# survivor and the exact syscall it is parked in. That distinguishes:
#   - test binary present, wchan=do_wait/futex   => runtime-drop join (B)
#   - test binary present, wchan=pipe_read/unix* => blocked on a pipe/socket
#   - test binary ABSENT, only bazel java idle   => bazel-side stall (A)
# It distinguishes COMPUTE (a process pegged near 100% CPU, high load) from a
# WAIT (everything idle, load ~0 => a lock / network / deadlock).
set -u

INTERVAL="${HANG_SAMPLER_INTERVAL:-30}"
# Broad enough to catch the test binary (bdd), the .NET simulator
# (ascom.alpaca.simulators / dotnet), and the coverage/sandbox tooling.
relevant='bazel|llvm-profdata|llvm-cov|profdata|genhtml|lcov|CoverageOutput|java|process-wrapper|linux-sandbox|ascom|dotnet|simulator|omnisim|sky-survey|calibrator|cucumber|/bdd|[ /]bdd-|rp_bdd'
# Resolve the remote-cache host once so we can watch the sockets to it during a park.
# H1 (stale pooled keep-alive): a half-open socket is reused, the request write
# blackholes, and the op blocks until --remote_timeout -> the socket shows ESTABLISHED
# with Send-Q stuck non-zero / Recv-Q 0, and the bazel JVM has a remote/netty thread
# parked in LockSupport.park. These two lines name it the next time it parks.
CACHE_HOST="${BAZEL_CACHE_HOST:-cache.rustyphoton.space}"
CACHE_IP="$(getent hosts "$CACHE_HOST" 2>/dev/null | awk '{print $1; exit}')"

while :; do
  ts="$(date -u +%H:%M:%SZ)"
  mem="$(free -m 2>/dev/null | awk '/^Mem:/{m=$3"/"$2"MB"} /^Swap:/{s=$3"/"$2"MB"} END{printf "mem=%s swap=%s", m, s}')"
  load="$(cut -d' ' -f1-3 /proc/loadavg 2>/dev/null)"
  nprocs="$(ps -e --no-headers 2>/dev/null | grep -avcE '\[' || echo '?')"
  echo "::group::[hang-sampler $ts] ${mem} load=${load} userprocs=${nprocs}"
  echo "-- top 6 by CPU (stat/wchan show blocked-in; compute vs idle) --"
  ps -eo pcpu,etimes,rss,stat,wchan:22,comm --sort=-pcpu --no-headers 2>/dev/null | head -6
  echo "-- ALL non-kernel procs by elapsed (pid ppid etimes %cpu stat wchan comm args) --"
  # Drop kernel threads (bracketed comm/args) and the sampler's own ps|grep.
  # wchan is the syscall the process is parked in — decisive for A vs B.
  ps -eo pid,ppid,etimes,pcpu,stat,wchan:22,comm,args --sort=-etimes --no-headers 2>/dev/null \
    | grep -avE '\] ?$|\[[a-z]' \
    | grep -avE 'hang-sampler|--sort=-etimes|grep -avE' \
    | head -28
  echo "-- explicit: any rp:bdd / OmniSim / dotnet survivor? (broad match, with wchan) --"
  ps -eo pid,ppid,etimes,pcpu,stat,wchan:22,comm,args --no-headers 2>/dev/null \
    | grep -aiE "${relevant}" | grep -avE 'hang-sampler|grep -aiE' | head -14
  echo "-- remote-cache sockets to ${CACHE_HOST} (${CACHE_IP:-unresolved}): State Recv-Q Send-Q Local Peer --"
  if [ -n "${CACHE_IP:-}" ]; then
    ss -tan 2>/dev/null | grep -F "${CACHE_IP}" | head -10 || echo "  (none established to cache IP)"
  else
    echo "  (cache host did not resolve)"
  fi
  echo "-- bazel JVM remote/netty threads (jcmd Thread.print, filtered) --"
  bpid="$(pgrep -f 'A-server\.jar' 2>/dev/null | head -1)"
  if [ -n "${bpid:-}" ] && command -v jcmd >/dev/null 2>&1; then
    jcmd "${bpid}" Thread.print 2>/dev/null \
      | grep -iE 'remote|netty|grpc|Downloader|ChannelPool|epollWait|SocketChannel|LockSupport.park' \
      | head -20 || echo "  (no matching threads)"
  else
    echo "  (jcmd unavailable or bazel server pid not found: pid=${bpid:-none})"
  fi
  echo "::endgroup::"
  sleep "${INTERVAL}"
done
