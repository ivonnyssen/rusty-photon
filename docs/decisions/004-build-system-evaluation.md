# ADR-004: Build System Evaluation — cargo-rail, Bazel, Buck2

## Status

Accepted — optimize cargo-rail; revisit when the UI technology is decided.

## Context

The workspace uses cargo-rail for change detection in CI, reducing the
build matrix from ~75 to ~21 jobs per PR. However, any change to the
workspace `Cargo.toml` triggers a full rebuild across all CI jobs because
cargo-rail's file classifier hardcodes workspace `Cargo.toml` as
infrastructure (`FILE_KIND_TOML_WORKSPACE`), setting `infra=true`
regardless of what section changed. In practice, adding or updating a
workspace dependency — the most common PR type — forces all 315 BDD
scenarios to run across 5 services on 3 OSes, even though BDD tests
account for 69-72% of each job's runtime.

With less than a third of the planned services written and a web UI on
the roadmap (technology undecided), we evaluated Bazel and Buck2 as
alternatives.

## Options Considered

### 1. Optimize cargo-rail with targeted workspace dep resolution (selected)

cargo-rail's `[change-detection.custom]` patterns take priority over the
hardcoded file classifier. Adding `workspace-manifest = ["Cargo.toml"]`
to the custom patterns intercepts the classification, producing a
`custom:workspace-manifest` surface instead of blanket `infra=true`.

A shell script (`scripts/resolve-workspace-deps.sh`) then uses
`cargo metadata` and TOML section comparison to determine which crates
are actually affected:

- If only `[workspace.dependencies]` changed: greps member `Cargo.toml`
  files for workspace references to the changed deps, walks the reverse
  dependency graph, and outputs targeted `-p` flags.
- If `[workspace.package]`, `[workspace.lints]`, `[profile]`, `members`,
  or other sections changed: falls back to `--workspace`.

Cargo's own fingerprinting handles incremental compilation correctly
regardless — the optimization targets test execution (BDD, ConformU),
which is the dominant CI cost.

### 2. Bazel with rules_rust + rules_ts

rules_rust provides mature Rust support. crate_universe reads
`Cargo.toml` for external deps. Bazel's content-addressable action
cache and sandboxed execution provide fine-grained, correct dependency
tracking. rules_ts offers production-ready TypeScript support.

**Not selected because:**
- The rp architecture (Tenet 7: "UI is a client, not a component")
  intentionally decouples the web UI from Rust services via REST +
  WebSocket. The build graphs are architecturally independent —
  Bazel's multi-language advantage does not apply.
- No Bazel integration exists for Leptos SSR + WASM builds.
- Migration effort is disproportionate at 12 crates.
- The immediate problem (unnecessary test execution) is solvable with
  cargo-rail configuration.

Bazel remains the preferred alternative if the workspace grows beyond
~50 crates, if the UI technology decision introduces shared build
artifacts between Rust and TypeScript, or if build hermeticity becomes
critical.

### 3. Buck2

**Disqualified:** No TypeScript support, immature ecosystem outside
Meta, sparse documentation, limited Windows support.

## Decision

Optimize cargo-rail with the custom pattern and workspace dep resolver.

## Consequences

- `.config/rail.toml` gains a `[change-detection.custom]` entry for
  `Cargo.toml`.
- `scripts/resolve-workspace-deps.sh` added for CI workspace dep
  resolution.
- `test.yml` and `conformu.yml` plan jobs updated with a manifest
  resolver step.
- Planned upstream cargo-rail PR to add native workspace Cargo.toml
  semantic analysis, which would eliminate the workaround.
- Revisit this ADR when the UI technology decision is made.
