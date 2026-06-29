# Adopt `color-eyre` for top-level error & panic reporting

**Status:** Proposed — not yet implemented. This document is the plan and the
reasoning; it commits no code.
**Author:** drafted 2026-06-27 on `docs/plan-color-eyre-error-reporting`.
**Scope:** the binary boundary only — `main()` return formatting and the process
panic hook. `thiserror` remains the workspace's error-*definition* mechanism
everywhere; this plan does **not** touch the ~46 structured error types, the
`driver_error!` macro, or the ASCOM/HTTP classification path.
**Decision record:** if accepted, land an ADR (`docs/decisions/011-error-reporting-layers.md`)
capturing the layered-error policy below; this plan is the implementation plan, the
ADR is the durable "why".

---

## 1. Motivation

### 1.1 What we have today

Error handling in the workspace is unusually disciplined and splits cleanly into
two jobs:

- **Defining errors** — `thiserror` (`= "2.0.18"`, workspace dep, used by 24
  member crates, ~46 `#[derive(Error)]` types). Every fallible boundary returns a
  structured, matchable enum. There is **zero** ad-hoc string error boxing in
  library code.
- **Classifying errors** — the `driver_error!` macro
  (`crates/rusty-photon-driver/src/macros.rs`) and per-service `From` impls turn
  those enums into `ASCOMError` codes (device layer) or HTTP status codes
  (`services/plate-solver/src/error.rs`, via `IntoResponse`). This is **pattern
  matching over named variants** — it depends on errors staying typed.

The one weak spot is the **top of `main()`**. Of the 15 service binaries
(`services/*/src/main.rs`), **14 return `Result<(), Box<dyn std::error::Error>>`**;
the exception is `plate-solver`, whose `main()` returns `ExitCode` (it owns
process exit codes for the astrometry solver) and so needs separate handling
(§3.1). The inner `ServiceRunner` closure returns the same type alias
(`crates/rusty-photon-service-lifecycle/src/runner.rs`: `type ServiceResult`).
When a service fails to *start* (config parse, port bind, native-SDK init) or
exits with a fatal error, the Rust runtime prints that boxed error via its `Debug`
impl — a **single line, no `source()` chain, no context, no span**. For a
long-running imaging daemon that may fail at 2am unattended, that is the worst
possible moment to have the least diagnostic detail.

### 1.2 Why this is the right layer to improve

