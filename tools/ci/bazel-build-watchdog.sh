#!/usr/bin/env bash
# Inactivity watchdog around `bazel build` for the macOS CI leg (bazel.yml).
#
# Failure mode this exists for: the build reaches the remote-cache bulk
# download tail ("Downloading <bin>; ... (N actions, 0 running)") and every
# in-flight transfer wedges at once inside Bazel's HTTP cache client. No bytes
# move, so no UI events fire and the step goes completely silent;
# --remote_timeout does not catch it because it bounds an established RPC, not
# a wedged connection pool / event loop (bazelbuild/bazel#11782, #25484).
#
# A healthy build emits progress output at least every ~60s even while a
# single long action runs, so prolonged total silence is a reliable hang
# signal — and, unlike a wall-clock cap, it can never kill a legitimate
# cold-cache build that is merely slow.
#
# On a stall this script:
#   1. captures diagnostics from the server JVM while it is still hung:
#      thread stacks (jstack, plus SIGQUIT -> java.log, which works even
#      where the jstack attach API fails) and the TCP queues toward the
#      remote cache (Recv-Q piled high = the JVM stopped reading; all zero =
#      the network black-holed the connections);
#   2. SIGKILLs the server — `bazel shutdown` would queue an RPC behind the
#      wedged build command and can itself hang;
#   3. re-runs the command once on a fresh server. The on-disk action cache
#      in the output base survives the kill, so the retry restarts warm.
# A genuine build failure exits straight through with its own exit code;
# only output silence triggers the retry.
#
# Usage: bazel-build-watchdog.sh <command> [args...]
# Tunables: WATCHDOG_STALL_SECS (default 300), WATCHDOG_POLL_SECS (default 15).

set -uo pipefail

if (($# == 0)); then
  echo "usage: ${0##*/} <command> [args...]" >&2
  exit 64
fi

STALL_SECS="${WATCHDOG_STALL_SECS:-300}"
POLL_SECS="${WATCHDOG_POLL_SECS:-15}"
# A malformed override (e.g. "300s") would make the stall arithmetic error on
# every poll and the watchdog silently never fire — the exact failure mode
# this script exists to prevent. Fail loudly up front instead.
if ! [[ "$STALL_SECS" =~ ^[1-9][0-9]*$ && "$POLL_SECS" =~ ^[1-9][0-9]*$ ]]; then
  echo "${0##*/}: WATCHDOG_STALL_SECS/WATCHDOG_POLL_SECS must be positive integers (got '$STALL_SECS' / '$POLL_SECS')" >&2
  exit 64
fi
LOG="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/bazel-watchdog-output.log"

# mtime that works with both GNU and BSD (macOS) stat, so the script stays
# testable on Linux. GNU first: BSD stat rejects -c outright, while GNU stat
# would misread -f %m as a filesystem query and print the mount point.
mtime() { stat -c %Y "$1" 2>/dev/null || stat -f %m "$1" 2>/dev/null || echo 0; }

dump_hung_server() {
  echo "::group::bazel-build-watchdog: no output for ${STALL_SECS}s — dumping hung Bazel server"
  pgrep -fl 'bazel|java' 2>/dev/null || true
  echo "--- TCP connections to port 443 (remote cache) ---"
  netstat -an 2>/dev/null | awk 'NR <= 2 || $0 ~ /[.:]443([^0-9]|$)/' || true
  # `bazel info output_base` would queue behind the wedged build command, so
  # locate the server through the on-disk pid files instead. The output base
  # directory is named md5(<workspace root>) — the directory holding
  # MODULE.bazel / WORKSPACE, found by walking up like Bazel does, so this
  # works from a subdirectory too — which pins this to OUR server, never
  # another workspace's. The globbed roots cover the GitHub macOS runner
  # location, the macOS default, and Linux.
  local ws_root ws_hash pidfile server_pid output_base
  ws_root="$PWD"
  while [[ "$ws_root" != "/" ]] &&
    ! [[ -e "$ws_root/MODULE.bazel" || -e "$ws_root/WORKSPACE.bazel" || -e "$ws_root/WORKSPACE" ]]; do
    ws_root="$(dirname "$ws_root")"
  done
  if [[ "$ws_root" == "/" ]]; then
    echo "(no MODULE.bazel/WORKSPACE at or above $PWD — cannot locate the server's output base)"
    echo "::endgroup::"
    return
  fi
  if command -v md5 >/dev/null 2>&1; then
    ws_hash="$(md5 -q -s "$ws_root")"
  else
    ws_hash="$(printf '%s' "$ws_root" | md5sum | cut -d' ' -f1)"
  fi
  for pidfile in "$HOME"/Library/Caches/bazel/_bazel_*/"$ws_hash"/server/server.pid.txt \
    /private/var/tmp/_bazel_*/"$ws_hash"/server/server.pid.txt \
    "$HOME"/.cache/bazel/_bazel_*/"$ws_hash"/server/server.pid.txt; do
    [[ -f "$pidfile" ]] || continue
    server_pid="$(<"$pidfile")"
    kill -0 "$server_pid" 2>/dev/null || continue
    output_base="$(dirname "$(dirname "$pidfile")")"
    # Guard against a stale pidfile whose pid the OS has recycled: the server's
    # startup args carry its own output base, so only touch a process whose
    # command line names exactly this one (-ww: unlimited width, no truncation).
    ps -ww -o command= -p "$server_pid" 2>/dev/null |
      grep -Fq -- "--output_base=$output_base" || continue
    echo "--- bazel server pid ${server_pid}, output base ${output_base} ---"
    if command -v jstack >/dev/null 2>&1; then
      jstack "$server_pid" 2>&1 || echo "(jstack could not attach)"
    fi
    kill -QUIT "$server_pid" 2>/dev/null || true
    sleep 3
    echo "--- tail of ${output_base}/java.log ---"
    tail -n 1000 "$output_base/java.log" 2>/dev/null || true
    kill -KILL "$server_pid" 2>/dev/null || true
  done
  echo "::endgroup::"
}

: >"$LOG"
"$@" > >(tee -a "$LOG") 2>&1 &
cmd_pid=$!

stalled=0
while kill -0 "$cmd_pid" 2>/dev/null; do
  sleep "$POLL_SECS"
  if (($(date +%s) - $(mtime "$LOG") > STALL_SECS)); then
    stalled=1
    dump_hung_server
    # Kill the client and any children it spawned, so nothing lingers holding
    # the step's output pipe open.
    pkill -TERM -P "$cmd_pid" 2>/dev/null || true
    kill -TERM "$cmd_pid" 2>/dev/null || true
    break
  fi
done

wait "$cmd_pid"
status=$?

if ((stalled)); then
  echo "bazel-build-watchdog: build stalled; server killed, retrying once on a fresh server"
  exec "$@"
fi
exit "$status"
