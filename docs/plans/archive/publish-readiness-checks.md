# Publish-readiness checks for the dual-homed FFI crates

**Status: COMPLETE (archived 2026-07-12).** Implemented on
`feature/publish-readiness-checks`; the verification machinery (explicit
per-crate MSRVs, `scripts/verify-publishable-crate.sh`,
`.github/workflows/publish-readiness.yml`) is live and has run green on every
nightly `publish-readiness.yml` cron since 2026-06-22 (10/10 most recent runs
verified). It remains a **prerequisite** for the first crates.io publish
deferred by [`vendor-qhyccd-rs.md`](../vendor-qhyccd-rs.md) and
[`vendor-zwo-rs.md`](../vendor-zwo-rs.md), neither of which has published yet.
**Author:** drafted 2026-06-22.
**Scope:** the **four dual-homed FFI crates only** — `qhyccd-rs` + `libqhyccd-sys`
(ADR-009) and `zwo-rs` + `libzwo-sys` (ADR-010). Nothing else in the workspace is
touched; every other member keeps inheriting the unified `1.94.1` MSRV.

## Motivation

These four crates are authored by us but **published to crates.io independently**
(dual-home). A published library owes its consumers two guarantees the rest of the
workspace does not:

1. **An honest MSRV** — the `rust-version` it advertises actually builds.
2. **Honest dependency lower bounds** — the crate builds with the *minimal*
   semver-compatible versions of its declared dependencies, not just the newest.

The standalone repos verified both (qhyccd-rs declared `1.68.0` and had a
`cargo +nightly update -Zminimal-versions` job; the `#to make Zminimal happy`
floor-bump deps were its hand-maintained artifacts). **Vendoring silently dropped
all of it**: the crates were switched to the workspace's unified `1.94.1` MSRV and
the workspace's single newest-version `Cargo.lock`, and the standalone
`cargo-semver-checks` / docs.rs jobs were lost too. Publishing in that state would
ship a `1.94.1` MSRV for a crate that used to support `1.68.0` — a 26-minor
regression for downstream consumers — with unverified dependency floors.

## The core problem: the workspace masks both properties

Neither property can be verified **in** the workspace:

- **MSRV floor.** The root `[profile.dev] debug = "line-tables-only"` requires Rust
  ≥ 1.71, so an in-workspace `cargo +1.68 check -p libqhyccd-sys` (its floor is
  1.68) fails at *profile parsing* before the crate compiles. Profiles never travel
  to downstream consumers, so this constraint is a workspace artifact, not a real
  consumer limit.
- **Minimal versions.** `cargo update -Zminimal-versions` rewrites the **whole**
  shared `Cargo.lock`; the rest of the workspace needs newest deps and won't build
  on minimums. It cannot be scoped to four crates in place.
- **rail.** `.config/rail.toml` had `enforce_msrv_inheritance = true` — it actively
  required every member to inherit the workspace MSRV.

The only faithful way to check a published crate is **out of the workspace**, the
way a crates.io consumer sees it.

## Decisions (settled with the user, 2026-06-22)

| Decision | Choice | Why |
|---|---|---|
| minimal-versions flavor | **`-Z direct-minimal-versions`** (+ MSRV-aware resolver) | Floors only *direct* deps; transitive deps resolve normally. Low-maintenance, and let us **delete** the brittle `#to make Zminimal happy` phantom deps. |
| CI gating | **Nightly, non-blocking** (`schedule` + `workflow_dispatch`) | A minimal-versions break usually comes from an *upstream* release, not the PR under review — gating every PR on it would red-bar unrelated work (same rationale as conformu/pi-nightly). |
| Adjacent scope | **Also restore `cargo-semver-checks` + docs.rs build** | The standalone had both; they are part of "ready to publish" and were lost on vendoring. |
| MSRV declaration | **Explicit per-crate `rust-version`** (not `workspace = true`) | A `workspace = true` MSRV flattens to `1.94.1` on publish. Only an explicit lower value publishes a lower floor. |

## The mechanism

### 1. Explicit, honest per-crate MSRVs

