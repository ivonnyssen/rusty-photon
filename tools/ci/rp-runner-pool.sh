#!/bin/bash
# Ephemeral GitHub Actions runner pool for a Proxmox VE host.
#
# Maintains one warm runner clone per POOL SLOT:
#   linked-clone the template -> boot -> mint a single-use JIT runner config
#   via the GitHub API -> inject it through the QEMU guest agent -> the
#   in-guest one-job runner runs exactly one job and powers off -> the
#   clone is destroyed -> repeat.
#
# Each slot runs that loop independently and concurrently, so the pool can
# serve several queued jobs at once. Slots are declared in SLOTS below; two
# slots sharing a label set are interchangeable, which is how the Linux pool
# serves bazel.yml and bazel-coverage.yml (both fire on the same PR event)
# without one queueing behind the other.
#
# Deployment (on the Proxmox host, as root — see
# docs/skills/proxmox-runner-pool.md):
#   install -m 755 rp-runner-pool.sh /usr/local/sbin/
#   put a fine-grained PAT in /etc/rp-runner/github-token (chmod 600); resource
#   owner: the rusty-photon org, sole permission "Self-hosted runners: Read
#   and write" (organization permission) — runner registration and nothing
#   else, which is why runners register at org level rather than repo level
#   (repo-level registration would require the far broader Administration
#   permission)
#   run under a systemd unit with Restart=always
#
# Security properties this loop preserves:
#   * the PAT lives only on the hypervisor, never inside any VM;
#   * each VM receives exactly one single-use JIT config: a compromised job
#     cannot mint further registrations;
#   * every job runs on a fresh linked clone; nothing persists between jobs
#     except the shared remote build cache, whose writes are separately
#     credential-gated.
# pipefail so the injection check below really does gate on BOTH `qm guest
# exec` and the in-guest exitcode it reports: the exit status of a pipeline is
# otherwise its last command's, so a `qm` failure that still emitted
# `exitcode: 0` JSON would read as a successful injection and wedge the slot.
# The script's other pipelines feed emptiness checks or string comparisons, so
# this changes nothing for them.
set -u -o pipefail

ORG=rusty-photon
TOKEN_FILE=/etc/rp-runner/github-token

# Per-clone "this one received its config" markers, used to tell an in-flight
# job from an orphan when this service restarts. tmpfs on purpose: a host
# reboot clears it, and a host reboot also takes the clones with it.
STATE_DIR=/run/rp-runner-pool
mkdir -p "$STATE_DIR"

# Pool slots: name|template VMID|clone VMID|guest OS|labels
#
# Clone VMIDs must be unique and must not collide with any other VM on the
# host. Guest OS selects the jitconfig injection path (the guests differ in
# shell and runner directory, nothing else). Sizing note: every slot keeps one
# clone powered on at all times, so the host must hold the sum of their
# memory — see the capacity section of docs/plans/proxmox-pr-routing.md.
SLOTS=(
  "runner-eph|903|9100|linux|[\"self-hosted\",\"Linux\",\"X64\",\"proxmox-ephemeral\"]"
  "runner-eph2|903|9101|linux|[\"self-hosted\",\"Linux\",\"X64\",\"proxmox-ephemeral\"]"
  "runner-win|904|9200|windows|[\"self-hosted\",\"Windows\",\"X64\",\"proxmox-ephemeral-windows\"]"
)

# Free-plan orgs have exactly one (default) runner group, but resolve its id
# rather than assuming 1 so a plan change can't silently break registration.
GROUP_ID=$(curl -fsS \
  -H @<(printf 'Authorization: Bearer %s' "$(cat $TOKEN_FILE)") \
  -H "Accept: application/vnd.github+json" \
  "https://api.github.com/orgs/$ORG/actions/runner-groups" \
  | python3 -c 'import json,sys; gs=json.load(sys.stdin)["runner_groups"]; print(next(g["id"] for g in gs if g["default"]))')
if [ -z "${GROUP_ID:-}" ]; then
  echo "cannot resolve the default runner group — token invalid or lacking the org Self-hosted runners permission" >&2
  exit 1
