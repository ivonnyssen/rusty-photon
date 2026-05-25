# Skill: Self-Hosted Bazel Remote Cache (TrueNAS SCALE + Cloudflare Tunnel)

## When to Read This

- Standing up or re-pointing the Bazel remote cache that `.github/workflows/bazel.yml` uses.
- Diagnosing cache misses, `403` on upload, or a cold-cache outlier in a Bazel CI run.
- Auditing the cache's security posture on this **public** repo.

This replaces the BuildBuddy free-tier cache (see `docs/plans/bazel-migration.md` → Decisions, 2026-05-24). The replacement fixes BuildBuddy's LRU eviction (no retention guarantee), which caused random cold-cache rebuilds — most visibly a ~15 min Windows `bazel build` when action results had aged out.

The **repo-side wiring is already committed** (`.bazelrc`, `.github/workflows/bazel.yml`). This runbook covers the TrueNAS + Cloudflare side and the two values you must fill in (§4).

## Architecture

```
GitHub-hosted runner (bazel.yml)
   │   GET  = anonymous            → reads, incl. fork PRs (warm cache for everyone)
   │   PUT  = Authorization: Bearer → writes, only on push-to-main / nightly
   ▼
Cloudflare edge   cache.rustyphoton.space          ← free TLS, no open router ports
   │   (Cloudflare Tunnel; cloudflared dials OUT from TrueNAS)
   ▼  ┌──────────────── TrueNAS SCALE ────────────────────────────────────────┐
   └─▶│ Cloudflared app ─→ Caddy :8088 ─→ bazel-remote :8080 ─→ /data (SSD pool)│
      │   (catalog)        (read/write split)  (cache server)   (dataset)       │
      └────────────────────────────────────────────────────────────────────────┘
```

Caddy is required because `bazel-remote`'s own auth (`--htpasswd_file`) gates **all** methods identically. We want anonymous reads (so every PR, including forks, gets cache speed) but token-gated writes (anti-poisoning). Caddy does that split in six lines.

## Security model — why this is safe on a public repo

The cache stores build/test **outputs of public OSS code** — not secrets. So:

