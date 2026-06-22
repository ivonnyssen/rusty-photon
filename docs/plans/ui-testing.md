# Plan: ui-htmx UI testing strategy

**Date:** 2026-06-14
**Branch:** `worktree-research-ui-testing`
**Parent design docs:** [`docs/workspace.md`](../workspace.md), [`docs/services/ui-htmx.md`](../services/ui-htmx.md), [`docs/skills/testing.md`](../skills/testing.md), [`docs/skills/pre-push.md`](../skills/pre-push.md), [`docs/plans/bazel-migration.md`](bazel-migration.md)
**Closest precedent:** [ADR-004 — Testing strategy for HTTP-client error paths](../decisions/004-testing-strategy-for-http-client-error-paths.md)

> **Status:** research-backed recommendation with a phased, **anticipatory** spike
> plan. The model (§2), the build-system stance (§3), the layer design (§4–§6),
> and the rejections (§11) are settled. The phasing (§10) is a proposal pending a
> review pass. Crate versions and external facts were verified 2026-06-14 and will
> drift — re-check before adopting a specific crate or pinning a workflow.

## 1. Background

`ui-htmx` (axum + [Maud] + [HTMX], server-rendered BFF, port 11120) is the only
browser-facing surface in scope. It is a **proof-of-concept** config UI today; the
intended trajectory is a full real-time astrophotography web interface (live mount
telemetry, exposure progress, image preview, an SSE activity stream — see
[`docs/plans/ui-design/`](ui-design/)). Leptos/WASM and the sentinel dashboard are
out of scope.

The current BDD suite (`services/ui-htmx/tests/bdd/`) spawns the real `ui-htmx` +
`dsd-fp2` binaries via `bdd_infra::ServiceHandle`, drives the BFF over HTTP with
`reqwest`, and asserts with `String::contains` on `world.last_body` plus a
hand-rolled `world.input_tag(name)` slicer. That proves the server *computed* a
value, but it is brittle and never proves the **UI behaves**:

- **False-positive-prone substrings.** `contains("disabled")`/`"invalid"` match the
  word anywhere; `input_tag()` (`world.rs:335-349`) panics on a malformed tag and
  mishandles attribute order / boolean attributes / self-closing tags.
- **The contract is asserted, never followed.** `polls_for_reconnection` checks the
  body *contains* `hx-trigger="every 1s"`, never issues the GET; `open_page_with_unlock`
  **fakes** the htmx GET out-of-band; `submit_form` reconstructs the POST body from a
  direct driver `config.get` rather than from the rendered form.
- **Real htmx is never executed.** The vendored `htmx.min.js` (v2.0.4,
  `services/ui-htmx/src/assets.rs`) is served but never loaded.

[Maud]: https://maud.lambda.xyz/
[HTMX]: https://htmx.org/

## 2. The model: two concerns, three proof obligations

`ui-htmx` is not an SPA — no client state machine, no virtual DOM, no bundler, **no
hand-written JavaScript**. Its interactivity is a thin, declarative contract, which
yields the identity that drives this whole plan:

```
browser-observable behavior = f( bytes the server sent , htmx.min.js , browser engine )
```

`htmx.min.js` is the same vendored file regardless of server OS, and the browser
engine is the **end-user's device** — something rusty-photon neither ships nor
controls. So behavior can differ across *server* OSes only if the *server's bytes*
differ. That decomposes the problem into three obligations, each with a different
cheapest-correct tool:

| # | Obligation | Question | Where it must run | Tool |
|---|---|---|---|---|
| **P1** | Output correctness | Does each OS's server emit the *right* markup + `hx-*` wiring? | full matrix **+ Pi** | `scraper` DOM assertions (§4) |
| **P2** | Output OS-invariance | Is the browser-relevant output *identical* across OSes? | full matrix **+ Pi** | `insta` byte-equivalence snapshots (§5) |
| **P3** | Output → behavior | Does real htmx *execute* it — swap lands, poll terminates, click works? | **one** environment | `thirtyfour` browser layer (§6) |