fi

log() { echo "$(date -Is) [$1] ${*:2}"; }

# Write the JIT config into the guest. Both variants write a temp file and
# rename it, because the in-guest runner polls for a NON-EMPTY .jitconfig and
# must never read a partial write. A JIT config is base64, so single-quoting
# it in the PowerShell variant is safe.
inject_jitconfig() {
  local vmid=$1 os=$2 jit=$3
  case "$os" in
    linux)
      qm guest exec "$vmid" -- /bin/bash -c "printf %s \"$jit\" > /home/ci/actions-runner/.jitconfig.tmp && chown ci:ci /home/ci/actions-runner/.jitconfig.tmp && mv /home/ci/actions-runner/.jitconfig.tmp /home/ci/actions-runner/.jitconfig"
      ;;
    windows)
      # PowerShell exits 0 even when a cmdlet raises a non-terminating error,
      # so the caller's exitcode check alone would accept a failed write and
      # then deadlock waiting for a job that can never start. Force errors to
      # terminate, confirm the landed file is non-empty, and exit explicitly.
      qm guest exec "$vmid" -- powershell.exe -NoProfile -NonInteractive -Command "\$ErrorActionPreference='Stop'; try { Set-Content -Path 'C:\\actions-runner\\.jitconfig.tmp' -Value '$jit' -NoNewline -Encoding ascii; Move-Item -Force 'C:\\actions-runner\\.jitconfig.tmp' 'C:\\actions-runner\\.jitconfig'; if ((Get-Item 'C:\\actions-runner\\.jitconfig').Length -le 0) { exit 1 }; exit 0 } catch { exit 1 }"
      ;;
    *)
      echo "unknown guest os '$os'" >&2
      return 1
      ;;
  esac
}

destroy_clone() {
  qm stop "$1" >/dev/null 2>&1
  qm destroy "$1" --purge >/dev/null 2>&1
  rm -f "$STATE_DIR/$1.injected"
}