- **Reads are anonymous.** Harmless, and a feature: fork PRs get a warm cache (BuildBuddy's read-only-key model never gave forks anything, because GitHub withholds secrets from forks).
- **Writes require a Bearer token** that is a GitHub Actions **secret**. Secrets are exposed only to `push` and `schedule` events — which run from the default branch — never to PRs (and never to forks at all). So only trusted, main-derived runs can populate the cache. No poisoning path.
- **Defense in depth:** PRs are read-only by a Bazel flag (`--remote_upload_local_results=false`) *and* Caddy `403`s any unauthenticated write. A workflow edited in a malicious PR still can't write (no token in its env; Caddy rejects it).

This mirrors the trust split the BuildBuddy setup already encoded — just self-hosted, with retention you control.

## Prerequisites

- [ ] **TrueNAS SCALE 24.10.2.2+** ("Electric Eel"; the catalog Cloudflared app requires this).
- [ ] A **domain on a Cloudflare zone** (free plan is fine) — needed for a named Tunnel hostname.
- [ ] **~100 GB free** on the SSD pool.
- [ ] A write token: `openssl rand -hex 32` — used in two places (Caddy env in §2, GitHub secret in §4). Keep it somewhere safe.

---

## Part 1 — Dataset on the SSD pool

1. **Datasets → Add Dataset** on the SSD pool, e.g. `ssd-pool/apps/bazel-remote`. Note the mountpoint (e.g. `/mnt/ssd-pool/apps/bazel-remote`). Adjust the paths in §2 to match.
2. **Disable snapshots / replication** for it. The cache is fully reconstructible; snapshotting it just wastes SSD. Wiping it only costs one cold rebuild.
3. Create a `data/` subdir and drop the `Caddyfile` from §2 at the dataset root. Make `data/` writable by the container UID — on SCALE the built-in apps user is **UID 568**; from **System → Shell**: `chown -R 568:568 /mnt/ssd-pool/apps/bazel-remote/data`.

## Part 2 — Deploy `bazel-remote` + Caddy (Apps → Custom App → Install via YAML)

These two images aren't in the catalog, so install them as one Custom App. **Apps → Discover Apps → Custom App → Install via YAML** opens the Compose editor:

```yaml
services:
  bazel-remote:
    image: buchgr/bazel-remote-cache:latest   # :latest, not pinned — not reproducible as a result; fine since the cache is disposable
    user: "568:568"                            # TrueNAS 'apps' uid:gid; must own /data
    command:
      - --dir=/data
      - --max_size=80                          # GiB; raise as the SSD pool allows
      - --storage_mode=zstd                    # compress at rest
      - --http_address=0.0.0.0:8080
      - --access_log_level=none
    volumes:
      - /mnt/ssd-pool/apps/bazel-remote/data:/data
    restart: unless-stopped
    # bazel-remote's 8080 is NOT published to the host — only Caddy reaches it
    # over the app's internal network.

  caddy:
    image: caddy:2
    depends_on: [bazel-remote]
    environment:
      CACHE_WRITE_TOKEN: "PASTE_OPENSSL_TOKEN_HERE"   # same value as the GitHub secret (§4)
    ports:
      - "8088:80"                              # published on the TrueNAS LAN IP
    volumes:
      - /mnt/ssd-pool/apps/bazel-remote/Caddyfile:/etc/caddy/Caddyfile:ro
    restart: unless-stopped
```

`Caddyfile` (anonymous read, Bearer-only write) — save at `/mnt/ssd-pool/apps/bazel-remote/Caddyfile` *before* launching:

```
:80 {
	@write_no_auth {
		method PUT POST PATCH DELETE
		not header Authorization "Bearer {$CACHE_WRITE_TOKEN}"
	}
	respond @write_no_auth 403
	reverse_proxy bazel-remote:8080
}
```

GET/HEAD never match the write matcher → served anonymously. A write with the exact Bearer token → proxied. A write without it → `403`. (`{$CACHE_WRITE_TOKEN}` is resolved from the container env at config load.)

> **Why publish Caddy on `:8088`?** The catalog Cloudflared app (§3) runs as a *separate* Docker app, so it can't reach `caddy` by service name — it connects via the TrueNAS LAN IP. Publishing `8088` also doubles as a **fast LAN dev cache**: point a gitignored `user.bazelrc` at `build:remote-cache --remote_cache=http://<TRUENAS_LAN_IP>:8088` and your local `bazel build` reuses CI's cache. On a trusted home LAN this exposure is fine (reads are harmless; writes still need the token).

### Using Traefik instead of Caddy (optional)

If you already run the catalog **Traefik** app as your ingress, you can drop the `cloudflared`→`caddy` hop and route the public hostname through Traefik. The catch: Traefik has no built-in "require this exact Bearer token" middleware, so the read/write split needs one of:

- a **ForwardAuth** middleware pointing at a tiny token-checker, applied only to a router with a `Method(\`PUT\`,\`POST\`,\`PATCH\`,\`DELETE\`)` rule (a second catch-all router serves reads with no middleware), or
- a community API-key/bearer **plugin**, or
- switching CI to **Basic auth** via URL-embedded creds (`--remote_cache=https://user:pass@cache...`) — but that sends credentials on reads too and changes the `bazel.yml` wiring.

Caddy keeps the proxy self-contained and matches the `Authorization: Bearer` wiring already committed, so it's the default here. If you'd rather standardize on Traefik, say so and the router + ForwardAuth config can replace §2's `caddy` service.

## Part 3 — Cloudflare Tunnel via the catalog Cloudflared app

1. **Cloudflare Zero Trust → Networks → Tunnels → Create a tunnel** → *Cloudflared* → name it (e.g. `truenas`) → copy the **tunnel token**.
2. **TrueNAS → Apps → Discover Apps → search "Cloudflared"** (Community train) → **Install** → paste the tunnel token into the app config → deploy. No ports are opened; `cloudflared` dials out to Cloudflare.
3. Back in the Cloudflare tunnel → **Public Hostname → Add**:
   - Subdomain `cache`, Domain `rustyphoton.space`
   - Service **Type** `HTTP`, **URL** `<TRUENAS_LAN_IP>:8088`
   Save. Cloudflare auto-creates the proxied DNS record and edge TLS.

## Part 4 — Fill in the two repo values

1. **`.bazelrc`:** already set — `--remote_cache=https://cache.rustyphoton.space` is committed (zone `rustyphoton.space` verified on Cloudflare). No change needed unless the hostname differs.
2. **GitHub repo secret:** Settings → Secrets and variables → Actions → **New repository secret** → name `BAZEL_CACHE_WRITE_TOKEN`, value = the same token as Caddy's `CACHE_WRITE_TOKEN`.
3. (After a week of green) delete the old `BUILDBUDDY_API_KEY` / `BUILDBUDDY_API_KEY_READONLY` secrets.

## Part 5 — Validate

- **Status:** `curl -sf https://cache.rustyphoton.space/status` → bazel-remote JSON (`NumFiles`, `ServerTime`, …).
- **Write auth:** a write without the token must be rejected, with it must succeed:
  ```bash
  curl -s -o /dev/null -w '%{http_code}\n' -X PUT --data x \
    https://cache.rustyphoton.space/cas/0000000000000000000000000000000000000000000000000000000000000000      # → 403
  curl -s -o /dev/null -w '%{http_code}\n' -X PUT --data x \
    -H "Authorization: Bearer <token>" \
    https://cache.rustyphoton.space/cas/0000000000000000000000000000000000000000000000000000000000000000      # → 200
  ```
- **End to end (LAN):** `bazel build //... --config=remote-cache --remote_cache=http://<TRUENAS_LAN_IP>:8088`, then `bazel clean && bazel build //...` → the tail line shows `remote cache hit`.
- **CI:** push the `feature/bazel-remote-truenas` branch; open the Bazel run and confirm `… processes: N remote cache hit`. The first push-to-main populates; subsequent PRs read.

## Part 6 — Rollback

Cache outages are **non-fatal** — Bazel treats remote-cache errors as a cache miss (warns, builds locally), so a stopped tunnel or full disk degrades to a cold build, never a red CI. To fully revert: `git revert` the `.bazelrc` + `bazel.yml` change; the BuildBuddy secrets are still present until you delete them (§4.3), so the old backend works again in one commit.

## Operational notes

- **Sizing:** `--max_size 80` (GiB) is the cache-content cap; Linux + macOS + Windows artifacts coexist (platform is part of the action key). Raise it freely on an SSD pool. Deterministic LRU within the cap — no surprise eviction.
- **No backup:** the dataset is a cache; leave snapshots off.
- **Lost build UI:** `bazel-remote` is cache-only; there's no BuildBuddy-style invocation dashboard. Timing still comes from the GHA logs and the `--profile` / exec-log artifacts `bazel.yml` already uploads. If you miss the UI, you can re-add BuildBuddy's *free BES* (`--bes_backend`) independently of the cache.
- **HTTP not gRPC:** simplest over a Tunnel. `--remote_cache_compression` is gRPC-only and intentionally omitted; `--storage_mode zstd` compresses at rest. gRPC is a later optimization.

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| CI uploads `403` | `BAZEL_CACHE_WRITE_TOKEN` (GitHub) ≠ `CACHE_WRITE_TOKEN` (Caddy env). |
| Every run is a full miss | action-key instability (an env var like `PATH` leaking into keys), cache wiped, or `--max_size` too small so entries evict each run. |
| `curl /status` hangs/fails | Cloudflared app stopped or wrong tunnel token; or the public hostname points at the wrong `LAN_IP:8088`. |
| Reads work, writes never cache | expected on PRs (read-only by design); only push-to-main / nightly write. |
| Caddy won't start | `Caddyfile` missing at the mounted host path, or `CACHE_WRITE_TOKEN` env unset. |

## References

- [docs/plans/bazel-migration.md](../plans/bazel-migration.md) — Decisions (2026-05-24) and Phase 5.
- [.bazelrc](../../.bazelrc) `build:remote-cache` / [.github/workflows/bazel.yml](../../.github/workflows/bazel.yml) — the committed wiring.
- [bazel-remote](https://github.com/buchgr/bazel-remote) — the cache server (flags, releases).
- [TrueNAS: Custom App / Install via YAML](https://www.truenas.com/docs/scale/24.10/scaleuireference/apps/installcustomappscreens/)
- [TrueNAS: Cloudflared app](https://apps.truenas.com/catalog/cloudflared) and [Cloudflare Tunnel tutorial](https://www.truenas.com/docs/scale/24.04/scaletutorials/apps/appsecurity/cloudflaretunnel/)
- [Bazel remote caching](https://bazel.build/remote/caching) — HTTP cache protocol, `--remote_upload_local_results`.