| Crate | MSRV | Bound by |
|---|---|---|
| `qhyccd-rs` | **1.85.0** | `simulation` pulls rand 0.10 (MSRV 1.85) + its chacha20/getrandom chain; the base build is held to 1.81 by derive_more 2.1. Was 1.68.0 until #392 moved this crate onto rand 0.10 / derive_more 2.1 — the check re-floored it. |
| `libqhyccd-sys` | **1.68.0** | dependency-free hand-written FFI; unaffected by #392, so it keeps the low 1.68.0 floor (it no longer shares qhyccd-rs's value). |
| `libzwo-sys` | **1.70.0** | its `bindgen 0.72` build-dep (MSRV 1.70.0); verified green |
| `zwo-rs` | **1.87.0** | `src/camera.rs` calls `u32::is_multiple_of` (stabilised in Rust 1.87.0). Replace it with `% n == 0` to drop the wrapper toward libzwo-sys's 1.70.0. |

The `find` job reports whether any of these can go lower (see "lowest possible").

### 2. `scripts/verify-publishable-crate.sh`

Per crate family, it:
1. **Copies the family out of the workspace** (escaping the `Cargo.lock` and the
   `profile.dev` MSRV-raiser) and inlines the few `{ workspace = true }` deps the
   way `cargo publish` would. (`cargo package` is *not* usable: zwo's `libzwo-sys`
   is unpublished, so packaging the wrapper can't resolve it from crates.io.)
2. Generates a **direct-minimal-versions** lockfile on nightly **with the
   MSRV-aware resolver**:
   `CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback cargo +nightly generate-lockfile -Z direct-minimal-versions`.
   The fallback resolver is the load-bearing detail — without it, direct-minimal
   leaves *transitive* deps at newest, and a newest transitive dep can demand a
   higher Rust than our floor (dogfooding hit exactly this: `rayon 1.10` → `rayon-core
   1.13` needs 1.80). The fallback resolver caps transitives at MSRV-compatible
   versions (rayon-core 1.12.1), so the low floor holds.
3. `cargo +<msrv> hack --feature-powerset check --locked` with the family's
   `*_SKIP_NATIVE_LINK=1` env, so the check needs **no SDK** (and, for zwo, only
   libclang for bindgen — not the SDK binary). It verifies the sys crate standalone
   (so *its* direct deps — zwo's `bindgen` — are floored honestly) then the wrapper.

A `find` mode runs `cargo msrv find` instead, reporting the lowest declarable MSRV.

### 3. `.github/workflows/publish-readiness.yml`

Nightly + `workflow_dispatch` (+ paths-filtered PR/push on the workflow and script
themselves). A `plan` job discovers the families via
`[package.metadata.publish-readiness]` (the same dynamic-discovery pattern as
`[package.metadata.conformu]`/`[package.metadata.miri]`), then matrixes:
`msrv-minimal-versions` (the script), `semver-checks`, `docs` (docs.rs build), and
an advisory `find`. A `notify-on-failure` job opens/updates a `publish-readiness`
tracking issue on scheduled red.

### 4. Discovery metadata

Only the two wrapper crates carry the marker — which is what scopes the checks to
exactly the four FFI crates:

```toml
[package.metadata.publish-readiness]
sys-crate = "libqhyccd-sys"            # the nested FFI sub-crate to co-verify
skip-link-env = "QHYCCD_SKIP_NATIVE_LINK"
needs-libclang = false                 # true for zwo (bindgen)
```

## Proven results (dogfood, 2026-06-22)

- **qhy**: `libqhyccd-sys` builds on **1.68.0**; `qhyccd-rs` builds on **1.85.0**
  with a direct-minimal-versions lockfile across the feature powerset (incl.
  `simulation` → rand/rayon). The three `#to make Zminimal happy` phantom deps were
  confirmed unused and **removed** — they *break* direct-minimal-versions
  (`tracing-attributes = 0.1.28`, a direct dep, conflicts with `tracing 0.1.44`'s
  `^0.1.31`).
- **zwo**: `libzwo-sys` builds on **1.70.0**; `zwo-rs` builds on **1.87.0**. The
  check **caught that the declared 1.70.0 was a lie** — `zwo-rs` uses
  `u32::is_multiple_of` (Rust 1.87.0). Exactly the class of bug this exists to find.

### Re-verified after merging #392 (rand 0.10 + house-conventions), 2026-06-22

Pulling #392 into the branch exercised the check against a real upstream change,
and it caught **three** publishability regressions before they could ship:

1. **qhyccd-rs MSRV re-floored 1.68 → 1.85.** #392 moved the crate onto rand 0.10
   (MSRV 1.85, under `simulation`) and derive_more 2.1 (MSRV 1.81, base). The
   declared 1.68.0 became a lie; `rust-version` was raised to the honest 1.85.0.
2. **`derive_more` floor too loose.** The workspace pinned `derive_more = "2.0"`,
   but qhyccd-rs uses the `eq` derive feature, which was added in **2.1.0** (absent
   in 2.0.0/2.0.1). direct-minimal-versions floored it to 2.0.0 and failed to
   resolve `eq`. Tightened the workspace pin to `2.1`.
3. **`rand` floor too loose.** `rand = "0.10"` floors to 0.10.0, whose feature
   table references `rand/getrandom` as a `dep:`-only dependency (no implicit
   feature), so the minimal-versions sim build fails to resolve. Tightened to
   `0.10.1`.

Both floor fixes are zero-behaviour (the in-workspace lockfile already resolved to
derive_more 2.1.1 / a newer rand). The inliner also gained support for inherited
deps that carry `features`/`optional` keys (`{ workspace = true, features = [...] }`),
which #392 introduced.

## Reconciliation with in-workspace checks

The lower, divergent MSRVs would otherwise trip checks that assume one uniform
workspace MSRV — both are reconciled here:

- **rail.** `.config/rail.toml`: `enforce_msrv_inheritance = false` (the four crates
  diverge by design; every other member still inherits voluntarily).
- **`check.yml` `ubuntu / msrv` job.** It runs `cargo msrv verify` per member
  in-workspace, which CANNOT pass for these four (the very `profile.dev` ≥ 1.71 /
  shared-lockfile problem this plan exists to dodge). The job now **skips** them —
  the wrapper plus its `sys-crate`, discovered from
  `[package.metadata.publish-readiness]` (the same marker the nightly workflow keys
  on) — and logs which crates it skipped. Their floors are verified out-of-tree
  here instead.

One CI nuance worth recording: `cargo-docs-rs` has no `taiki-e/install-action`
manifest (unlike `cargo-hack` / `cargo-semver-checks`), so the **docs** job installs
it with `cargo install cargo-docs-rs --locked` rather than the action shorthand.

## Publish gating

The first crates.io publish (deferred by the two vendoring plans) is now gated on a
**green nightly publish-readiness run** for the crate being released. Recorded in
[`crates/qhyccd-rs/RELEASING.md`](../../../crates/qhyccd-rs/RELEASING.md) and ADR-010's
release runbook.

## "Lowest possible", maintained

"Lowest possible" is kept *operationally*, not as a one-time number: the advisory
`find` job reports each crate's true minimum every night, so the declared
`rust-version` can be ratcheted down (or must rise when a dep/std-API bumps). The
first ratchet candidate is already flagged: rewrite `zwo-rs`'s `is_multiple_of`
to reach ~1.70.

## Risks & open items

| Risk / item | Note |
|---|---|
| `zwo-rs` MSRV is 1.87 (high) because of one `is_multiple_of` call | One-line refactor to `% n == 0` drops it to ~1.70. Owner decision — not done here to avoid silently changing camera logic. |
| direct-minimal-versions needs the MSRV-aware resolver to hold a low floor | Encoded in the script (`CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback`); documented above so it is not "simplified" away. |
| `cargo-semver-checks` has no baseline for unpublished crates | First-publish case — reported as a warning, not a failure. Becomes meaningful from the 2nd release. |
| A future `{ workspace = true, features = [...] }` dep | The script's inliner fails loudly (it only inlines plain `{ workspace = true }`); extend it if that shape appears. |
| Upstream release breaks minimal-versions on an unrelated night | Non-blocking by design; the tracking issue surfaces it before it blocks a publish. |