slot_loop() {
  local name=$1 template=$2 vmid=$3 os=$4 labels=$5

  while true; do
    # Establish the invariant the rest of the iteration depends on: either the
    # VM does not exist, or it exists AND holds a real job. A clone with no
    # injection marker was created but never configured, and nothing will ever
    # shut it down — the guest waits for a config that cannot arrive (the
    # Linux runner script has no no-config timeout at all) while the poweroff
    # wait below waits for a shutdown that never comes, wedging the slot
    # permanently. Two ways to reach that state: this service restarting
    # mid-window, and a destroy that did not take (a Proxmox lock, say), which
    # is why the check runs every iteration rather than once at startup. A
    # clone WITH a marker is left alone: an orchestrator restart must never
    # abort an in-flight job.
    if qm status "$vmid" >/dev/null 2>&1 && [ ! -e "$STATE_DIR/$vmid.injected" ]; then
      log "$name" "clone $vmid exists but never received a config; destroying"
      destroy_clone "$vmid"
      # If the destroy did not take, do not fall through into the poweroff
      # wait — retry the reconcile instead, so a transient lock resolves.
      if qm status "$vmid" >/dev/null 2>&1; then
        log "$name" "clone $vmid still present after destroy; retrying"
        sleep 30
        continue
      fi
    fi

    if ! qm status "$vmid" >/dev/null 2>&1; then
      qm clone "$template" "$vmid" --name "$name" >/dev/null || { sleep 30; continue; }
      qm start "$vmid" >/dev/null

      # Windows clones take appreciably longer than Linux to reach a
      # responding guest agent, so the wait is generous rather than tuned.
      booted=0
      for _ in $(seq 1 60); do
        qm agent "$vmid" ping >/dev/null 2>&1 && { booted=1; break; }
        sleep 5
      done
      if [ $booted != 1 ]; then
        log "$name" "clone $vmid never reached the guest agent; destroying"
        destroy_clone "$vmid"
        sleep 30
        continue
      fi

      # The auth header arrives via process substitution (bash printf is a
      # builtin), so the PAT never appears on any process command line.
      JIT=$(curl -fsS -X POST \
        -H @<(printf 'Authorization: Bearer %s' "$(cat $TOKEN_FILE)") \
        -H "Accept: application/vnd.github+json" \
        "https://api.github.com/orgs/$ORG/actions/runners/generate-jitconfig" \
        -d "{\"name\":\"$name-$(date +%s)\",\"runner_group_id\":$GROUP_ID,\"labels\":$labels,\"work_folder\":\"_work\"}" \
        | python3 -c 'import json,sys; print(json.load(sys.stdin)["encoded_jit_config"])')
      if [ -z "${JIT:-}" ]; then
        log "$name" "jitconfig mint failed; destroying $vmid"
        destroy_clone "$vmid"
        sleep 60
        continue
      fi

      # An unverified injection would deadlock this loop — the guest waits for
      # a config that never arrives while this loop waits for a poweroff that
      # never comes — so check both qm's own exit and the in-guest exitcode.
      if ! inject_jitconfig "$vmid" "$os" "$JIT" \
          | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("exitcode") == 0 else 1)'; then
        log "$name" "jitconfig injection into $vmid failed; destroying"
        destroy_clone "$vmid"
        sleep 30
        continue
      fi
      # Only now is the clone recoverable across a restart of this service.
      #
      # Ordering is deliberate, and this is the safe direction. Dying in the
      # sliver between a successful injection and this line loses a clone that
      # was about to run a job — one aborted job, and the pool immediately
      # rebuilds the slot. Writing the marker FIRST would instead lose a clone
      # that never got a config, which nothing recovers: the guest waits
      # forever for a config that will not come and this loop waits forever
      # for its poweroff. A transient, self-healing failure beats a permanent
      # one. Note also that the guest deletes .jitconfig the moment it reads
      # it (~2 s), so the file's presence cannot be used to detect a running
      # job — proving liveness needs the runner's state from the GitHub API,
      # tracked in the pool health-check issue.
      : > "$STATE_DIR/$vmid.injected"
      log "$name" "runner clone $vmid up and registered"
    fi

    # The clone powers itself off after its single job. This wait is
    # deliberately unbounded: a registered clone with no job yet assigned is
    # not stalled, it is WARM — sitting here ready to be picked is the whole
    # point of the pool, and a wall-clock cap would periodically destroy
    # healthy idle runners and put cold-start latency back on every job. A
    # genuinely wedged guest (hung shutdown, dead runner process) does hold a
    # slot, but distinguishing that from idle needs a health check rather than
    # a timer — tracked separately.
    # Only a confirmed `stopped` ends the wait. An unreadable status is NOT
    # "stopped": stderr is suppressed here, so a transient failure (host under
    # load, a lock) yields an empty string, and treating that as stopped would
    # destroy a VM still running a job. Unknown therefore means keep waiting —
    # but not forever, since a VM removed out of band would never report
    # anything again; after a few minutes of silence give up on the wait and
    # let the next iteration's reconcile decide.
    unknown=0
    while true; do
      state=$(qm status "$vmid" 2>/dev/null | awk '{print $2}')
      [ "$state" = stopped ] && break
      if [ -z "$state" ]; then
        unknown=$((unknown + 1))
        if [ "$unknown" -ge 30 ]; then
          log "$name" "status of $vmid unreadable for 5 minutes; abandoning the wait"
          break
        fi
      else
        unknown=0
      fi
      sleep 10
    done
    log "$name" "runner clone $vmid finished; destroying"
    destroy_clone "$vmid"
  done
}

# One background loop per slot. Killing the service kills the loops but not
# the clones; each slot reconciles its own leftover on the next start —
# waiting on one that was already running a job, destroying one that never got
# a config (see slot_loop).
for slot in "${SLOTS[@]}"; do
  IFS='|' read -r s_name s_template s_vmid s_os s_labels <<<"$slot"
  log "$s_name" "starting slot (template $s_template, clone $s_vmid, $s_os)"
  slot_loop "$s_name" "$s_template" "$s_vmid" "$s_os" "$s_labels" &
done
wait
