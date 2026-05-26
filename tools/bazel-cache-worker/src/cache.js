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

export default {
  async fetch(request, env) {
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
        return obj ? new Response(obj.body, { status: 200 }) : new Response(null, { status: 404 });
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
