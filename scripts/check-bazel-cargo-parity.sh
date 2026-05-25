#!/usr/bin/env bash
# Bazel/Cargo target-parity check.
#
# Ensures every Cargo workspace member has at least one Bazel rust target, so
# that "Bazel green" actually means Bazel builds what Cargo builds. Hand-written
# BUILD.bazel files can silently drift from the Cargo workspace — a crate added
# to Cargo.toml can go without a BUILD.bazel and escape the Bazel gate entirely.
# This guards against that. See docs/plans/bazel-migration.md.
#
# A Cargo member is "covered" if its directory contains any rust_* Bazel rule
# (rust_library / rust_binary / rust_proc_macro / rust_shared_library / ...).
# The script FAILS on a Cargo member with no Bazel target that is not in the
# documented EXPECTED_GAPS allowlist below — which doubles as the pre-cutover
# TODO list. Empty that list before making Bazel the required PR gate.
#
# Requires: cargo, bazel (bazelisk), jq, git.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# --- Cargo members deliberately not (yet) in the Bazel graph ----------------
# Each line is a crate the Bazel gate does NOT cover. Remove an entry the
# moment its BUILD.bazel lands; the cutover (docs/plans/bazel-migration.md
# Phase 7) should not flip until this list is empty. Inline "# reason"
# comments are allowed and stripped before comparison.
read -r -d '' EXPECTED_GAPS <<'GAPS' || true
crates/skywatcher-motor-protocol  # plain library; BUILD.bazel pending (cutover prereq)
services/dsd-fp2                   # CoverCalibrator service; BUILD.bazel pending
services/star-adventurer-gti       # mount service; BUILD.bazel pending
GAPS

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Cargo workspace member directories, relative to the workspace root.
root=$(cargo metadata --format-version 1 | jq -r '.workspace_root')
cargo metadata --no-deps --format-version 1 \
  | jq -r --arg root "$root/" '.packages[].manifest_path | ltrimstr($root) | rtrimstr("/Cargo.toml")' \
  | sort -u > "$tmp/cargo.txt"

# Bazel packages (directories) containing any rust_* rule. --output=package
# yields the directory, e.g. "services/rp".
bazel query 'kind("rust_.*", //...)' --output=package 2>/dev/null \
  | sort -u > "$tmp/bazel.txt"

# Strip inline/full-line "# comments", trailing whitespace, and blank lines.
printf '%s\n' "$EXPECTED_GAPS" | sed 's/#.*//; s/[[:space:]]*$//; /^[[:space:]]*$/d' | sort -u > "$tmp/allow.txt"

# Cargo members with no Bazel target.
comm -23 "$tmp/cargo.txt" "$tmp/bazel.txt" > "$tmp/gaps.txt"
# New (not allowlisted) => failure. Closed (allowlisted but now covered) => nudge.
comm -23 "$tmp/gaps.txt" "$tmp/allow.txt" > "$tmp/new.txt"
comm -13 "$tmp/gaps.txt" "$tmp/allow.txt" > "$tmp/closed.txt"

echo "::group::Bazel/Cargo target parity"
echo "Cargo workspace members  : $(wc -l < "$tmp/cargo.txt")"
echo "Covered by a Bazel target: $(comm -12 "$tmp/cargo.txt" "$tmp/bazel.txt" | wc -l)"
echo "Current gaps             :"
sed 's/^/    /' "$tmp/gaps.txt"
echo "::endgroup::"

status=0
if [ -s "$tmp/new.txt" ]; then
  echo "::error::Cargo member(s) with no Bazel target and not allowlisted. Add a BUILD.bazel (preferred), or — if the omission is intentional — add the path to EXPECTED_GAPS in scripts/check-bazel-cargo-parity.sh with a reason:"
  sed 's/^/    /' "$tmp/new.txt"
  status=1
fi
if [ -s "$tmp/closed.txt" ]; then
  echo "::warning::These allowlisted gaps now HAVE Bazel targets — remove them from EXPECTED_GAPS:"
  sed 's/^/    /' "$tmp/closed.txt"
fi

if [ "$status" -eq 0 ]; then
  remaining=$(wc -l < "$tmp/allow.txt")
  if [ "$remaining" -gt 0 ]; then
    echo "Parity OK (no new gaps). $remaining crate(s) still need a BUILD.bazel before the Bazel-required cutover."
  else
    echo "Parity OK — Bazel builds every Cargo workspace member."
  fi
fi
exit "$status"
