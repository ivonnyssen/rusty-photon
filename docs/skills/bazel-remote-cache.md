# Skill: Bazel Remote Cache

## When to Read This

- Diagnosing Bazel cache misses, slow `Downloading …` stalls, or `403` on upload in CI.
- Changing or redeploying the remote cache.
- Auditing the cache's security posture on this **public** repo.

## What it is

Cloud CI (`.github/workflows/bazel.yml`) uses a Bazel HTTP remote cache served
by a **Cloudflare Worker backed by R2**, at `https://cache.rustyphoton.space`.
Code, config, and deploy steps live in
[`tools/bazel-cache-worker/`](../../tools/bazel-cache-worker/README.md).

For **local builds**, point a gitignored `user.bazelrc` at a LAN `bazel-remote`
(reads at LAN speed):

```
build:remote-cache --remote_cache=http://<your-lan-cache-host>:8088
```

## Security model (public repo)

The cache stores build/test outputs of public code, so:

- **Reads (GET/HEAD) are anonymous** — every PR, including forks, gets a warm cache.
- **Writes (PUT) require `Authorization: Bearer <token>`** — the token is a
  GitHub Actions secret (`BAZEL_CACHE_WRITE_TOKEN`) exposed only to `push`-to-main
  and the nightly `schedule`, never to PRs or forks. So only trusted,
  main-derived runs can populate the cache — no poisoning path.

`bazel.yml` attaches the Bearer token only on push/schedule and adds
`--remote_upload_local_results=false` on PRs (read-only). Reads need no
credentials, so fork PRs benefit too.

## Repo wiring

- `.bazelrc` — `build:remote-cache --remote_cache=https://cache.rustyphoton.space`; debuginfo is stripped (`-Cdebuginfo=0`) to keep cached blobs small.
- `bazel.yml` — sends the Bearer token on trusted events; reads anonymously otherwise.
- The GitHub secret `BAZEL_CACHE_WRITE_TOKEN` must match the Worker's `WRITE_TOKEN`.

## Verify

```bash
curl -sf https://cache.rustyphoton.space/status                       # -> ok
H=0000000000000000000000000000000000000000000000000000000000000000
curl -s -o/dev/null -w '%{http_code}\n' -X PUT --data x \
  https://cache.rustyphoton.space/cas/$H                              # -> 403 (no token)
```

A healthy CI run shows `… processes: N remote cache hit` without long
`Downloading …` stalls.

## QHYCCD SDK (qhy-camera) — not on this cache

The QHYCCD SDK that `qhy-camera` links (`static=qhyccd`, pinned 25.09.29) does
**not** go through this cache. It is publicly downloadable from qhyccd.com, and
the GitHub-hosted ubuntu jobs install it via the author's published
`ivonnyssen/qhyccd-sdk-install@v1` action (which wraps the download + the SDK's
own `install.sh` → `/usr/local/lib`, and caches it) — no secret, no SHA pin, no
internal tier. The Pi nightly installs it from qhyccd.com via
`scripts/setup-pi-runner.sh`. See `docs/services/qhy-camera.md` "Native
dependency & build gating".

## References

- [tools/bazel-cache-worker/](../../tools/bazel-cache-worker/README.md) — Worker code + deploy runbook.
- [docs/plans/bazel-migration.md](../plans/bazel-migration.md) — migration status.