Composing them: **P3** establishes "this structure ⟹ this behavior" once; **P2**
establishes "every OS emits that structure"; transitively, behavior is correct on
every OS **without a browser on every OS**. **P1** independently proves the structure
is *correct* on each OS (P2 alone would pass if every OS were identically wrong).

**Every layer lives in the BDD scenarios** (cucumber-rs). The Gherkin `.feature`
files remain the **single source of truth**; one runner, one set of step
definitions. P1/P2 are added assertions on the same captured output; P3 reuses the
same Given/When/Then steps, dispatching to the browser when a driver is present.

## 3. Build-system reality: Bazel is going primary

This is load-bearing and corrects an earlier assumption that "keeping snapshots in
BDD defers the Bazel work." It does not:

- **BDD runs under Bazel today** on ubuntu/macOS/Windows (bazel.yml's explicit
  `--test_tag_filters=bdd` step) and in the bazel-coverage job. It is also run under
  Cargo (ubuntu/macOS required, Windows per-service matrix, Pi nightly).
- **Bazel is becoming the *required* per-PR gate** (bazel-migration Phase 7; Cargo
  demoted to a nightly safety net). Only Phase 7 remains; criteria are concrete
  (Phase 5: 2 weeks green + ≤20% wall-clock; Phase 7: 30 days required-Bazel).
- **Therefore the insta-under-Bazel wiring and the browser-under-Bazel story are
  required-path work, not deferrable.** Shadow Bazel BDD exercises any new snapshot
  on every PR from the first commit; a mis-wire reddens the shadow job (a visible
  cutover blocker).

**Browser under Bazel = a system-tool target, not hermetic.** There is no maintained
hermetic browser path for Rust (`rules_webtesting` archived 2025-11, never supported
Rust). But the repo already built everything needed: `test:ci --spawn_strategy=local`
(added for the rp:bdd park fix) bypasses the sandbox on all 3 OSes, neutralizing both
the localhost-WebDriver-connect block and system-browser invisibility; the
`OMNISIM_PATH`/`CONFORMU_PATH` `--test_env` **by-name** passthrough is how to forward
`FIREFOX_BINARY`/`GECKODRIVER_BINARY`; and bdd-infra's spawn/`$(rootpath)`/`bound_addr=`/
`kill_on_drop` machinery is directly reusable. So: **treat Firefox + geckodriver as
external system tools exactly like OmniSim/ConformU/ASTAP.** A dedicated `browser`
config must set `--spawn_strategy=local` itself (it is `test:ci`-only today) or it
"works in CI, fails locally." **Cargo-only escape hatch:** if the 3-OS browser spike
can't go green quickly, the browser layer stays cargo-only/system-dep (like the three
`bdd-infra` `requires-cargo` tests) with no migration-blocker status.

## 4. Layer A — server-contract (P1 `scraper` + §A thin request helpers)

> **Status (2026-06-21): implemented.** `scraper` 0.27 is a `ui-htmx` dev-dep;
> the BDD suite's Then-steps assert via CSS selectors ([`tests/bdd/dom.rs`]) and
> the request helpers ([`tests/bdd/world.rs`]) submit the rendered form, follow
> the rendered unlock link, and poll the reconnect endpoint by DOM — all with the
> `HX-*` header set. `input_tag()` and the `String::contains` assertions are
> gone. All 9 scenarios pass under Cargo. Layers B (§5) and C (§6) remain.

[`tests/bdd/dom.rs`]: ../../services/ui-htmx/tests/bdd/dom.rs
[`tests/bdd/world.rs`]: ../../services/ui-htmx/tests/bdd/world.rs

The everyday suite. Runs on every OS leg via the BDD suite, deterministically, no
browser.

- **`scraper` 0.27** — `[dev-dependencies]` on `services/ui-htmx`. Servo
  html5ever/selectors stack (the parser family browsers ship). Replace `input_tag()`
  and all `last_body.contains(...)` with CSS-selector assertions. **`!Send`
  discipline:** parse `&world.last_body` *inside* the synchronous Then-step, select,
  assert, drop — never store a parsed DOM in the `Send` World or hold it across
  `.await`; keep `last_body: String`.
