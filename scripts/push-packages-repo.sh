#!/bin/sh
# push-packages-repo.sh — replace the published package-repo tree in the
# R2 bucket behind pkg.rustyphoton.space with the freshly built-and-
# verified SITE (docs/plans/nightly-releases.md, phase N5). The upload
# ordering is what makes the nightly replacement safe for a client
# mid-`apt update`/`dnf makecache`:
#
#   1. read the previously published object list — manifest.txt on the
#      bucket (wrangler has no `r2 object list`; the manifest IS the
#      listing),
#   2. upload every content object (pool debs, rpms, Packages indices,
#      repodata blobs, pubkey.asc) — purely additive: the old metadata
#      keeps serving the old, still-complete tree throughout,
#   3. upload the metadata entry points last, as the flip — each
#      signature lands before the file it covers, so the moment a client
#      sees new metadata its signature is already fetchable:
#      Release.gpg → Release → InRelease (apt), repomd.xml.asc →
#      repomd.xml (dnf),
#   4. only now delete stale objects (previous manifest minus the new
#      tree), so nothing the old metadata could still point a client at
#      ever disappears mid-publish; per-key tolerant, because an
#      interrupted earlier run may have removed some already,
#   5. upload the new manifest.txt. An interruption anywhere leaves the
#      OLD manifest on the bucket still listing any leaked keys, so the
#      next run's step 4 sweeps them — self-healing, no growth.
#
# Auth: wrangler reads CLOUDFLARE_API_TOKEN (the PACKAGES_R2_API_TOKEN
# secret — Object Read & Write scoped to just this bucket) and
# CLOUDFLARE_ACCOUNT_ID from the environment.
#
# Usage: scripts/push-packages-repo.sh [--site DIR] [--bucket NAME]
#   --site DIR     the verified tree to publish (default: site)
#   --bucket NAME  R2 bucket name (default: rusty-photon-packages)

set -eu

die() { echo "push-packages-repo: $*" >&2; exit 1; }

usage() {
    sed -n '/^# Usage:/,/^$/{s/^# \{0,1\}//p}' "$0"
}

SITE="site"
BUCKET="rusty-photon-packages"
while [ $# -gt 0 ]; do
    case "$1" in
        --site)
            shift
            [ $# -gt 0 ] || die "--site needs a directory"
            SITE="$1"
            ;;
        --bucket)
            shift
            [ $# -gt 0 ] || die "--bucket needs a name"
            BUCKET="$1"
            ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; die "unknown option: $1" ;;
    esac
    shift
done

command -v wrangler > /dev/null 2>&1 || die "wrangler not found (npm install -g wrangler)"
[ -n "${CLOUDFLARE_API_TOKEN:-}" ] || die "CLOUDFLARE_API_TOKEN is not set"
[ -n "${CLOUDFLARE_ACCOUNT_ID:-}" ] || die "CLOUDFLARE_ACCOUNT_ID is not set"
[ -f "$SITE/pubkey.asc" ] || die "$SITE/pubkey.asc missing — build + verify the tree first"
SITE_ABS=$(cd "$SITE" && pwd)

TMPD=$(mktemp -d)
trap 'rm -rf "$TMPD"' EXIT INT TERM

put() {
    # no-store: Cloudflare's default edge cache covers .gz, and a stale
    # cached Packages.gz against a freshly flipped InRelease is a client
    # hash-mismatch. Every read origin-pulls from R2 instead — free egress,
    # and this channel's traffic is a handful of rigs; revisit only if
    # that ever changes.
    wrangler r2 object put "$BUCKET/$1" --file "$2" --cache-control no-store --remote > /dev/null 2>&1 \
        || die "upload failed: $1"
    echo "  put $1"
}

is_flip() {
    case "$1" in
        deb/dists/*/InRelease | deb/dists/*/Release | deb/dists/*/Release.gpg) return 0 ;;
        rpm/*/repodata/repomd.xml | rpm/*/repodata/repomd.xml.asc) return 0 ;;
        *) return 1 ;;
    esac
}

# 1. The previously published listing (absent on the first publish).
: > "$TMPD/old.txt"
if wrangler r2 object get "$BUCKET/manifest.txt" --file "$TMPD/old.txt" --remote > /dev/null 2>&1; then
    echo "Previous manifest: $(wc -l < "$TMPD/old.txt" | tr -d ' ') objects"
else
    echo "No previous manifest.txt on the bucket (first publish)"
fi
LC_ALL=C sort -u "$TMPD/old.txt" > "$TMPD/old.sorted"

(cd "$SITE_ABS" && find . -type f ! -name manifest.txt | sed 's|^\./||' | LC_ALL=C sort) > "$TMPD/new.txt"
[ -s "$TMPD/new.txt" ] || die "no files under $SITE"

# 2. Content first — additive while the old metadata is still live.
echo "Uploading content objects..."
while IFS= read -r k; do
    if ! is_flip "$k"; then
        put "$k" "$SITE_ABS/$k"
    fi
done < "$TMPD/new.txt"

# 3. The flip, signature-before-signed within each pair/trio.
echo "Flipping metadata..."
for name in Release.gpg Release InRelease repomd.xml.asc repomd.xml; do
    while IFS= read -r k; do
        if [ "$(basename "$k")" = "$name" ] && is_flip "$k"; then
            put "$k" "$SITE_ABS/$k"
        fi
    done < "$TMPD/new.txt"
done

# 4. Sweep what the previous publish served and this one no longer does.
comm -23 "$TMPD/old.sorted" "$TMPD/new.txt" > "$TMPD/stale.txt"
if [ -s "$TMPD/stale.txt" ]; then
    echo "Deleting $(wc -l < "$TMPD/stale.txt" | tr -d ' ') stale objects..."
    while IFS= read -r k; do
        [ -n "$k" ] || continue
        if wrangler r2 object delete "$BUCKET/$k" --remote > /dev/null 2>&1; then
            echo "  deleted $k"
        else
            echo "push-packages-repo: warning: could not delete stale $k (already gone?)" >&2
        fi
    done < "$TMPD/stale.txt"
else
    echo "No stale objects to delete"
fi

# 5. The new listing becomes the next run's step 1.
put manifest.txt "$TMPD/new.txt"

echo ""
echo "push-packages-repo: OK ($(wc -l < "$TMPD/new.txt" | tr -d ' ') objects in $BUCKET)"
