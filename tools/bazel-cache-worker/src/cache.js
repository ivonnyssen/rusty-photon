// Bazel HTTP remote cache backed by Cloudflare R2.
//
// Implements the Bazel HTTP cache protocol (GET/HEAD/PUT on /ac/<hash> and
// /cas/<hash>) over an R2 bucket, served from Cloudflare's edge, so cloud CI
// reads it at edge speed (with R2's zero egress cost).
//
// Security (public repo): reads are ANONYMOUS, so every PR including forks gets
// a warm cache; writes require a Bearer token that only push-to-main / nightly
// hold, so forks can never poison the cache. For local builds, point
// user.bazelrc at a LAN bazel-remote instead. See README.md.
//
// Bindings (wrangler.toml): CACHE = R2 bucket.  Secret: WRITE_TOKEN.

const KEY_RE = /^(ac|cas)\/[0-9a-fA-F]+$/;

// Touch-on-read. The bucket's lifecycle rule deletes objects by UPLOAD age,
// but Bazel never re-uploads an entry that keeps getting cache hits, so
// without a touch every object dies a fixed time after it was last BUILT, not
// last USED — the whole stable core of the action graph expires en masse once
// per lifecycle window and the next builds run cold. Re-putting a read object
// resets its lifecycle clock, turning age-based expiry into effective LRU:
// anything read within the window survives.
//
// Only objects older than TOUCH_AFTER_MS are re-put, bounding the Class A
// (write) cost to at most one write per read object per 2 days. The safety
// constraint is TOUCH_AFTER + the longest read gap of a live object < the
// lifecycle window (7 days); the nightly full build of main reads the entire
// //... action-cache set daily, so 2 days leaves wide margin. Known residue,
// backstopped by Bazel 9's default eviction retries (rewind + rebuild): blobs
// a hit-heavy build never GETs (build-without-the-bytes skips most
// intermediate downloads) are not touched and still age out.
const TOUCH_AFTER_MS = 2 * 24 * 60 * 60 * 1000;

async function touch(env, key, seenUploadedMs) {
  // Fresh get(): the client response consumes its own body stream. put()
  // accepts the R2ObjectBody stream directly (its length is known).
  const obj = await env.CACHE.get(key);
  // Gone, or already refreshed/replaced since the GET that scheduled this
  // touch (a sibling touch from a GET burst on the same stale key, or a real
  // push-to-main PUT) -> nothing to do. Collapses duplicate touches to one
  // Class A write and keeps a touch from resurrecting a replaced entry.
  if (!obj || obj.uploaded.getTime() !== seenUploadedMs) return;
  // The conditional write closes the remaining get->put window: if anything
  // replaced the object in between, the etag no longer matches and R2 drops
  // the touch (put resolves null; fine — the object is fresh either way).
  await env.CACHE.put(key, obj.body, { onlyIf: { etagMatches: obj.etag } });
}

export default {
  async fetch(request, env, ctx) {
    const key = new URL(request.url).pathname.replace(/^\/+/, "");

    // Health check for humans; Bazel never hits this.
    if (key === "" || key === "status") {
      return new Response("bazel-cache (cloudflare worker + r2) ok\n", { status: 200 });
    }
    if (!KEY_RE.test(key)) {
      return new Response("not a bazel cache key\n", { status: 400 });
    }

    switch (request.method) {
      case "GET": {
        const obj = await env.CACHE.get(key);
        if (!obj) return new Response(null, { status: 404 });
        if (Date.now() - obj.uploaded.getTime() > TOUCH_AFTER_MS) {
          ctx.waitUntil(touch(env, key, obj.uploaded.getTime()));
        }
        return new Response(obj.body, { status: 200 });
      }
      case "HEAD": {
        const obj = await env.CACHE.head(key);
        return new Response(null, { status: obj ? 200 : 404 });
      }
      case "PUT": {
        // Token-gated. The token is a Worker secret; only trusted CI events
        // (push-to-main, nightly) send it. Fork PRs get no secret -> read-only.
        if ((request.headers.get("Authorization") || "") !== `Bearer ${env.WRITE_TOKEN}`) {
          return new Response("forbidden\n", { status: 403 });
        }
        // Bazel sends Content-Length, so request.body is a fixed-length stream;
        // R2 streams it straight to storage (no buffering -> large blobs OK,
        // subject to Cloudflare's 100 MB request-body limit — see README).
        await env.CACHE.put(key, request.body);
        return new Response(null, { status: 200 });
      }
      default:
        return new Response("method not allowed\n", { status: 405 });
    }
  },
};
