# Nightly package repos — Cloudflare R2 public bucket

The `apt`/`dnf` repositories for the nightly channel
(docs/plans/nightly-releases.md, phase N5), served at
`pkg.rustyphoton.space` straight from the `rusty-photon-packages` R2
bucket via R2's custom-domain public-bucket feature. Unlike
[../bazel-cache-worker](../bazel-cache-worker/README.md) there is **no
Worker** — and therefore no `wrangler.toml` here, nothing deploys: the
bucket needs no eviction or touch logic because the tree is small and
fully replaced every night by `scripts/push-packages-repo.sh` (see its
header for the client-safe upload/flip/delete ordering). Setup is the
handful of one-time commands below.

## Layout (what the bucket serves)

```
pkg.rustyphoton.space/
  pubkey.asc          # repo signing key, public half —
                      # byte-for-byte packaging/gpg/pubkey.asc
  manifest.txt        # full object listing; push-packages-repo.sh reads it
                      # to find stale objects (wrangler has no `r2 object list`)
  deb/dists/nightly/  # InRelease, Release(.gpg), main/binary-{amd64,arm64}/
  deb/pool/main/      # the .debs
  rpm/x86_64/         # .rpms + repodata/ (repomd.xml + repomd.xml.asc)
  rpm/aarch64/        # same for arm64
```

Client setup lives in
[docs/packaging.md](../../docs/packaging.md#nightly-channel).

## Security model (public repo)

- **GET = anonymous** — a public bucket on a custom domain; R2 serves
  `GET`/`HEAD`/`Range`/conditional requests natively, no code in front.
- **Writes = the CI publish job only**, via the Cloudflare API with
  `PACKAGES_R2_API_TOKEN` (Object Read & Write, scoped to just this
  bucket), held as a GitHub Actions secret.
- **The trust anchor for consumers is the GPG signature, not the
  transport**: apt verifies `InRelease`/`Release.gpg` and dnf verifies
  `repomd.xml.asc` (the documented `.repo` sets `repo_gpgcheck=1`)
  against the public key committed at `packaging/gpg/pubkey.asc` —
  fingerprint recorded in docs/packaging.md. The signed metadata covers
  every package via its checksums, so packages are not individually
  signed.

## Setup (one-time)

Prereqs: the Cloudflare account with the `rustyphoton.space` zone, R2
enabled, `wrangler` (`npm i -g wrangler`), and the GPG keypair
(generated offline: rsa4096, sign-only, no passphrase, no expiry —
the same bare-secret trust model as every other CI credential).

```bash
# 1. Create the bucket.
wrangler r2 bucket create rusty-photon-packages

# 2. Attach the public custom domain (make sure no DNS record already
#    claims pkg.rustyphoton.space; the zone is in the same account, so
#    this provisions DNS itself). Leave the r2.dev URL disabled.
wrangler r2 bucket domain add rusty-photon-packages \
    --domain pkg.rustyphoton.space --zone-id <zone id of rustyphoton.space>
#    (dashboard equivalent: R2 → bucket → Settings → Public access →
#    Custom Domains → Connect domain)

# 3. CI write credentials: dashboard → R2 → API → Manage API tokens →
#    create with Object Read & Write on ONLY this bucket; the "Token
#    value" (not the S3 key pair shown next to it) becomes:
gh secret set PACKAGES_R2_API_TOKEN
gh secret set CLOUDFLARE_ACCOUNT_ID     # wrangler needs it with the token

# 4. Signing key: private half to CI (back it up first — GitHub secrets
#    are write-only), public half committed at packaging/gpg/pubkey.asc.
gh secret set PACKAGES_GPG_PRIVATE_KEY < private.asc
```

## Caching

Every object is uploaded with `Cache-Control: no-store`
(`push-packages-repo.sh`), so Cloudflare's edge never serves a stale
mix — a cached `Packages.gz` outliving a flipped `InRelease` would be a
client hash-mismatch. Reads always origin-pull from R2: egress is free,
and this channel's traffic is a handful of rigs. Do **not** add cache
rules for this hostname; if the channel ever gains real traffic, revisit
by caching only the immutable objects (pool files and hash-named
repodata blobs), never `dists/`, `repodata/repomd.*`, `pubkey.asc`, or
`manifest.txt`.

## Verify

```bash
curl -sf https://pkg.rustyphoton.space/pubkey.asc | gpg --show-keys      # fingerprint per docs/packaging.md
curl -sf https://pkg.rustyphoton.space/deb/dists/nightly/InRelease | head -5
curl -sf https://pkg.rustyphoton.space/manifest.txt | wc -l              # object count
```

Then the real proof, on any Debian/Fedora machine: the client setup in
docs/packaging.md followed by `apt update` / `dnf makecache` — both
verify the signature with only the public key.

## Cost

Effectively free: the tree is ~150 MB (34 debs + 34 rpms + metadata),
far inside R2's 10 GB free tier; egress is $0; nightly writes are ~90
Class A operations against a 1M/month free allowance, and the no-store
reads are Class B operations (10M/month free).
