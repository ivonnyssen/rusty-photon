#!/bin/bash
# Resolve which workspace members are affected by changes to [workspace.dependencies].
#
# Used by CI workflows when cargo-rail's custom:workspace-manifest surface fires.
# Instead of rebuilding/testing everything, this script uses cargo metadata to
# determine which crates actually depend on the changed workspace dependencies.
#
# Usage: resolve-workspace-deps.sh <base-ref>
#
# Outputs (via $GITHUB_OUTPUT or stdout):
#   mode=targeted|workspace|none
#   cargo-args=-p crate1 -p crate2    (only when mode=targeted)
#   crates=crate1 crate2              (only when mode=targeted)
#   test=true|false

set -euo pipefail

BASE_REF="${1:?Usage: resolve-workspace-deps.sh <base-ref>}"

# --- Section comparison ---
# Check if non-dependency sections of Cargo.toml changed.
# If [workspace.package], [workspace.lints], [profile], members, or resolver
# changed, the entire workspace must be rebuilt and tested.

OLD_TOML=$(git show "${BASE_REF}:Cargo.toml" 2>/dev/null) || {
  echo "mode=workspace" >> "${GITHUB_OUTPUT:-/dev/stdout}"
  echo "test=true" >> "${GITHUB_OUTPUT:-/dev/stdout}"
  exit 0
}
NEW_TOML=$(cat Cargo.toml)

# --- Section comparison ---
# Strip the [workspace.dependencies] section from both files and compare the
# remainder. If anything outside that section changed, trigger full rebuild.
# This correctly handles [workspace.lints], [workspace.package], [profile], etc.
#
# The sed command deletes lines from [workspace.dependencies] up to (but not
# including) the next section header or EOF.
strip_ws_deps() {
  sed '/^\[workspace\.dependencies\]/,/^\[/{/^\[workspace\.dependencies\]/d;/^\[/!d;}' | \
    sed '/^$/d'
}

OLD_STRIPPED=$(echo "$OLD_TOML" | strip_ws_deps)
NEW_STRIPPED=$(echo "$NEW_TOML" | strip_ws_deps)

if [ "$OLD_STRIPPED" != "$NEW_STRIPPED" ]; then
  echo "Non-dependency sections changed -- full rebuild required"
  diff <(echo "$OLD_STRIPPED") <(echo "$NEW_STRIPPED") | head -10 || true
  echo "mode=workspace" >> "${GITHUB_OUTPUT:-/dev/stdout}"
  echo "test=true" >> "${GITHUB_OUTPUT:-/dev/stdout}"
  exit 0
fi

# --- Dep extraction ---
# Only [workspace.dependencies] entries changed. Extract the dep names from
# the old and new sections, then find which ones differ.
# Filter to key lines only (start with dep-name followed by whitespace and =)
# to skip multi-line continuation lines (e.g., features = [ ... ]).
extract_ws_deps() {
  sed -n '/^\[workspace\.dependencies\]/,/^\[/{/^\[/d;/^$/d;/^#/d;p;}' | \
    grep -E '^[a-zA-Z0-9_][a-zA-Z0-9_.-]*[[:space:]]*=' | \
    sed 's/[[:space:]]*=.*//' | sort
}

OLD_DEPS=$(echo "$OLD_TOML" | extract_ws_deps)
NEW_DEPS=$(echo "$NEW_TOML" | extract_ws_deps)

# Find dep names that were added, removed, or whose value changed
CHANGED_DEPS=""
for dep in $(echo "$OLD_DEPS"$'\n'"$NEW_DEPS" | sort -u); do
  OLD_LINE=$(echo "$OLD_TOML" | sed -n "/^\[workspace\.dependencies\]/,/^\[/{/^${dep}[[:space:]]*=/p;}")
  NEW_LINE=$(echo "$NEW_TOML" | sed -n "/^\[workspace\.dependencies\]/,/^\[/{/^${dep}[[:space:]]*=/p;}")
  if [ "$OLD_LINE" != "$NEW_LINE" ]; then
    CHANGED_DEPS="$CHANGED_DEPS $dep"
  fi