- **Thin DOM-driven request helpers** (not an htmx simulator): on `reqwest` (already a
  `ui-htmx` dep) + `scraper` — *submit the rendered form* (read fields/hidden inputs
  + the `hx-post` URL from the actual HTML), *follow the unlock link* (its `hx-get`
  URL), *bounded reconnect-poll* (`follow_poll(sel, max_iters)`, no wall-clock sleep).
  These retire the out-of-band fakes and send htmx's **full request header set**
  (`HX-Request`, `HX-Target`, `HX-Trigger`, `HX-Trigger-Name`, `HX-Current-URL`) so
  the captured fragments match what the browser would receive.

## 5. Layer B — cross-OS output-equivalence (P2 `insta`)

Snapshot the **server response bytes** (full pages + `HX-Request` swap fragments)
captured by the existing non-browser BDD path. This is the cross-OS-comparable
artifact (the *browser DOM* is not — it only runs on one OS and a browser reserializes
it). For ui-htmx's current swap model (plain `outerHTML`, no OOB/`hx-select`/response-
header swaps/morph) the fragment bytes are byte-identical to what htmx swaps.

- **`insta` 1.48** (`redactions` + `filters` features; ≥1.46 brings `INSTA_PENDING_DIR`
  for hermetic builds). Snapshots **embedded in the BDD scenarios** (added assertions
  on `world.last_body` at key Then-steps), with **explicit names**
  (`assert_snapshot!("config_page__override_pinned", normalized(&world.last_body))`) —
  insta's auto-naming is murky inside a cucumber step.
- **External `.snap` files** under `services/ui-htmx/tests/snapshots/` (readable diffs;
  inline is the lower-Bazel-wiring fallback for small fragments).
- **Bazel wiring (required-path, mandatory up front):**
  - `data += glob(["tests/snapshots/**"])` on the `bdd` target so goldens reach the
    runfiles tree.
  - `env += {"INSTA_UPDATE": "no"}` — Bazel does **not** propagate `CI`, so force
    compare-only/fail-on-mismatch explicitly.
  - **Snapshot-path resolver** in the World/before-hook: `INSTA_WORKSPACE_ROOT` can't
    be a static `$TEST_SRCDIR` string in BUILD.bazel (no runtime interpolation at
    analysis time), so set it (or a per-thread `Settings::set_snapshot_path`) from
    `TEST_SRCDIR/TEST_WORKSPACE` at runtime — the `ppba-driver/tests/translations.rs`
    `locate_i18n_dir()` pattern applied to insta. (The `bdd` target's runtime chdir
    into the package dir may make the relative path resolve; verify empirically, keep
    the resolver as the fallback.)
- **Cross-OS storage = one committed golden, no special infra.** Requirements: pin
  `*.snap text eol=lf` in `.gitattributes` (the CRLF hazard, *not* fixed by insta
  filters); `add_filter` regexes to scrub OS-varying tokens (paths, ports,
  `ServerTransactionID`, temp dirs); compare-only in CI. **Updates are cargo-local**
  (`cargo insta review`/`accept`, commit) — never under Bazel/CI (read-only sandbox).

## 6. Layer C — real browser (P3 `thirtyfour`)

A **small** set (≈3–5 scenarios, plus the spike scenarios in §9) for behaviors only a
browser can prove.

- **`thirtyfour` 0.37.1** (active; MSRV 1.88 ≤ workspace 1.94.1; no longer depends on
  fantoccini; `WebDriver::managed` auto-downloads the *driver*, not the browser).
  Chosen over `fantoccini` (whose own CI is persistently red on macOS+Chrome) and
  `chromiumoxide` (open click/screenshot hang bugs).
- **Single engine, configurable, Firefox default.** Read the engine from
  `UI_TEST_BROWSER=firefox|chrome`; default Firefox (best arm64 story, and the engine
  swap is a few lines in the World — cross-engine stays a free future option).
  Cross-engine *behavioral* testing adds ~nothing for a no-custom-JS htmx app; the
  iPad/Safari concern is *responsive/visual*, a separate future track.
