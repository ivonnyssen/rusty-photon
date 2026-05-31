#!/usr/bin/env bash
# CI hang diagnostic for bazel runs (used by bazel-coverage.yml and bazel.yml,
# Linux only). Launch it backgrounded right before a possibly-hanging
# `bazel ...` invocation; the caller traps EXIT to kill it.
#
# Why this exists: `bazel --profile` only records COMPLETED actions, so a
# hung-then-killed action (e.g. //services/rp:bdd's post-test coverage
# collection, or a wedged OmniSim BDD scenario) is INVISIBLE in the profile.
# This streams a snapshot of the machine every HANG_SAMPLER_INTERVAL seconds to
# the LIVE CI log, so the trail survives even a job/timeout cancel. It shows
# what is actually burning the wall-clock during a stall — the running
# subprocess (llvm-profdata / llvm-cov / java CoverageOutputGenerator / a test
# binary / ascom.alpaca / linux-sandbox), %CPU, load average, memory/swap — and
# in particular distinguishes COMPUTE (a process pegged near 100% CPU, high
# load) from a WAIT (everything idle, load ~0 => a lock / network / deadlock).
set -u

INTERVAL="${HANG_SAMPLER_INTERVAL:-30}"
relevant='bazel|llvm-profdata|llvm-cov|profdata|genhtml|lcov|CoverageOutput|java|process-wrapper|linux-sandbox|ascom\.alpaca|sky-survey|calibrator|cucumber'

while :; do
  ts="$(date -u +%H:%M:%SZ)"
  mem="$(free -m 2>/dev/null | awk '/^Mem:/{m=$3"/"$2"MB"} /^Swap:/{s=$3"/"$2"MB"} END{printf "mem=%s swap=%s", m, s}')"
  load="$(cut -d' ' -f1-3 /proc/loadavg 2>/dev/null)"
  echo "::group::[hang-sampler $ts] ${mem} load=${load}"
  echo "-- top 6 by CPU (compute vs idle) --"
  ps -eo pcpu,etimes,rss,comm,args --sort=-pcpu --no-headers 2>/dev/null | head -6
  echo "-- bazel/test/coverage procs by elapsed --"
  ps -eo etimes,pcpu,rss,comm,args --sort=-etimes --no-headers 2>/dev/null \
    | grep -aiE "${relevant}" | grep -avE 'hang-sampler|grep -aiE' | head -8
  echo "::endgroup::"
  sleep "${INTERVAL}"
done
