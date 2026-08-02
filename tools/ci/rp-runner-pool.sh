#!/bin/bash
# Ephemeral GitHub Actions runner pool for a Proxmox VE host.
#
# Maintains one warm runner clone at a time:
#   linked-clone the template -> boot -> mint a single-use JIT runner config
#   via the GitHub API -> inject it through the QEMU guest agent -> the
#   in-guest gha-runner service runs exactly one job and powers off -> the
#   clone is destroyed -> repeat.
#
# Deployment (on the Proxmox host, as root — see
# docs/skills/proxmox-runner-pool.md):
#   install -m 755 rp-runner-pool.sh /usr/local/sbin/
#   put a fine-grained PAT in /etc/rp-runner/github-token (chmod 600); scope:
#   this repository only, permission "Administration: Read and write", nothing
#   else
#   run under a systemd unit with Restart=always
#
# Security properties this loop preserves:
#   * the PAT lives only on the hypervisor, never inside any VM;
#   * each VM receives exactly one single-use JIT config: a compromised job
#     cannot mint further registrations;
#   * every job runs on a fresh linked clone; nothing persists between jobs
#     except the shared remote build cache, whose writes are separately
#     credential-gated.
set -u

REPO=ivonnyssen/rusty-photon
TEMPLATE=902
VMID=9100
NAME=runner-eph
TOKEN_FILE=/etc/rp-runner/github-token
LABELS='["self-hosted","Linux","X64","proxmox-ephemeral"]'

while true; do
  if ! qm status $VMID >/dev/null 2>&1; then
    qm clone $TEMPLATE $VMID --name $NAME >/dev/null || { sleep 30; continue; }
    qm start $VMID >/dev/null

    booted=0
    for _ in $(seq 1 60); do
      qm agent $VMID ping >/dev/null 2>&1 && { booted=1; break; }
      sleep 5
    done
    if [ $booted != 1 ]; then
      qm stop $VMID >/dev/null 2>&1
      qm destroy $VMID --purge >/dev/null 2>&1
      sleep 30
      continue
    fi

    JIT=$(curl -fsS -X POST \
      -H "Authorization: Bearer $(cat $TOKEN_FILE)" \
      -H "Accept: application/vnd.github+json" \
      "https://api.github.com/repos/$REPO/actions/runners/generate-jitconfig" \
      -d "{\"name\":\"$NAME-$(date +%s)\",\"runner_group_id\":1,\"labels\":$LABELS,\"work_folder\":\"_work\"}" \
      | python3 -c 'import json,sys; print(json.load(sys.stdin)["encoded_jit_config"])')
    if [ -z "${JIT:-}" ]; then
      qm stop $VMID >/dev/null 2>&1
      qm destroy $VMID --purge >/dev/null 2>&1
      sleep 60
      continue
    fi

    # .tmp + mv: the in-guest service polls for a non-empty .jitconfig, so the
    # rename keeps it from ever reading a partial write.
    qm guest exec $VMID -- /bin/bash -c "printf %s \"$JIT\" > /home/ci/actions-runner/.jitconfig.tmp && chown ci:ci /home/ci/actions-runner/.jitconfig.tmp && mv /home/ci/actions-runner/.jitconfig.tmp /home/ci/actions-runner/.jitconfig" >/dev/null
    echo "$(date -Is) runner clone $VMID up and registered"
  fi

  # The clone powers itself off after its single job.
  while [ "$(qm status $VMID 2>/dev/null | awk '{print $2}')" = running ]; do
    sleep 10
  done
  echo "$(date -Is) runner clone $VMID finished; destroying"
  qm destroy $VMID --purge >/dev/null 2>&1
done