These services log routine errors via `tracing` **inside** their run loop; the
`main()` return value is therefore hit mainly on **startup failures and fatal
exits** — exactly the rare, high-stakes path where a readable report pays off.
Improving it is cheap and self-contained, and it aligns with the project tenets
(**#1 night-time use, #2 robustness**).

### 1.3 Why `color-eyre` specifically (not `anyhow`, not a hand-rolled walk)

Three candidates were weighed (full pros/cons in §5):

| Need | hand-rolled `source()` log | `anyhow` | `color-eyre` |
| --- | --- | --- | --- |
| Readable `source()` chain at top level | ✅ | ✅ | ✅ |
| Pretty **panic** reports | ❌ | ❌ | ✅ |
| **Span context** (`tracing` `SpanTrace`) in panics | ❌ | ❌ | ✅ |
| New dependency | none | light | heavier |

The workspace already standardises on `tracing`. `color-eyre`'s differentiator is
the `tracing-error` `SpanTrace` integration: it can capture *which span stack was
active* when a panic (and, with deeper wiring, an error) occurred. That is context
neither `anyhow` nor a `source()`-walk can produce, and it is precisely what an
async multi-service system loses today. If we were not going to use the `tracing`
synergy, `anyhow` would be the more conventional pick — so this plan only makes
sense **because** we wire up the span layer.

---

## 2. Crucial scope boundary — what this does and does NOT buy

This must be stated plainly, because it shapes the whole design and prevents
overselling.

`color-eyre` captures a `SpanTrace`/backtrace **at the moment a `Report` is
constructed**. There are two distinct surfaces:

1. **Panics** — `color_eyre::install()` replaces the panic hook. A panic anywhere
   in any service then prints a formatted report **including the active
   `SpanTrace`**, provided the `tracing` subscriber carries a
   `tracing_error::ErrorLayer`. ✅ **Full value, no discipline cost.**

2. **Errors returned from `main()`** — to get the pretty multi-line chain, the
   error `main()` returns must be an `eyre::Report`. The `source()` chain is
   preserved and printed nicely. **But** the `Report` is constructed at the
   *top* of `main`, long after the failing `async` code returned and its spans
   closed — so the report carries **no useful error-origin `SpanTrace`**. ✅
   Pretty chain, ❌ no span context on the *error* path.

Getting an error-origin `SpanTrace` would require constructing `eyre::Report`s
**deep in the call stack, inside the spans** — i.e. in the very library/device
code that must stay `thiserror`-typed for ASCOM/HTTP classification. **We
explicitly decline to do that.** The discipline-preserving version of this plan
therefore delivers:

- Pretty **panic** reports **with** span context (the headline win), and
- Pretty top-level **error-chain** formatting (a smaller, real win),

and **deliberately forgoes** error-origin span traces. §3.4 makes that boundary
unbypassable by construction.

---

## 3. Design

### 3.1 Single owner: `rusty-photon-service-lifecycle`

Every binary already routes its lifecycle through this one crate (it owns the
tokio runtime, signal handlers, *and* `init_tracing`). That makes it the natural
and only place to wire error reporting. **No service-level error-handling code
changes.** Concretely:

1. **Dependencies (this crate only):** add `color-eyre` and `tracing-error`.
   - Per **CLAUDE.md rule 10**, these are *not* promoted to workspace deps —
     exactly one crate uses them, and keeping them crate-local is itself the
     enforcement mechanism (see §3.4).

2. **`init_tracing` gains an `ErrorLayer`** (`src/logging.rs`). Compose
   `tracing_error::ErrorLayer::default()` into the existing subscriber so
   `SpanTrace::capture()` sees the live span stack. This is the prerequisite for
   span context in panic reports; without it `color-eyre` still works but prints
   "span trace disabled".

3. **`ServiceRunner` installs the hook once** (`src/runner.rs`). Call
   `color_eyre::install()` before building the runtime, guarded by a
   `std::sync::Once` so repeated runner invocations (the crate's own tests call
   `run()` many times per process) never hit `install()`'s "already installed"
   error. Services get pretty panics **for free** just by using the runner — zero
   per-service edits for surface #1.

4. **Top-level error formatting** (surface #2). Change the closure/`main` return
   so the error type `main` yields is an `eyre::Report`. Two mechanisms are
   possible; the spike (§4) picks one:
   - **(a) Re-typed alias (preferred):** redefine the crate's public result type
     as `Result<(), color_eyre::Report>` and **re-export it** as
     `rusty_photon_service_lifecycle::ServiceResult`. Each service changes its
     `fn main() -> Result<(), Box<dyn std::error::Error>>` to
     `fn main() -> rusty_photon_service_lifecycle::ServiceResult` (14 one-line,
     mechanical edits; the closure bodies — which mostly end in `Ok(())` and `?`
     typed errors — are unaffected because `thiserror` errors are
     `Send + Sync + 'static` and convert into `Report` cleanly).
   - **(b) Convert at the boundary:** keep the closure returning a boxed
     `Send + Sync` error and convert to `Report` only at the runner's return.
     Smaller blast radius, but leaks the `Box<dyn Error>` shape into signatures.

   Either way the conversion is contained to this crate + the 14 `main`
   signatures. No `lib.rs`, device, codec, or `rp-*`/`rusty-photon-*` library code
   is touched.

5. **`plate-solver` exception.** Its `main()` returns `ExitCode`, not the
   lifecycle result type, so the §3.1(4) swap does not apply. It still benefits
   from the panic hook (surface #1) for free via the runner. For surface #2,
   where it maps an error to a non-zero `ExitCode`, optionally format that error
   through an `eyre::Report` first (e.g. `eprintln!("{:?}", Report::from(e))`)
   before returning the code — a one-site change, kept out of scope for the MVP if
   it complicates the spike.

### 3.2 What the change touches (full surface)

| File / area | Change | Risk |
| --- | --- | --- |
| `crates/rusty-photon-service-lifecycle/Cargo.toml` | +2 deps (crate-local) | low |
| `…/src/logging.rs` | add `ErrorLayer` to subscriber | low |
| `…/src/runner.rs` | `Once`-guarded `install()`; result-type change | **medium** (return-type bound — see §4) |
| `…/src/lib.rs` | re-export `ServiceResult` / `Report` | low |
| `services/*/src/main.rs` (×14) | one-line return-type swap | low, mechanical |
| `docs/crates/rusty-photon-service-lifecycle.md` | document the new behaviour (CLAUDE.md rule 2) | n/a |
| `docs/decisions/011-error-reporting-layers.md` | new ADR | n/a |

### 3.3 SCM path parity

The Windows SCM dispatch path (`runner.rs` `mod scm`) type-erases the closure
future into `Pin<Box<dyn Future<Output = ServiceResult>>>`. If §3.1(4a) changes
`ServiceResult`, that boxed type changes with it — the SCM arm must compile under
the new alias too. `install()` should also run on the SCM entry path so a service
running under the Windows Service Control Manager gets the same panic reports.
Covered by the spike on a Windows target (or at least `--target` type-check).

### 3.4 Enforcement: the dependency graph **is** the guardrail

The single biggest risk of introducing an `anyhow`/`eyre`-style crate into a
codebase this disciplined is **erosion**: once `eyre!("…")` / `.wrap_err("…")`
are in scope, they become the path of least resistance and contributors (human or
agent) reach for an untyped ad-hoc error inside device/library code — silently
defeating the ASCOM/HTTP classification that depends on typed variants.

We neutralise that **structurally, not by lint**:

- **Only `rusty-photon-service-lifecycle` depends on `color-eyre`/`tracing-error`.**
  No other crate can write `eyre!`/`bail!`/`wrap_err` without **adding the
  dependency to its own `Cargo.toml`** — a visible, reviewable, hard-to-do-by-
  accident act.
- We **re-export the result *type* only** (`ServiceResult` / `Report`), never the
  `eyre!`/`bail!` *macros*. Services can name the return type but cannot conjure
  ad-hoc errors from it.
- The ADR states the policy in one sentence: *`thiserror` defines errors
  everywhere; `eyre` formats them only at the binary boundary, owned solely by the
  lifecycle crate.*
- **Optional belt-and-braces:** a workspace `clippy.toml` `disallowed-types` entry
  for `eyre::Report` outside the lifecycle crate. Listed as optional because the
  dependency-graph constraint already makes the violation require a deliberate
  `Cargo.toml` edit; add the lint only if a reviewer wants a second tripwire.

---

## 4. De-risking spike (do this first)

The one real unknown is the **return-type bound**. `eyre::Report` requires its
inner error be `Send + Sync + 'static`; the current `Box<dyn std::error::Error>`
does not. Some service closure may today return a non-`Send`/`Sync` error that
compiles under the looser bound.

**Spike:** on a throwaway branch, apply only §3.1(4a) (re-type the alias to
`Result<(), color_eyre::Report>`, re-export it, swap the 14 `main` signatures) and
run `bazel build //...`. Outcome decides:

- **Clean build** → adopt mechanism (a).
- **A closure surfaces a non-`Send`/`Sync` error** → either fix that error type
  (usually trivial and desirable) or fall back to mechanism (b) for that path.

This is read-only on `main` and contained; it answers the only question that can
make the plan more invasive than advertised.

---

## 5. Pros & cons (the argument, in full)

### Pros

1. **Far better startup/fatal diagnostics** on the exact path that is worst today
   (single-line `Debug`), at the worst time (unattended night runs). Directly
   serves tenets #1/#2.
2. **Pretty panic reports with span context** — the headline win, free to every
   service via the runner, impossible with `anyhow` or a `source()` walk.
3. **Centralised and tiny** — one crate owns it; services change one line of
   signature each and nothing else.
4. **Removes the `Box<dyn Error>` smell** from `main` in favour of an idiomatic
   report type.
5. **`tracing`-native** — reuses infrastructure the workspace already commits to,
   rather than bolting on an unrelated mechanism.

### Cons / risks (and mitigations)

1. **Discipline erosion** — *mitigated structurally* by the dependency-graph
   guardrail (§3.4); this is the crux and the reason the plan is shaped around a
   single owner.
2. **Error-origin `SpanTrace` is NOT obtained** without eroding the boundary, so
   the error-path win is "pretty chain" only, not "span trace" (§2). Stated up
   front to avoid overselling. The *panic* path does get span context.
3. **Return-type bound change** (`Send + Sync`) — *de-risked by the §4 spike*
   before any broad edit.
4. **New transitive deps** (`backtrace`, `owo-colors`, `tracing-error`, …) in the
   Bazel `crate_universe` graph; requires the
   `CARGO_BAZEL_REPIN=1 bazel mod tidy && bazel mod tidy` refresh (CLAUDE.md rule
   10) and must stay green across the native-SDK and sanitizer matrices. Modest,
   one-time.
5. **`install()` is process-global** — *mitigated* by the `Once` guard so the
   crate's multi-`run()` tests and the BDD harness don't double-install.
6. **Coverage tenet** — the install/format path is production code, so it needs a
   test (no `coverage(off)` on production code). See §6.

### Why not the alternatives

- **Do nothing / hand-rolled `source()`-walk in the runner catch path** — gets
  ~80% of the *error-chain* value with zero deps and zero erosion risk, but gives
  **nothing** for panics and no span context. A legitimate cheaper option; this
  plan is the "spend one guarded dependency for the panic+span win" choice. If the
  team prefers minimalism, fall back to this.
- **`anyhow`** — strictly worse here: same erosion risk, but leaves the `tracing`
  span value (the whole reason to bother) on the table.

---

## 6. Testing & quality gate

- **Unit (lifecycle crate):** assert `init_tracing` builds a subscriber carrying
  `ErrorLayer`; assert the `Once`-guarded install is idempotent across repeated
  `ServiceRunner::run()` calls (extends the existing `runner.rs` tests, which
  already invoke `run()` several times per process).
- **Smoke:** a deliberately-failing closure returns `Err`; assert the process
  prints a multi-line report (chain present) rather than a single `Debug` line.
- **No production coverage exclusions** (project feedback): the install/format
  path is covered by the above, not annotated `coverage(off)`.
- **Gate (CLAUDE.md rule 4):** `bazel build //... && bazel test //...`,
  `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings`. Because
  deps change, run the `CARGO_BAZEL_REPIN=1 bazel mod tidy && bazel mod tidy`
  refresh and commit the updated `MODULE.bazel.lock`.

## 7. Rollback

Fully reversible and low-blast-radius: revert the lifecycle-crate commit and the
14 one-line `main` signatures; remove the two deps and re-run the `bazel mod tidy`
refresh. No persisted state, wire format, or error-classification behaviour is
affected, so nothing downstream depends on the change.

## 8. Phased delivery

- **Phase 0 — Decision:** review this plan; if accepted, write
  `docs/decisions/011-error-reporting-layers.md`.
- **Phase 1 — Spike (§4):** confirm the return-type bound; pick mechanism (a)/(b).
- **Phase 2 — Lifecycle crate:** add deps, `ErrorLayer`, `Once`-guarded
  `install()`, result-type change; unit + smoke tests; update
  `docs/crates/rusty-photon-service-lifecycle.md`.
- **Phase 3 — Services:** swap the 14 `main` signatures; full quality gate +
  `bazel mod tidy` refresh.
- **Phase 4 — Optional:** add the `clippy.toml` `disallowed-types` tripwire if a
  reviewer wants belt-and-braces on top of the dependency-graph guardrail.

---

## References

- `crates/rusty-photon-service-lifecycle/src/{runner,logging}.rs` — the single
  owner; `type ServiceResult`, `init_tracing`, SCM dispatch.
- `docs/skills/service-lifecycle.md` — standard `main.rs` shape and the
  `ServiceRunner` contract this plan extends.
- `crates/rusty-photon-driver/src/macros.rs` — `driver_error!`, the
  classification machinery that mandates errors stay `thiserror`-typed.
- `services/plate-solver/src/error.rs` — the HTTP `IntoResponse` classification,
  same constraint.
- [`color-eyre`](https://github.com/eyre-rs/color-eyre) /
  [`eyre`](https://github.com/eyre-rs/eyre) /
  [`tracing-error`](https://github.com/tokio-rs/tracing/tree/master/tracing-error)
  — upstream crates.
- Project tenets & coverage policy — workspace memory / `docs/decisions/`.
