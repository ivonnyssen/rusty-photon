# Bazel remote cache — Cloudflare Worker + R2

A serverless Bazel HTTP remote cache for **GitHub cloud CI**, served from
Cloudflare's edge and backed by [R2](https://developers.cloudflare.com/r2/)
(S3-compatible object storage, **zero egress fees**).

## Overview

Bazel's HTTP cache protocol is a simple keyed blob store (`GET`/`PUT` on
`/ac/<hash>` and `/cas/<hash>`). This Worker maps it onto an R2 bucket and
serves it from Cloudflare's edge, so cloud CI gets fast cache reads at zero
egress cost. Access is public-read / token-write (below), served at
`cache.rustyphoton.space`.

For **local builds**, point a (gitignored) `user.bazelrc` at a LAN
`bazel-remote` instead, which reads at LAN speed:

```
build:remote-cache --remote_cache=http://<your-lan-cache-host>:8088
```

| Consumer | Cache |
|---|---|
| GitHub cloud CI (`bazel.yml`) | this Worker + R2 (`cache.rustyphoton.space`) |
| Local builds | your own LAN `bazel-remote` (via `user.bazelrc`) |

## Security model (public repo)

- **GET/HEAD = anonymous** — every PR (incl. forks) gets a warm cache.
- **PUT = `Authorization: Bearer <WRITE_TOKEN>`** — only push-to-main / nightly
  (which hold the GitHub secret) can write, so forks can't poison the cache.

The `WRITE_TOKEN` Worker secret must equal the `BAZEL_CACHE_WRITE_TOKEN` GitHub
Actions secret (`bazel.yml` already sends `Authorization: Bearer` on push/schedule).

## Deploy (one-time)

Prereqs: a Cloudflare account with the `rustyphoton.space` zone, R2 enabled
(free), and `wrangler` (`npm i -g wrangler`, or `npx wrangler`).

```bash
cd tools/bazel-cache-worker

# 1. Create the bucket.
wrangler r2 bucket create rusty-photon-bazel-cache

# 2. Set the write token (paste the SAME value as the GitHub secret
#    BAZEL_CACHE_WRITE_TOKEN; don't echo it into shell history).
wrangler secret put WRITE_TOKEN

# 3. Make sure no other Cloudflare DNS record/route already claims
#    cache.rustyphoton.space (remove it if so), then deploy — this provisions
#    the Worker + the cache.rustyphoton.space custom domain.
wrangler deploy
```

### Retention / eviction

R2 has no built-in LRU, so set a **lifecycle rule** to bound growth: Cloudflare
dashboard → R2 → `rusty-photon-bazel-cache` → Settings → Object lifecycle →
**delete objects older than 30 days**. Cache entries are regenerable, so
age-based eviction is safe and keeps storage in the free tier.

## Verify

```bash
curl -sf https://cache.rustyphoton.space/status                       # -> ok
H=0000000000000000000000000000000000000000000000000000000000000000
curl -s -o/dev/null -w '%{http_code}\n' -X PUT --data x \
  https://cache.rustyphoton.space/cas/$H                              # -> 403 (no token)
curl -s -o/dev/null -w '%{http_code}\n' -X PUT --data x \
  -H "Authorization: Bearer <WRITE_TOKEN>" \
  https://cache.rustyphoton.space/cas/$H                              # -> 200
```

On the next push-to-main, `cache.rustyphoton.space` fills from the edge and PR
reads come back fast (`bazel.yml` shows remote cache hits without long
`Downloading …` stalls).

## Cost

Effectively free. R2: 10 GB storage free, then ~$0.015/GB·mo, **$0 egress** (the
whole point — cache reads don't cost). Workers: 100k requests/day free; a very
busy CI day could tip into the $5/mo Workers plan (10M requests). Debuginfo is
stripped in `.bazelrc`, so individual CAS blobs stay well under Cloudflare's
100 MB request-body (write) limit.