- **Gating: a cucumber tag + a runtime env var — NOT a cargo feature.** `@browser`
  scenarios are filtered out by `bdd.rs` unless `UI_BROWSER_TESTS=1` is set (the same
  closure that filters `@wip`). A cargo feature would be flipped on by the repo's
  `--all-features` runs, dragging browser flake into the required gate. `thirtyfour`
  is an always-compiled dev-dep (harmless; `--all-targets` compiles it anyway).
- **Advisory, with a nightly recording job** (§8).

## 7. The no-JS decision (resolved: abandon it)

The UI is **optional** — rusty-photon runs fully headless and the genuine recovery
path is **ssh + edit the config file**, strictly more capable than a degraded web
form. A whole-app no-JS guarantee is also incompatible with the future real-time UI.
So:

- **Abandon the no-JS fallback.** Remove the redundant progressive-enhancement
  affordances: `method`/`action` on the `<form>` (`pages/mod.rs:341`, keep `hx-post`)
  and `href` on the unlock/lock/retry `<a hx-get>` (convert to `<button hx-get>` for
  accessibility). Document the UI as **JavaScript-required**.
- **Keep + test the `HX-Request` full-page-vs-fragment branch** (`lib.rs` `is_htmx()`)
  — that is core htmx (direct navigation/refresh must return a full styled page), *not*
  a no-JS feature.
- This is a small **ui-htmx code cleanup** (`pages/mod.rs` + `docs/services/ui-htmx.md`),
  tracked separately from this plan doc.

## 8. Gating & CI

- **Default/required suite** (Cargo + Bazel): Layers A + B on every OS leg; `@browser`
  filtered out (env unset). Deterministic, no browser, full server coverage.
- **`@browser` nightly recording job** — a dedicated workflow modeled on `scheduled.yml`'s
  Miri job: `schedule` + `workflow_dispatch`, runs `@browser` (`UI_BROWSER_TESTS=1`,
  `UI_TEST_BROWSER=firefox`) against `main` on ubuntu, with a `timeout-minutes` cap
  (the ~30s thirtyfour wait-DSL default makes a broken stream burn time) and a
  `notify-on-failure` job (`actions/github-script`, `if: failure() && github.event_name
  == 'schedule'`) that **opens-or-updates a labeled tracking issue** — append-comment
  while open, never reopen once closed (the `#356` pattern). Lands in Phase 3 *with*
  the `@browser` scenarios (creating it earlier = a spurious failing issue).
- **Promotion:** `@browser` stays **advisory** initially. Promote to required only
  after a defined sustained-green window — and note the bytes≠DOM rule (§9): for
  behaviors only the browser can prove (OOB, response-header swaps, morph, SSE), the
  browser is the *sole* check, so those specifically may warrant required status once
  stable.
- **The Pi** runs Layers A + B (pure Rust) — never a browser (no official arm64-Linux
  Chrome until Q2-2026; snap-Chromium headless friction). The cross-OS browser result
  transfers to arm64 via P2.

## 9. Anticipatory spike plan — find the gotchas now

Decision (overriding a reactive default): **build anticipatory** — enough to robustly
verify the browser path *fully works* and exercises the system the way the full app
will, with test scenarios for the edge cases we're worried about, so issues surface
now. Build test code + minimal **test-only** fixtures; **zero production features**.

### Tier 0 — prove the path works (one target, the worst-case scenario)
One `@browser` target (cargo + Bazel, tag `browser`, `--spawn_strategy=local`, system
Firefox/geckodriver via `--test_env`, headless, geckodriver on an **ephemeral** port):
1. **Smoke** — load the config page in real Firefox; assert a *rendered* element
   (proves htmx.min.js + Firefox render, not just a session).
2. **Real htmx** — unlock-click → `outerHTML` swap (re-find after swap), every-1s
   poller **fires then terminates** (bounded DOM-poll, no `sleep()`, implicit-wait=0).
