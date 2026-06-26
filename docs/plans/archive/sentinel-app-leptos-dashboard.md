# Sentinel Leptos/WASM Dashboard (`sentinel-app`)

**Status: OBSOLETE (archived 2026-06-14).** A `sentinel-app` Leptos/WASM
crate was scaffolded as an alternative reactive frontend for the sentinel
monitoring dashboard, never wired into the running service, formally
abandoned on 2026-05-24 (Bazel-migration Phase 4 dropped), and removed
from the workspace on 2026-06-14. The dashboard direction is the
hand-rolled, server-rendered HTML already shipping in `sentinel`; the
config-UI direction is [`ui-htmx`](../../services/ui-htmx.md). This note
preserves the design intent and the path back, since there was never a
dedicated plan doc for the effort.

## What it was

`services/sentinel-app/` was a standalone Leptos 0.8 crate
(`crate-type = ["cdylib", "rlib"]`) holding a reactive web UI for the
sentinel observatory monitoring dashboard:

- `src/lib.rs` — crate root; re-exported `App`, plus a `hydrate()` entry
  point gated on `cfg(all(feature = "hydrate", target_arch = "wasm32"))`
  calling `leptos::mount::hydrate_body(App)`.
- `src/app.rs` — root `#[component] App`, composing `MonitorTable` +
  `HistoryTable`.
- `src/components/{monitor_table,history_table,status_badge}.rs` — Leptos
  components using `Resource` + `Suspense` + `gloo_net` to fetch
  `/api/status` and `/api/history`.
- `src/api.rs` — two serde DTOs (`MonitorStatusResponse`,
  `NotificationHistoryResponse`) mirroring the sentinel JSON responses.

Feature sets: `hydrate` (`leptos/hydrate` + `wasm-bindgen` + `web-sys` +
`gloo-net`, for `wasm32-unknown-unknown` client-side hydration) and `ssr`
(`leptos/ssr` + `leptos_meta/ssr` + `leptos_router/ssr`, for native
server-side rendering). `default = []`, so a plain `cargo build` produced
a native rlib only — no WASM.

The intended wiring lived only in the workspace-root
`[[workspace.metadata.leptos]]` block (a `cargo-leptos` build target named
`sentinel-dashboard`, `bin-package = "sentinel"`, `lib-package =
"sentinel-app"`, `site-addr = "127.0.0.1:11114"`).

## Why it was abandoned

- **Never wired in.** Nothing depended on `sentinel-app`: the resolved
  `sentinel` package had no dependency on it (zero reverse deps in
  `Cargo.lock`), `services/sentinel/` imported neither `leptos` nor
  `sentinel_app`, and `services/sentinel/BUILD.bazel` had no edge to it.
  The only link was the `cargo-leptos` metadata block, which no compiled
  artifact reads — and whose `site-addr` port (11114) actually collided
  with the live dashboard's port.
- **A working dashboard already exists.** The served dashboard is
  hand-rolled HTML built with `format!()` in
  `services/sentinel/src/dashboard.rs`, refreshed by a vanilla `fetch()`
  loop every five seconds — no Leptos, no WASM. See
  [`docs/services/sentinel.md`](../../services/sentinel.md).
- **Formally dropped.** The Bazel migration recorded "Leptos /
  `sentinel-app` WASM: abandoned … Phase 4 is dropped, not deferred"
  (2026-05-24). See [`bazel-migration.md`](bazel-migration.md).
- **The UI direction moved.** Server-rendered HTML (the `ui-*` family,
  starting with [`ui-htmx`](../../services/ui-htmx.md)) is the chosen
  approach for browser UIs; a Leptos/WASM `ui-leptos` remains only a
  hypothetical future member of that naming scheme, distinct from this
  crate.

Carrying the crate, its `leptos`/`wasm` dependency subtree, and the
"looks-wired" `cargo-leptos` metadata around made the docs and code
misleading about the project's actual direction, which is why it was
removed rather than quarantined.

## What was removed (2026-06-14)

- The entire `services/sentinel-app/` directory (8 files).
- The `services/sentinel-app` workspace member and the whole
  `[[workspace.metadata.leptos]]` block in the root
  [`Cargo.toml`](../../../Cargo.toml).
- The `leptos` family and its Leptos-only `wasm`/`web-sys`/`gloo` subtree
  from `Cargo.lock` and `MODULE.bazel.lock` (regenerated). Some
  `wasm-bindgen` lock entries remain — they are transitive via
  `chrono`/`getrandom`/`uuid` targeting WASM, unrelated to Leptos.
- Documentation references that presented Leptos as the dashboard
  direction (`README.md`, `docs/workspace.md`, `docs/services/sentinel.md`,
  `docs/services/rp.md`, `docs/skills/pre-push.md`, `docs/plans/i18n.md`).

## Reviving it

The crate was introduced inline with the sentinel service in **PR #32**
(commit `b3c574e`, 2026-02-23); its `BUILD.bazel` was added in **PR #82**.
To resurrect a WASM dashboard, restore the crate from that history, re-add
the workspace member and a `cargo-leptos` target (on a non-colliding
port), re-pin Cargo and Bazel, and re-open Phase 4 of the Bazel migration.
The `wasm_bindgen` / `@platforms//cpu:wasm32` / hydrate+ssr Bazel approach
is sketched in [`bazel-migration.md`](bazel-migration.md).
