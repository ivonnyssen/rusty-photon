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
# A second failure mode shares the same phase of the build: instead of going
# silent, the client's gRPC command stream to the server resets ("Server
# terminated abruptly … recvmsg:Connection reset by peer", exit 37) while the
# server JVM survives as an orphan. The decisive evidence — a Java exception
# or internal OOM on the server side — lands only in the output base's
# server/jvm.out and java.log, which evaporate with the ephemeral runner. So
# on any infrastructure-class bazel exit (code >= 32) this script echoes both
# into the step log before propagating the exit code. (A first capture showed
# jvm.out clean on the abrupt-termination crash, so java.log — where the
# server's logger records event-loop deaths and command aborts — is included.)
# Every post-mortem of that crash shows the same picture: the server JVM
# alive and healthy, idle after cleanly tearing down the interrupted command
# — only the loopback stream died. So exit 37 specifically is retried once
# after the dump: the new client reconnects to the surviving server and
# resumes warm, with the in-memory analysis graph intact. Other
# infrastructure exits stay fatal — none of them carries evidence that the
# server is reusable.
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

# The output base directory is named md5(<workspace root>) — the directory
# holding MODULE.bazel / WORKSPACE, found by walking up like Bazel does, so
# this works from a subdirectory too — which pins every dump below to OUR
# server, never another workspace's. Prints the hash; fails if no workspace
# marker exists at or above $PWD. (`bazel info output_base` is not an option:
# it would queue an RPC behind a wedged or dead build command.)
workspace_hash() {
  local ws_root="$PWD"
  while [[ "$ws_root" != "/" ]] &&
    ! [[ -e "$ws_root/MODULE.bazel" || -e "$ws_root/WORKSPACE.bazel" || -e "$ws_root/WORKSPACE" ]]; do
    ws_root="$(dirname "$ws_root")"
  done
  if [[ "$ws_root" == "/" ]]; then
    echo "bazel-build-watchdog: no MODULE.bazel/WORKSPACE.bazel/WORKSPACE at or above $PWD" >&2
    return 1
  fi
  if command -v md5 >/dev/null 2>&1; then
    md5 -q -s "$ws_root"
  elif command -v md5sum >/dev/null 2>&1; then
    printf '%s' "$ws_root" | md5sum | cut -d' ' -f1
  else
    echo "bazel-build-watchdog: neither md5 nor md5sum available to hash the workspace path" >&2
    return 1
  fi
}

# Defense-in-depth for everything tailed into the step log: CI hands the
# remote-cache credential to bazel via --remote_header="Authorization=Bearer
# <token>", and the server may echo received options into its logs. GitHub
# masks the registered secret value itself, but nothing guarantees that is
# the only place the token appears. POSIX ERE only ([Aa]/[Bb] casings, no
# GNU-sed /I) so BSD sed on the macOS runners accepts it.
redact_bearer() {
  sed -E 's/([Aa]uthorization[=:][[:space:]]*[Bb]earer[[:space:]]+)[^[:space:]]+/\1[REDACTED]/g'
}

# Print the live server pid for an output base, if any: pidfile present, pid
# alive, and its command line naming exactly this output base (guards against
# a stale pidfile whose pid the OS has recycled; -ww: unlimited width, no
# truncation).
server_pid_for_base() {
  local output_base="$1" pid pidfile="$1/server/server.pid.txt"
  [[ -f "$pidfile" ]] || return 1
  pid="$(<"$pidfile")"
  kill -0 "$pid" 2>/dev/null || return 1
  ps -ww -o command= -p "$pid" 2>/dev/null |
    grep -Fq -- "--output_base=$output_base" || return 1
  printf '%s' "$pid"
}

# Echo the server's post-mortem evidence into the step log: the JVM's raw
# stderr/stdout capture (server/jvm.out — fatal exceptions, OOM, crash
# banners) and the server's own log (java.log — where a netty/event-loop
# death or command abort lands without ever touching the JVM's stderr).
# Works whether the server is alive, orphaned, or gone — it only needs the
# files, located by workspace hash under the globbed roots (GitHub macOS
# runner location, macOS default, Linux).
dump_server_logs() {
  local reason="$1" ws_hash dir server_pid printed=0
  if ! ws_hash="$(workspace_hash)"; then
    echo "bazel-build-watchdog: ${reason} — cannot locate the server's output base (cause above); skipping server-log dump"
    return
  fi
  for dir in "$HOME"/Library/Caches/bazel/_bazel_*/"$ws_hash" \
    /private/var/tmp/_bazel_*/"$ws_hash" \
    "$HOME"/.cache/bazel/_bazel_*/"$ws_hash"; do
    [[ -d "$dir/server" ]] || continue
    printed=1
    echo "::group::bazel-build-watchdog: ${reason} — server logs under ${dir}"
    # The abrupt-termination crash leaves the server alive as an orphan with
    # its log tail still buffered (a first capture showed java.log ending
    # mid-line, minutes stale). SIGQUIT is non-destructive: it makes the JVM
    # dump its thread stacks and flush, turning the tails below into actual
    # evidence of the surviving server's state.
    if server_pid="$(server_pid_for_base "$dir")"; then
      echo "--- server pid ${server_pid} still alive; SIGQUIT for thread dump + log flush ---"
      kill -QUIT "$server_pid" 2>/dev/null || true
      sleep 3
    fi
    echo "--- tail of ${dir}/server/jvm.out ---"
    tail -n 1000 "$dir/server/jvm.out" 2>/dev/null | redact_bearer || true
    echo "--- tail of ${dir}/java.log ---"
    tail -n 1000 "$dir/java.log" 2>/dev/null | redact_bearer || true
    echo "::endgroup::"
  done
  ((printed)) || echo "bazel-build-watchdog: ${reason} — no server directory found for workspace hash ${ws_hash}"
}