3. **Coverage invariant, asserted** — run under llvm-cov; assert non-zero BFF coverage
   with the browser in the loop; reverse the teardown order to confirm it drops (locks
   `quit() → stop ui → stop driver` in with a test).
4. **Worst-case** — a step that **deliberately panics** mid-session; external `pgrep`
   asserts zero geckodriver/firefox survive, the BFF still flushed `.profraw`, and a
   screenshot + page-source landed at an absolute path. One scenario exercises
   orphan-cleanup, teardown ordering, no-async-Drop, artifact-before-quit, and
   chdir-safe paths at once.
5. **Go/no-go on the 3-OS Bazel matrix + Cargo;** cargo-only escape hatch (§3) if
   macOS/Windows can't go green fast.

### Tier 1 — bytes≠DOM future-htmx edge cases (cheap fixtures)
A feature-gated, test-only `/fixtures/*` route set (ships nothing) the `@browser`
scenarios drive, each proving the harness can **observe a divergence P1/P2/§A cannot**:
- **`hx-swap-oob`** — main fragment + a sibling OOB toast → assert a *second region*
  updated (+ negative: missing OOB target → silent drop).
- **`HX-Retarget`/`HX-Reswap`** — set the header on an **unchanged body** → assert the
  swap landed elsewhere/differently AND the body bytes are identical (the concrete
  "P1/P2 are insufficient" demo; add a §A header-presence tripwire).
- *(optional)* **`HX-Redirect`/`HX-Push-Url`** → assert `window.location`/`history`
  changed (proves navigation/history observability).

