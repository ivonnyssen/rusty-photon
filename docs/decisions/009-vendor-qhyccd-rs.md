# ADR-009: Vendor `qhyccd-rs` + `libqhyccd-sys` into the workspace (dual-homed)

## Status

Accepted (2026-06-17). **Implemented** Phases 1–2 on `worktree-qhy-camera`
(commits `81a4d42` Cargo vendoring, `0325417` Bazel two-variant); Phase 3 (docs)
in progress; the standalone-repo archive is deferred until the first
publish-from-subdir proves the release path. Tracked by
[`docs/plans/vendor-qhyccd-rs.md`](../plans/vendor-qhyccd-rs.md).

Supersedes the **variant-selecting** role of the interim Bazel fix on
`worktree-qhy-camera` (a test-only `qhyccd-rs = { features = ["simulation"] }`
dev-dep + `simulation` `crate_features`; see
[`docs/plans/bazel-migration.md`](../plans/bazel-migration.md),
"External-crate non-default features for tests"). The real/sim split is now done
by two first-party BUILD variants; the dev-dep is **retained** only to keep
simulation's optional deps (`rand`/`rayon`) resolved into `@cr` (Phase 2 spike).

## Context

`qhyccd-rs` (`=0.1.9`) and its FFI sub-crate `libqhyccd-sys` (`0.1.4`) are
authored by us (`ivonnyssen/qhyccd-rs`) but consumed as **external** crates.io
dependencies. The `qhy-camera` service links the QHYCCD SDK through them
(`libqhyccd-sys` declares `links = "qhyccd"`, hand-written FFI in `lib.rs` — no
bindgen — and a per-OS `build.rs` emitting `static=qhyccd` + libusb directives).
Three frictions follow from the external status:

1. **A `[patch.crates-io]` git override.** `libqhyccd-sys 0.1.4` on crates.io was
   cut before the macOS link fix, so the root `Cargo.toml` pins it to git rev
   `d84f867…`. Every binding fix means an upstream commit + a rev bump.
2. **No real/sim parity under Bazel.** `crate_universe` resolves **one** feature
   set per external crate and ignores a Bazel target's `crate_features`, so we
   cannot build a real-SDK *and* a `simulation` `qhyccd-rs` from `@cr`. The interim
   workaround flips the single resolved variant to `simulation` via a dev-dep — so
   the (never-deployed) Bazel **production** binary is also simulated, and the real
   `Sdk::new()` FFI path is never compiled under Bazel.
3. **Repin/publish churn.** Any binding change needs a crates.io publish (or rev
   bump) + `CARGO_BAZEL_REPIN`, rather than an in-tree edit.

This mirrors ADR-001 Amendment A (the cfitsio purge) and ADR-008 (zwo-camera's
native SDK FFI) as a native-link dependency question. `zwo-rs`/`libzwo-sys` have
the same shape and will likely follow the same path later (they add bindgen).

## Decision

Move `qhyccd-rs` and `libqhyccd-sys` into the workspace as **first-party,
nested, dual-homed** members.

### Nested layout

`crates/qhyccd-rs/` holds the crate, with `libqhyccd-sys/` nested inside it —
preserving the upstream `libqhyccd-sys = { version = "0.1.4", path =
"libqhyccd-sys" }` dependency verbatim. Both are added to `[workspace] members`.
(Flat `crates/qhyccd-rs` + `crates/libqhyccd-sys` was the alternative; nested
wins on minimal churn and on keeping the sys-crate visibly part of qhyccd-rs.)

### Dual-home (keep publishing to crates.io)

The monorepo becomes the **canonical development home** and the single source of
truth; we keep publishing both crates to crates.io **from the vendored subdirs**
for outside consumers. This works cleanly because the inter-crate dep already
carries both a `version` and a `path`: `cargo publish` rewrites the path dep to
the crates.io `version`, while local + Bazel builds use the `path`. To stay
publishable, the vendored manifests **keep independent explicit `version` /
`edition` / `license` / `authors` / `description` / `repository`** (NOT
`*.workspace = true` — they release on their own cadence) and retain their
`CHANGELOG` / `README` / `LICENSE-*`. The standalone `ivonnyssen/qhyccd-rs` repo
is **hard-archived** (read-only on GitHub, README pointing at the monorepo) once
the first publish-from-monorepo is verified — the monorepo is the sole source of
truth thereafter.

### Two Bazel variants (the parity win)

With the crates first-party we own their `BUILD.bazel`, so we apply the existing
first-party variant pattern (`crates/rp-plate-solver` ↔ `rp-plate-solver_with_mock`):

- a `cargo_build_script` for `libqhyccd-sys` (the repo's **first** first-party
  build-script crate — modelled on what `crate_universe` generates for it today,
  so the `/usr/local/lib` + `static=qhyccd` link is a re-expression of a proven
  build, not a new one), plus its `rust_library`;
- `qhyccd-rs` (real SDK, no features) **and** `qhyccd-rs_sim` (`testonly = True`,
  `crate_features = ["simulation"]`, same `crate_name`).

`qhy-camera`'s production targets depend on the real `qhyccd-rs`; its sim / BDD /
ConformU targets depend on `qhyccd-rs_sim`. `testonly = True` makes Bazel reject
any production target that accidentally links the sim variant.

## Consequences

- **The `[patch.crates-io]` git override is deleted.** SDK-binding fixes are
  in-tree edits; no rev bump.
- **Real/sim parity under Bazel:** the production `qhy-camera` Bazel binary links
  the **real** SDK again (and compiles the real `Sdk::new()` FFI path) via
  `//crates/qhyccd-rs:qhyccd-rs`, while BDD / ConformU use the `testonly`
  `:qhyccd-rs_sim`. The Phase 2 spike showed `rand`/`rayon` **do** leave `@cr`
  without the interim `simulation` dev-dep (`:qhyccd-rs_sim` then fails to compile),
  so it is **kept with a narrowed role** — keeping simulation's optional deps in
  `@cr`, not selecting the SDK variant.
- **Orphan `@cr` `libqhyccd-sys`:** because the vendored path dep keeps a `version`
  for publish, crate_universe still materializes `@cr__libqhyccd-sys-0.1.4`. It is an
  orphan (everything resolves the workspace-member edge), so it is never fetched or
  built — harmless dead weight, documented rather than worked around.
- **First first-party `cargo_build_script`** in the repo. De-risked: the same
  `build.rs` already runs under Bazel via `crate_universe` today. No libclang
  needed (hand-written FFI). SDK provisioning (install action, `/usr/local/lib`,
  `GITHUB_WORKSPACE` forwarding in `.bazelrc`) is unchanged.
- **A small convention deviation:** these two members keep explicit, independent
  versions/metadata rather than `*.workspace = true`, because they are
  independently published. Documented here and in the plan.
- **A release runbook** is added: bump + `cargo publish` `libqhyccd-sys` first,
  then `qhyccd-rs`. `MODULE.bazel` is unchanged (members auto-discovered); a
  `CARGO_BAZEL_REPIN` is still needed when the vendored crates' *external* deps
  change.
- **`zwo-rs` is the template's next user** — same vendoring, plus a bindgen step.
