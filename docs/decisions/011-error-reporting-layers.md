# ADR-011: Layered error reporting — `thiserror` everywhere, `color-eyre` only at the binary boundary

## Status

Accepted (2026-07-01); implemented by
[`docs/plans/error-reporting-color-eyre.md`](../plans/error-reporting-color-eyre.md).

## Context

Error handling in the workspace splits cleanly into two jobs:

- **Defining errors** — `thiserror` (workspace dep, ~46 `#[derive(Error)]`
  types across 24 member crates). Every fallible boundary returns a
  structured, matchable enum; there is zero ad-hoc string error boxing in
  library code.
- **Classifying errors** — the `driver_error!` macro
  (`crates/rusty-photon-driver/src/macros.rs`) and per-service `From` impls
  turn those enums into `ASCOMError` codes (device layer) or HTTP status
  codes (`services/plate-solver/src/error.rs`). This is pattern matching
  over named variants — it depends on errors staying typed.

The weak spot was the top of `main()`: 14 of the 15 service binaries returned
`Result<(), Box<dyn std::error::Error>>`, so a startup failure (config parse,
port bind, native-SDK init) or fatal exit printed a **single-line `Debug`
dump** — no `source()` chain, no context — and a panic printed the default
hookless backtrace. For long-running imaging daemons that fail unattended at
night (tenets #1/#2), that is the worst diagnostic surface at the worst time.

## Decision

**`thiserror` defines errors everywhere; `eyre` formats them only at the
binary boundary, owned solely by `rusty-photon-service-lifecycle`.**

Concretely:

1. `rusty-photon-service-lifecycle` is the **single owner** of `color-eyre`
   and `tracing-error`. The deps are crate-local, deliberately **not**
   workspace deps: no other crate can write `eyre!` / `bail!` / `wrap_err`
   without visibly adding the dependency to its own `Cargo.toml`. The
   dependency graph *is* the guardrail against erosion of the typed-error
   discipline the ASCOM/HTTP classification depends on.
2. `init_tracing` composes a `tracing_error::ErrorLayer` into the shared
   subscriber, so `SpanTrace::capture()` sees the live span stack.
3. `ServiceRunner::run` / `run_with_reload` install the `color-eyre`
   error/panic hooks once per process (`Once`-guarded). Every service gets
   formatted **panic reports with span context** for free — no per-service
   wiring.
4. Run closures return `RunResult = Result<(), Box<dyn Error + Send + Sync>>`
   (mechanism (b) of the plan — see "Mechanism" below); the runner converts
   the error into a `color_eyre::Report` at its own boundary, preserving the
   `source()` chain, and `main` returns
   `ServiceResult = Result<(), Report>` so the chain prints as a readable
   multi-line report.
5. The lifecycle crate re-exports the **types only** (`ServiceResult`,
   `RunError`, `RunResult`, `Report`) plus one chain-preserving conversion
   (`report_from_boxed`, for the rare fallible step that runs before the
   runner). The `eyre!`/`bail!` macros are **never** re-exported: services
   can name the types, and while inherent constructors like `Report::msg`
   remain reachable through the re-export, using them outside the binary
   boundary is against policy — the macros' absence removes the path of
   least resistance, and the crate-local dependency keeps any violation a
   visible, reviewable `Cargo.toml` edit.
6. `plate-solver` keeps its `ExitCode`-returning `main` (it owns process exit
   codes) but formats the runner's `Report` with `{:?}` on stderr, so it
   prints the same multi-line chain.

### Mechanism: why closures return a boxed error, not `Report`

The plan's spike settled this. `eyre::Report`'s only conversion is the
blanket `From<E: Error + Send + Sync + Sized>`; boxed trait objects
(`Box<dyn Error + …>`) do not satisfy it (`Box<dyn Error>` is not itself
`Error`), so a closure returning `Report` cannot `?` the boxed errors the
services' `load_config` / `ServerBuilder::build` helpers return — every such
call site would need a manual conversion. Keeping the closure's error boxed
(now tightened to `+ Send + Sync`, which `Report` needs) leaves all existing
`?` sites untouched; the runner applies one chain-preserving adapter
(`report_from_boxed`) at its boundary.

### Scope boundary — what this does and does NOT buy

- **Panics**: full value — formatted report **with** the active `SpanTrace`
  (via the `ErrorLayer`). This is the headline win and the reason for
  `color-eyre` over `anyhow` or a hand-rolled `source()` walk.
- **Errors returned from `main`**: pretty multi-line `source()` chain, but
  **no error-origin span trace** — the `Report` is constructed at the top of
  the stack, after the failing code's spans closed. Capturing an
  error-origin `SpanTrace` would require constructing `Report`s deep inside
  the `thiserror`-typed device/library code, which this ADR explicitly
  forbids. We deliberately forgo it.

## Consequences

- Startup failures and fatal exits print the full error chain; panics print
  formatted reports with span context — on every service, uniformly.
- Services changed only their `main` signature (and, where helpers return
  boxed errors, `Box<dyn Error>` tightened to `Box<dyn Error + Send + Sync>`
  — a strict improvement pre-1.0).
- New transitive deps (`backtrace`, `owo-colors`, `tracing-error`, …) enter
  the Bazel `crate_universe` graph once, for one crate.
- Anyone adding `eyre`-style errors to library code must first edit that
  crate's `Cargo.toml` — a visible, reviewable act. Optional belt-and-braces
  (a `clippy.toml` `disallowed-types` entry) was considered and left out; the
  dependency-graph constraint suffices.
- Rollback is one commit: revert the lifecycle crate, the `main` signatures,
  and the `bazel mod tidy` refresh. No wire format, persisted state, or
  error-classification behaviour is affected.

## References

- Plan (motivation, alternatives weighed, spike):
  [`docs/plans/error-reporting-color-eyre.md`](../plans/error-reporting-color-eyre.md)
- Owner crate design:
  [`docs/crates/rusty-photon-service-lifecycle.md`](../crates/rusty-photon-service-lifecycle.md)
- Classification machinery that mandates typed errors:
  `crates/rusty-photon-driver/src/macros.rs`, `services/plate-solver/src/error.rs`
- Upstream: [`color-eyre`](https://github.com/eyre-rs/color-eyre),
  [`tracing-error`](https://github.com/tokio-rs/tracing/tree/master/tracing-error)