dump_hung_server() {
  echo "::group::bazel-build-watchdog: no output for ${STALL_SECS}s — dumping hung Bazel server"
  # The client's own command line carries --remote_header with the cache
  # credential, and -fl prints full command lines.
  pgrep -fl 'bazel|java' 2>/dev/null | redact_bearer || true
  echo "--- TCP connections to port 443 (remote cache) ---"
  netstat -an 2>/dev/null | awk 'NR <= 2 || $0 ~ /[.:]443([^0-9]|$)/' || true
  # `bazel info output_base` would queue behind the wedged build command, so
  # locate the server through the on-disk pid files instead.
  local ws_hash server_pid output_base
  if ! ws_hash="$(workspace_hash)"; then
    echo "(cannot locate the server's output base — cause above)"
    echo "::endgroup::"
    return
  fi
  for output_base in "$HOME"/Library/Caches/bazel/_bazel_*/"$ws_hash" \
    /private/var/tmp/_bazel_*/"$ws_hash" \
    "$HOME"/.cache/bazel/_bazel_*/"$ws_hash"; do
    server_pid="$(server_pid_for_base "$output_base")" || continue
    echo "--- bazel server pid ${server_pid}, output base ${output_base} ---"
    if command -v jstack >/dev/null 2>&1; then
      jstack "$server_pid" 2>&1 | redact_bearer || echo "(jstack could not attach)"
    fi
    kill -QUIT "$server_pid" 2>/dev/null || true
    sleep 3
    echo "--- tail of ${output_base}/java.log ---"
    tail -n 1000 "$output_base/java.log" 2>/dev/null | redact_bearer || true
    echo "--- tail of ${output_base}/server/jvm.out ---"
    tail -n 1000 "$output_base/server/jvm.out" 2>/dev/null | redact_bearer || true
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
    # the step's output pipe open. Escalate to SIGKILL after a short grace
    # period: a client wedged hard enough to ignore TERM would otherwise park
    # the `wait` below and the retry would never run.
    pkill -TERM -P "$cmd_pid" 2>/dev/null || true
    kill -TERM "$cmd_pid" 2>/dev/null || true
    for _ in 1 2 3 4 5; do
      kill -0 "$cmd_pid" 2>/dev/null || break
      sleep 1
    done
    if kill -0 "$cmd_pid" 2>/dev/null; then
      pkill -KILL -P "$cmd_pid" 2>/dev/null || true
      kill -KILL "$cmd_pid" 2>/dev/null || true
    fi
    break
  fi
done

wait "$cmd_pid"
status=$?

if ((stalled)); then
  echo "bazel-build-watchdog: build stalled; server killed, retrying once on a fresh server"
  "$@"
  status=$?
  if ((status >= 32)); then
    dump_server_logs "retry exited ${status}"
  fi
  exit "$status"
fi

# Bazel reserves exit codes >= 32 for infrastructure failures (36 local
# environment, 37 unhandled exception / "Server terminated abruptly", 38
# transient remote issues); ordinary build/test failures (1/2/3/4/8) stay
# below and get no dump. Shell-level codes for a killed client (126+) land
# in the same bucket, where the extra diagnostics are harmless.
if ((status >= 32)); then
  dump_server_logs "bazel exited ${status}"
fi

# The abrupt-termination crash: the client's loopback gRPC stream dies
# mid-command while the server JVM survives, healthy and idle (see header).
# Reconnecting to that server resumes the build warm, so one retry converts
# a whole-job rerun on a fresh VM into an in-job recovery. The dump above
# already ran, so the evidence is preserved either way; if the server turns
# out unusable after all, the step's timeout-minutes bounds the retry.
if ((status == 37)); then
  echo "bazel-build-watchdog: bazel exited 37 with the server left alive; retrying once against the surviving server"
  "$@"
  status=$?
  if ((status >= 32)); then
    dump_server_logs "retry exited ${status}"
  fi
fi
exit "$status"
