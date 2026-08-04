#!/bin/bash
# Runs exactly ONE GitHub Actions job on an ephemeral Linux pool clone, then
# powers the VM off.
#
# Installed in the Linux runner template at /home/ci/run-one-job.sh and started
# by the `gha-runner` systemd unit; the pool orchestrator on the hypervisor
# (tools/ci/rp-runner-pool.sh) injects .jitconfig through the QEMU guest agent.
# Output goes to the unit's journal (`journalctl -u gha-runner` in the guest).
#
# This file is the source of truth for what the template contains: rebuilding a
# template means copying this in, not editing the guest's copy in place.
cd /home/ci/actions-runner || exit 1

# Monotonic seconds since boot. Deliberately NOT wall clock: a linked clone
# inherits the template's RTC and boots badly out of date, so time sync steps
# the clock a long way forward shortly after boot — straight past any end time
# computed from `date`. /proc/uptime is immune to that correction.
uptime_secs() {
  local up _
  read -r up _ </proc/uptime
  printf %s "${up%.*}"
}

# Bounded wait for the injected config, matching the Windows guest script.
#
# This is the guest's own backstop, not the primary defence: the orchestrator
# reclaims a slot whose runner never connects, and destroys a clone that has no
# injection marker. Both of those need the orchestrator to be running, which is
# exactly what this covers — if it is stopped, or the host is mid-upgrade, a
# clone that never receives a config would otherwise sit powered on forever,
# holding host memory with nothing able to notice.
deadline=$(($(uptime_secs) + 30 * 60))
while [ ! -s .jitconfig ]; do
  if [ "$(uptime_secs)" -ge "$deadline" ]; then
    echo "$(date -Is) no jitconfig within 30 minutes; powering off"
    sudo systemctl poweroff
    exit 0
  fi
  sleep 2
done

JIT="$(cat .jitconfig)"
# Single use: remove it before running, so a crash-restart of this unit can
# never re-register with a spent config.
rm -f .jitconfig
echo "$(date -Is) starting one job"
./run.sh --jitconfig "$JIT"
# Captured before the next echo: the command substitution in it would otherwise
# overwrite $? with `date`'s exit status.
rc=$?
echo "$(date -Is) job finished (exit $rc); powering off"
sudo systemctl poweroff
