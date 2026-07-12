#!/bin/sh
# rig.sh — ops helper for developing against the telescope field rig (a
# Raspberry Pi running the packaged rusty-photon stack; see
# docs/skills/rig-development.md).
#
# The rig's address is deliberately NOT in this script (public repo): it is
# resolved from the `rig` host alias in your ~/.ssh/config, overridable with
# the RIG_HOST environment variable. The alias must log in as a user with
# passwordless sudo on the rig.
#
# Usage: scripts/rig.sh <command> [args]
#   status                     unit overview of every rusty-photon service
#   logs <svc> [args...]       journalctl for a service; extra args are passed
#                              through (e.g. `logs zwo-focuser -f`, `-n 200`)
#   restart|start|stop <svc>   control a service's systemd unit
#   fetch-configs [dest]       copy the rig's service configs to dest (default
#                              ~/.config/rusty-photon-rig), rewriting loopback
#                              driver endpoints (http://127.0.0.1:PORT) to the
#                              rig's address so a locally-run service (rp,
#                              ui-htmx, sentinel, ...) talks to the rig's
#                              drivers. Serial device paths are left untouched.
#   ssh [cmd...]               interactive shell / one-off command on the rig
#
# <svc> accepts either the short name (zwo-focuser) or the full unit name
# (rusty-photon-zwo-focuser).

set -eu

RIG_HOST="${RIG_HOST:-rig}"
CONFIG_DIR=/var/lib/rusty-photon/.config/rusty-photon

usage() {
    # Print the header comment block (everything from line 2 to the first
    # non-comment line), so the help text can never desync from the header.
    awk 'NR < 2 { next } !/^#/ { exit } { sub(/^# ?/, ""); print }' "$0"
    exit "${1:-0}"
}

unit_name() {
    case "$1" in
        rusty-photon-*) printf '%s' "$1" ;;
        *) printf 'rusty-photon-%s' "$1" ;;
    esac
}

# Single-quote each argument for the remote shell (ssh concatenates its
# command arguments and the remote shell re-parses them, so boundaries and
# metacharacters survive only if we quote here): abc -> 'abc', don't -> 'don'\''t'.
quote_args() {
    for a in "$@"; do
        printf " '%s'" "$(printf '%s' "$a" | sed "s/'/'\\\\''/g")"
    done
}

[ $# -ge 1 ] || usage 1
cmd=$1
shift

case "$cmd" in
    status)
        ssh "$RIG_HOST" "systemctl list-units --type=service --all --no-pager --plain 'rusty-photon-*'"
        ;;
    logs)
        [ $# -ge 1 ] || { echo "logs: service name required" >&2; exit 1; }
        svc=$(unit_name "$1")
        shift
        # -t: keep colors; journalctl needs the adm group or sudo — use sudo
        # since the rig user has it passwordless anyway.
        ssh -t "$RIG_HOST" "sudo journalctl -u$(quote_args "$svc")$(quote_args "$@")"
        ;;
    restart | start | stop)
        [ $# -eq 1 ] || { echo "$cmd: exactly one service name required" >&2; exit 1; }
        svc=$(unit_name "$1")
        ssh "$RIG_HOST" "sudo systemctl $cmd$(quote_args "$svc") && systemctl --no-pager --plain list-units --type=service --all$(quote_args "$svc")"
        ;;
    fetch-configs)
        dest="${1:-$HOME/.config/rusty-photon-rig}"
        # The address as reachable from this machine, taken from ssh's own
        # resolution of the alias — keeps it out of the repo.
        addr=$(ssh -G "$RIG_HOST" 2>/dev/null | awk '/^hostname /{print $2}')
        if [ -z "$addr" ]; then
            echo "fetch-configs: ssh could not resolve an address for '$RIG_HOST'" >&2
            echo "add a 'Host $RIG_HOST' entry to ~/.ssh/config or set RIG_HOST" >&2
            exit 1
        fi
        mkdir -p "$dest"
        ssh -T "$RIG_HOST" "sudo tar -C '$CONFIG_DIR' -cf - ." | tar -C "$dest" -xf -
        for f in "$dest"/*.json; do
            [ -e "$f" ] || continue
            sed -e "s|http://127\.0\.0\.1:|http://$addr:|g" \
                -e "s|http://localhost:|http://$addr:|g" \
                "$f" > "$f.tmp" && mv "$f.tmp" "$f"
        done
        echo "rig configs written to $dest (driver endpoints -> $addr)"
        ;;
    ssh)
        if [ $# -eq 0 ]; then
            ssh "$RIG_HOST"
        else
            ssh -t "$RIG_HOST" "$@"
        fi
        ;;
    -h | --help | help)
        usage 0
        ;;
    *)
        echo "unknown command: $cmd" >&2
        usage 1
        ;;
esac