### Tier 2 — the streaming spike (the #2 infra risk, made real)
A **minimal test-only axum `Sse`** endpoint (e.g. `#[cfg(feature = "test-sse")]`)
emitting ≥2 named events on a timer + a fixture page with `hx-ext=sse` and two
`sse-swap` targets:
- Assert **both targets update from one connection** (empirically confirms async-*pushed*
  DOM updates are observable — thirtyfour's wait DSL polls the live DOM).
- **Streaming teardown/coverage proof** — the decisive one: an open SSE connection
  **blocks axum graceful shutdown** (axum issue **#2673**: unlike WebSockets, an SSE
  stream never closes on the shutdown signal; KeepAlive doesn't end it) → 5s SIGKILL →
  no `.profraw` → coverage silently 0. Crucially, **SSE removes §5.4's in-process
  escape hatch** (no `world.mcp_client = None` equivalent — the connection is held by
  the out-of-process browser), so `driver.quit()` is the *only* lever and **must
  precede** `ServiceHandle::stop()`. Assert: after `quit()`, `stop()` returns within
  the 5s grace with **SIGKILL count 0** (temporarily `eprintln!` the bdd-infra SIGKILL
  log, per §5.4's detection technique) and a non-empty `.profraw`.
- *(optional)* two `sse-connect` URLs → probe the ~6-per-host connection-limit failure.

### Tier 3 — reserve the seam, don't build
- **Morph swaps** (`idiomorph`) — irreducibly browser-only (focus/caret/node-identity
  survive); needs the real extension; flag as a definite future `@browser` obligation.
- **`hx-select`/`hx-preserve`/`hx-boost`** (incl. the no-JS leg, now moot per §7) —
  fixture-able later.
- The full multi-stream telemetry strip and the production scenario suite.

## 10. Cross-cutting gotchas to design against

- **Firefox orphans on non-graceful exit.** geckodriver only cleanly quits Firefox on
  SIGTERM in **Firefox 152**; ubuntu-latest ships **Firefox 151 + geckodriver 0.37**
  (bugzilla 1430064). Any panic/timeout that bypasses `quit()` orphans Firefox →
  explicit `quit()` + a kill-the-tree reaper.
- **snap-Firefox** breaks geckodriver's `/tmp` profile → non-snap Firefox
  (`browser-actions/setup-firefox` / Mozilla tarball) or `--profile-root`/`TMPDIR`.
- **Teardown ordering** = `driver.quit() → stop ui → stop driver` (the §5.4 rule;
  SSE/keep-alive makes it load-bearing — see Tier 2 / axum #2673). The current
  `bdd.rs` `.after()` hook stops `ui` first and holds no browser handle — wrong by
  construction once a browser handle lands.
- **The chdir** (`__bdd_bazel_chdir`) absolutizes only `*_BINARY`/`COVERAGE_DIR`;
  extend the absolutize set to the driver binary, Firefox profile dir, and the
  failure-artifact dir, or they resolve wrong under Bazel / lose artifacts.
- **geckodriver on an ephemeral port** (not the 4444 default) to avoid collisions.
- **Fresh session per scenario** (default; matches the suite's isolation ethos —
  makes robust teardown non-negotiable).
- **Headless, fail-loud** — set `-headless`/`MOZ_HEADLESS=1`; a missed flag reads as a
  timeout (mistakable for the rp:bdd "park").
- **thirtyfour has no async Drop** — always `quit().await` explicitly; never rely on
  Drop (it blocks the executor and can deadlock the after-hook).
- **Snapshot-once browser assertions race** — always poll (wait-for-condition), never
  snapshot the DOM once.

## 11. Explicitly rejected & deferred

- **Playwright (re-evaluated 2026-06-14, with everything learned).** Playwright is the
  **stronger browser engine** and cleanly retires our two ugliest gotchas — it ships
  its own version-pinned browser builds (solves snap-Firefox; mostly solves the
  FF-151/152 orphan trap) and its lazy auto-retrying locators dissolve the htmx
  stale-element flake — with best-in-class trace/video triage. It is **rejected for
  this repo now** because: (a) **no official/v1.0 Rust binding** — only
  `padamson/playwright-rust` (pre-1.0, single-maintainer); (b) **every** Rust path
  requires a **runtime Node.js + `npx playwright install`**, injecting a Node toolchain
  + npm-sourced browser downloads into a Rust-only, Cargo.toml/crate_universe-single-
  source-of-truth, Bazel-hermetic workspace; (c) the JS-runner routes (`playwright-bdd`,
  `cucumber-js`) **fork the source of truth** (duplicate every step def in TS + reimplement
  ~600 lines of `ServiceHandle` in Node + dual-own the `.feature` files) and the browser
  layer no longer feeds the Rust llvm-cov gate; and (d) the gotchas that gate the
  *required* check — coverage-zeroing on non-graceful service shutdown (a server-side
  property) and reaping on non-graceful exit (Playwright can still leak zombies,
  upstream #34190) — are **not** solved by Playwright. With our planned mitigations
  (pinned non-snap Firefox + reaper), the net delta shrinks to *automatic* stale-element
  handling + better trace tooling — a convenience win, not a correctness gap. `thirtyfour`
  is pure Rust, zero Node, reuses the entire harness, treats the browser as a system
  tool, and keeps `.feature` single-source. **Flip conditions:** (i) an official or
  v1.0+ Node-free Rust binding lands (watch MS #34213); (ii) browser-side flake becomes
  the dominant maintenance cost; (iii) cross-engine WebKit/Safari becomes a real
  requirement; (iv) the team adopts Node for other reasons. (thirtyfour 0.37's WebDriver
  BiDi is narrowing the engine gap without the Node cost.)
- **In-process unit-test snapshots** — rejected: a second test re-rendering the page
  duplicates the behavioral contract and cuts against BDD-as-source-of-truth. Snapshots
  ride the BDD scenarios (§5).
- **Cross-host (browser on a different machine than the server)** — right goal, wrong
  mechanism. The cross-platform obligation is a server-side byte property (P2),
  provable server-side; isolated CI runners make cross-host networking fragile for zero
  added behavioral coverage. Remote-origin/0.0.0.0/TLS postures are deferred in ui-htmx
  anyway — test them as an explicit remote-access scenario if/when they ship.
- **A general hand-rolled htmx *simulator*** — owning a model of `hx-swap`/`hx-target`/
  `HX-*` semantics is "testing your simulator," with a re-validate-on-every-htmx-bump
  tax. Keep only the thin request helpers (§4); route client-transformation behavior to
  the real browser (§6).
- **Cross-engine behavioral matrix / a browser on the Pi** — see §6 / §8.
- **T1 DOM-parser alternatives** (`tl`, `soup`, `kuchikiki`, `lol_html`, `visdom`, raw
  `html5ever`) — unmaintained / silently drops "invalid" tags (`tl`) / wrong shape /
  too niche. `scraper` dominates.
- **A whole-app no-JS guarantee** — abandoned (§7).

## 12. Open questions (residual)

1. **Cross-engine appetite** — confirm single-engine Firefox is acceptable indefinitely
   (cross-engine = a future visual/device-lab track, not the behavioral smoke).
2. **`@browser` required-gate timing** — when (if) to promote from advisory, and whether
   to promote *per-behavior* for the bytes≠DOM cases first.
3. **Inline vs external `.snap`** — external recommended; revisit if the Bazel resolver
   proves fiddly for small fragments.
4. **htmx surface growth** (the §9 trigger list) — staying within today's simple
   envelope keeps the server-bytes proxy faithful; adopting OOB/`hx-select`/response-
   header swaps/morph/SSE is the signal to lean on §6 over §4.

## 13. Key files

- `services/ui-htmx/tests/bdd.rs` — entry (`bdd_main!`, `.after()` teardown seam — fix the ordering).
- `services/ui-htmx/tests/bdd/world.rs` — `UiWorld`, `last_body`, `input_tag`, the HTTP helpers.
- `services/ui-htmx/tests/bdd/steps/config_page_steps.rs` — substring Then-steps to upgrade.
- `services/ui-htmx/tests/features/config_page.feature` — canonical spec (currently untagged).
- `services/ui-htmx/BUILD.bazel` — the `bdd` rust_test target (`data`/`env` edits for `.snap` + INSTA_UPDATE).
- `services/ui-htmx/Cargo.toml` — dev-deps (`scraper`, `insta`, `thirtyfour`).
- `services/ui-htmx/src/pages/mod.rs` — htmx attribute surface (+ the §7 no-JS-affordance cleanup).
- `services/ui-htmx/src/lib.rs` — router, `is_htmx()` branch, the `/fixtures/*` + `/test/stream` seams.
- `services/ui-htmx/src/assets.rs` — vendored htmx 2.0.4 (pin/document the version).
- `crates/bdd-infra/src/lib.rs` — `ServiceHandle`, `bdd_main!`, `__bdd_bazel_chdir`, `parse_bound_port` (reuse for a geckodriver handle; extend the chdir absolutize set).
- `services/ppba-driver/tests/translations.rs` — the `TEST_SRCDIR/TEST_WORKSPACE` runfiles resolver template for the `.snap` path under Bazel.
- `.bazelrc` — `test_tag_filters`; the `browser` config (`--spawn_strategy=local` + `--test_env` by-name).
- `.gitattributes` — add `*.snap text eol=lf`.
- `.github/workflows/scheduled.yml` — the Miri `notify-on-failure` pattern to mirror for the `@browser` nightly.

## 14. References

- [`docs/services/ui-htmx.md`](../services/ui-htmx.md) — service design + config-action wire contract.
- [`docs/skills/testing.md`](../skills/testing.md) — test pyramid, BDD conventions, §5.4 (drop streaming clients before stop), §6.7 (mock strategy).
- [`docs/plans/bazel-migration.md`](bazel-migration.md) — migration phases, cutover criteria, BDD-under-Bazel, runfiles/test-data handling.
- [`docs/skills/pre-push.md`](../skills/pre-push.md) — CI quality gates.
- [ADR-004](../decisions/004-testing-strategy-for-http-client-error-paths.md) — testing-strategy precedent.
- External (verified 2026-06-14): `scraper` 0.27, `insta` 1.48 (+`INSTA_PENDING_DIR` ≥1.46), `thirtyfour` 0.37.1, axum #2673 (SSE blocks graceful shutdown), Firefox 152 / bugzilla 1430064 (geckodriver SIGTERM quit), `rules_webtesting` archived (no Rust), Playwright `padamson/playwright-rust` pre-1.0 + MS #34213 (Node-free unimplemented).