done
CHANGED_DEPS=$(echo "$CHANGED_DEPS" | xargs echo 2>/dev/null)

if [ -z "$CHANGED_DEPS" ]; then
  echo "No dependency changes detected"
  echo "mode=none" >> "${GITHUB_OUTPUT:-/dev/stdout}"
  echo "test=false" >> "${GITHUB_OUTPUT:-/dev/stdout}"
  exit 0
fi

echo "Changed workspace deps: $CHANGED_DEPS"

# --- Member resolution ---
# Find which workspace members reference the changed deps with workspace = true.

METADATA=$(cargo metadata --format-version 1 --no-deps 2>/dev/null)

AFFECTED=""
while IFS= read -r pkg_line; do
  pkg_name=$(echo "$pkg_line" | jq -r '.name')
  manifest_path=$(echo "$pkg_line" | jq -r '.manifest_path')

  for dep in $CHANGED_DEPS; do
    # Check if the member's Cargo.toml has this dep with workspace = true.
    # Handles both `dep = { workspace = true }` and `dep.workspace = true` forms.
    if grep -qE "^${dep}[[:space:]]*(=.*workspace|\.workspace)" "$manifest_path" 2>/dev/null; then
      AFFECTED="$AFFECTED $pkg_name"
      break
    fi
  done
done < <(echo "$METADATA" | jq -c '.packages[]') || true

AFFECTED=$(echo "$AFFECTED" | xargs -n1 2>/dev/null | sort -u | xargs echo 2>/dev/null)

if [ -z "$AFFECTED" ] || [ "$AFFECTED" = "" ]; then
  echo "Changed deps not used by any workspace member"
  echo "mode=none" >> "${GITHUB_OUTPUT:-/dev/stdout}"
  echo "test=false" >> "${GITHUB_OUTPUT:-/dev/stdout}"
  exit 0
fi

echo "Directly affected crates: $AFFECTED"

# --- Reverse dependency walk (transitive BFS) ---
# Include workspace members that transitively depend on affected crates.
# Uses --no-deps metadata + member Cargo.toml grep to avoid full resolution
# (which would fail if a new dep version hasn't been published yet).
# Iterates until fixpoint to catch multi-hop chains (A -> B -> C).

ALL_AFFECTED="$AFFECTED"
QUEUE="$AFFECTED"

while [ -n "$QUEUE" ]; do
  NEXT_QUEUE=""
  for crate in $QUEUE; do
    while IFS= read -r pkg_line; do
      other_name=$(echo "$pkg_line" | jq -r '.name')
      other_manifest=$(echo "$pkg_line" | jq -r '.manifest_path')

      # Skip self and already-known crates
      echo " $ALL_AFFECTED " | grep -q " $other_name " && continue

      if grep -qE "^${crate}[[:space:]]*(=.*workspace|\.workspace)" "$other_manifest" 2>/dev/null; then
        ALL_AFFECTED="$ALL_AFFECTED $other_name"
        NEXT_QUEUE="$NEXT_QUEUE $other_name"
      fi
    done < <(echo "$METADATA" | jq -c '.packages[]') || true
  done
  QUEUE=$(echo "$NEXT_QUEUE" | xargs echo 2>/dev/null)
done

ALL_AFFECTED=$(echo "$ALL_AFFECTED" | xargs -n1 2>/dev/null | sort -u | xargs echo 2>/dev/null)
CARGO_ARGS=$(echo "$ALL_AFFECTED" | xargs -n1 2>/dev/null | sed 's/^/-p /' | xargs echo 2>/dev/null)

echo "All affected crates (with reverse deps): $ALL_AFFECTED"
echo "Cargo args: $CARGO_ARGS"

echo "mode=targeted" >> "${GITHUB_OUTPUT:-/dev/stdout}"
echo "cargo-args=$CARGO_ARGS" >> "${GITHUB_OUTPUT:-/dev/stdout}"
echo "crates=$ALL_AFFECTED" >> "${GITHUB_OUTPUT:-/dev/stdout}"
echo "test=true" >> "${GITHUB_OUTPUT:-/dev/stdout}"
