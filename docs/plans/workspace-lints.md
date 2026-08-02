# Workspace Lints Plan — deny the panic classes, on a measured ladder

## Goal

The workspace denies nine ways to panic as of L4 — `unwrap_used`,
`expect_used`, `unreachable`, `panic`, `todo`, `unimplemented`,
`panic_in_result_fn`, `unchecked_time_subtraction`, `string_slice`. What is
left of the target set is `indexing_slicing`, `arithmetic_side_effects`,
`as_conversions` and the `pedantic` / `nursery` groups (`exit` was measured
and deliberately dropped from the target — see L4):

```toml
[lints.clippy]
pedantic = { level = "deny", priority = -1 }
nursery  = { level = "deny", priority = -1 }
unwrap_used = "deny"
expect_used = "deny"
indexing_slicing = "deny"
arithmetic_side_effects = "deny"
unreachable = "deny"
unimplemented = "deny"
unchecked_time_subtraction = "deny"
todo = "deny"
string_slice = "deny"
panic_in_result_fn = "deny"
panic = "deny"
exit = "deny"                # measured, then deliberately dropped — see L4
as_conversions = "deny"
```

This matters for [tenet 2 (robustness)](../workspace.md#project-tenets): a panic
in a driver at 2am ends the night's imaging. The lints that close panic routes
are the point; the `pedantic` / `nursery` groups are a separate, much larger
style question that this plan deliberately sequences last.

## Measured baseline

Numbers below are the **pre-L1** census; each phase re-measures before it runs,
because the earlier estimate is reliably wrong once the knobs and the previous
phase's fixes are in (L3 in particular came in far cheaper than sized here).

Census taken with clippy 0.1.96 on `--workspace --all-targets --all-features`,
driving the full proposed set as `-W` flags so every crate still completes its
check pass. **The tree is warning-clean today (0 diagnostics)**, so every number
below is new debt. Nothing fails to *build* — at `deny` these become errors, but
each is a lint, not a compile failure.

For the 42 crates that inherit `[workspace.lints]`:

| Bucket | Sites | `--fix` can do | Hand-fix |
|---|---:|---:|---:|
| Production (lib/bin) | 4,853 | 2,234 | 2,619 |
| Test-side | 6,703 | 1,287 | 5,416 |
| **Total** | **11,556** | **3,521** | **8,035** |

**The named lints are the cheap part.** Only 1,054 of the 4,853 production
sites come from the thirteen named lints; the other 3,799 are `pedantic` /
`nursery` fallout.

| Named lint | Prod | Named lint | Prod |
|---|---:|---|---:|
| `arithmetic_side_effects` | 387 | `string_slice` | 25 |
| `as_conversions` | 368 | `panic` | 19 |
| `indexing_slicing` | 214 | `unchecked_time_subtraction` | 2 |
| `exit` | 39 | `unwrap`/`expect`/`unreachable`/`todo` | **0** |

### `clippy.toml` changes the test-side picture

Clippy lints test code by default — 3,891 of the test-side sites are in
`tests/` directories and 2,812 are in `#[cfg(test)] mod` blocks inside `src/`.
A repo-root `clippy.toml` suppresses four of them at source, in test scope
only, leaving production untouched:

```toml
allow-unwrap-in-tests = true
allow-expect-in-tests = true
allow-panic-in-tests = true
allow-indexing-slicing-in-tests = true
```

| | Without | With knobs |
|---|---:|---:|
| Production | 4,853 | 4,853 |
| Test-side | 6,703 | **5,020** |
| **Total** | **11,556** | **9,873** |

Three measured limits:

1. **Only eight knobs exist** — `dbg`, `expect`, `indexing-slicing`,
   `large-stack-frames`, `panic`, `print`, `unwrap`, `useless-vec`. There is
   none for `as_conversions`, `arithmetic_side_effects`, `string_slice`,
   `unreachable`, `todo`, `unimplemented`, or `exit`.
2. **Nothing in `pedantic` / `nursery` is covered.** All 3,844 test-side group
   sites survive, including `needless_pass_by_ref_mut`'s 1,171.
3. **The knobs only recognise `#[cfg(test)]` mods and `#[test]` fns.** The 682
   surviving `panic` / `indexing_slicing` test sites are dominated by
   `tests/bdd/steps/*.rs` and `tests/bdd/world.rs` — cucumber's
   `#[given]`/`#[when]`/`#[then]` are not `#[test]` functions, so clippy does
   not classify them as test code. L3 found the tail is broader than that:
   also plain `tests/*.rs` targets other than `bdd.rs`, and panics inside
   closures and `tests/common/` helpers that a `#[test]` fn merely calls.

### The knobs make most existing `#[allow]`s dead

Measured with `--force-warn` (which overrides `#[allow]` but *cannot*
resurrect a lint the knob suppressed at source, so it isolates exactly the
load-bearing attributes). Of 461 clippy allow attributes, 408 touch the trio:

| Lint | Files with allow | Still fire | **Dead** |
|---|---:|---:|---:|
| `unwrap_used` | 365 | 18 | **347** |
| `expect_used` | 363 | 25 | **338** |
| `unreachable` | 348 | 11 | **337** |
| `indexing_slicing` | 1 | 0 | **1** |

Applied in L1: **329 attributes deleted outright, 67 trimmed** to only the lint
that still fires — 470 clippy allow attributes down to 144.
Separately, 335 of the 348 files declaring `allow(clippy::unreachable)` contain
no `unreachable!()` at all: the lint was carried along by copy-paste from the
canonical snippet.

**Scope, not file, decides whether an allow is dead.** A per-file model is
wrong and fails loudly — `crates/bdd-infra/src/lib.rs` carries a crate-root
`#![allow(...)]` that covers every module in the package, and `bdd-infra` is
ordinary lib code rather than `#[cfg(test)]`, so the knobs never applied to it.
Three scopes have to be resolved before a removal is safe:

| Attribute | Scope |
|---|---|
| inner `#![allow]` in a file's header region | the whole package |
| outer `#[allow]` on a `mod name;` declaration | that module's file subtree, honouring `#[path = "..."]` |
| anything else | the file it sits in |

### Blast radius

Cargo only. Bazel never runs clippy (`.bazelrc` mentions it once, in a
comment), and `[lints.clippy]` is a Cargo feature that rules_rust does not
read. Affected: the pre-commit hook, the required `stable / clippy` PR gate,
and the nightly `beta / clippy` early-warning job — the last of which reports
rather than gates, so widening the deny set cannot make it red (L6a).

### Crates this does not reach

`qhyccd-rs`, `zwo-rs`, `svbony-rs` and their three `-sys` shims have no
`[lints] workspace = true` — they are dual-homed to crates.io per
[ADR-009](../decisions/009-vendor-qhyccd-rs.md) /
[ADR-010](../decisions/010-vendor-zwo-rs.md). They carry 1,038 sites even with
the knobs, including **every** `unwrap_used` (664) and `expect_used` (57) site
in the workspace. Phase 7.

## Implementation Status

| Phase | Description | Status | Branch / PR |
|-------|-------------|--------|-------------|
| L0 | This plan | Complete | #827 |
| L1 | `clippy.toml` + dead-allow sweep + the four free lints | Complete | #827 |
| L3 | Deny `panic` — test-crate-root allows | Complete | #831 |
| L4 | Deny `string_slice`; leave `exit` alone | Complete | #831 |
| L6a | Split the CI channels: beta reports, stable gates | Complete | #839 |
| L2 | Mechanical `cargo clippy --fix` sweep | Not started | |
| L5 | `as_conversions`, `arithmetic_side_effects`, `indexing_slicing` | Not started | |
| L6b | `pedantic` / `nursery` at deny | Not started | |
| L7 | Dual-homed FFI crates | Not started | |

**L6 split in two, and L2 moved back ahead of the policy half.** The original
sequencing note put L2 after L6 because L6's standing recommendation was
`pedantic = "warn"` with `nursery` off — under which L2's ~3,521-site `--fix`
sweep would have paid for fixes to lints the workspace never gates on.

That recommendation existed for one reason: both groups gain lints on the beta
channel, so denying them made the nightly `beta / clippy` job recurrently red.
That is a CI-policy problem, not a lint-policy one, and L6a fixes it directly —
beta now reports instead of failing. With the objection removed, `pedantic` and
`nursery` at `deny` on stable become viable (L6b), which restores the case for
L2: it is work the workspace will actually enforce.

L6a does not shrink the 7,643 sites. It removes the reason not to pay for them.

---

## L1 — `clippy.toml` + dead-allow sweep + the four free lints

One PR, mechanical, no per-site judgment.

- Add the four-key `clippy.toml` at the repo root.
- Delete the 360 dead attributes; trim the other 48 to the lints that still
  fire. **Same commit as the `clippy.toml`** — removing allows first would
  break the existing deny.
- Deny `todo`, `unimplemented`, `panic_in_result_fn`,
  `unchecked_time_subtraction`. Four lints for five fixes:
  - `services/session-runner/src/engine/exec.rs:547,575` — both auto-fixable
  - `services/pa-falcon-rotator/src/rotator_device.rs:355`
  - `services/pa-falcon-rotator/src/switch_device.rs:408`
  - `services/qhy-camera/src/backend.rs:784`
- Rewrite the `[workspace.lints.clippy]` comment block in `Cargo.toml`: it
  documents the per-test-module attribute pattern that this phase largely
  deletes. Cross-check `docs/workspace.md` and `docs/skills/testing.md`
  (rules 2 and 11).

**Verification.** Re-run the `--force-warn` census; the surviving set must
match the 48 kept attributes exactly, with no new diagnostics.

## L2 — mechanical sweep

`cargo clippy --fix` per crate, ~3,521 sites, zero judgment. Validated on four
crates: 141 → 67 warnings with all tests still green. Merge per crate so review
stays tractable. This phase does not flip any lint to `deny`; it only removes
debt so later phases are smaller.

## L3 — deny `panic`

Re-measured after L1, `panic` turned out to be the cheapest rung on the ladder:
442 sites, but only **20 outside `tests/`**, and 19 of those are `bdd-infra` —
test infrastructure shaped as a library, so the knobs never see it. Exactly one
was production.

| Where | Sites | Treatment |
|---|---:|---|
| `tests/` in 24 crates | 422 | `clippy::panic` appended to the crate-root `#![allow(...)]` each already carried |
| `crates/bdd-infra/src/` | 19 | same, on the existing crate-root allow |
| `services/doctor/src/catalog.rs` | 1 | fixed |

Two scope facts drove the mechanical part:

- **Every file directly under `tests/` is its own crate root.** Covering
  `tests/bdd.rs` alone missed `test_lib.rs`, `test_integration.rs`,
  `translations.rs`, `runner_integration.rs`, `supervision_integration.rs` and
  `test_mock_server.rs` — six more targets, each needing its own attribute.
- **The knobs see the `#[test]` fn, not what it calls.** A panic inside a
  closure or a `tests/common/mod.rs` helper still fires, which is why
  `rusty-photon-shared-transport`'s failure-injection helpers needed one.

The production fix: `doctor`'s `CATALOG` parsed each embedded
`pkg/doctor.toml` with `unwrap_or_else(|e| panic!(...))`. It now skips an
unparseable entry, and `test_catalog_covers_every_embedded_service` asserts
the catalog covers every `RAW` entry — so a malformed file fails CI loudly
instead of aborting every doctor run in the field.

## L4 — `string_slice` denied, `exit` deliberately not

**`string_slice` (41 sites, 38 in `src/`)** — done. Mostly `get(..)` in place
of a bare range, with three that read better rewritten outright:
`rp-fits`'s exponent split became `split_once('E')`, session-runner's duration
surface check became `strip_prefix`/`trim_start_matches` chaining with no
slicing or length arithmetic at all, and `dsd-fp2`'s mock command dispatch
became a `strip_prefix` chain instead of `starts_with` guards followed by
`[4..]`. Three test modules keep a scoped `#[allow]` (no knob exists), all
slicing a literal UUID to its 8-char disk key.

**`exit` (40 sites)** — **not** denied, and recorded as a decision rather than
a deferral. Every site is `services/*/src/doctor.rs`, where `pub fn run(...) -> !`
exits on doctor's documented 0/1/2 contract (see [doctor](../services/doctor.md)).
Denying it buys a pile of `#[allow]`s or a refactor of a deliberate signature.

## L5 — the expensive three

Real per-site judgment: `checked_*` / `TryFrom` / `get()`.

| Lint | Prod | Where it concentrates |
|---|---:|---|
| `as_conversions` | 368 | camera FFI boundaries (`qhy`/`svbony`/`zwo`/`sky-survey` `camera.rs`), `rp/src/mcp/internals.rs` |
| `arithmetic_side_effects` | 387 | `rp-catalog`, `rp/src/imaging/analysis/stars.rs`, `rp-fits/src/writer.rs`, `rp-ephemeris` |
| `indexing_slicing` | 214 | `ppba-driver/src/protocol.rs`, `skywatcher-motor-protocol`, `bdd-infra/src/rp_harness/config.rs` |

Crate by crate, `rp` last — it carries 2,077 of the hand-fix residue on its own.

## L6a — split the CI channels

Being strict on stable and getting early warning from beta are two goals, and
running one job for both forced a compromise on the stricter one. `check.yml`
now runs them as separate jobs:

- **`stable / clippy`** — unchanged required gate, `-D warnings`.
- **`beta / clippy`** — report-only, and on the schedule plus
  `workflow_dispatch` alone. Deliberately *not* on push to main: only the
  scheduled run acts on the census, so a per-merge beta build would compute a
  report nobody reads. `--cap-lints warn` on clippy's
  argument line downgrades every lint, *including the ones
  `[workspace.lints.clippy]` denies*, so the job exits 0 on lints and fails
  only on a genuine compile break. The cap rides on the argument line rather
  than `RUSTFLAGS` so it applies to the workspace packages being linted and
  leaves dependency artifacts cached.

`tools/ci/beta_clippy_census.py` aggregates the JSON diagnostics per lint
(deduplicating on file/line/column — `--all-targets` reports each source line
once per target, over-counting by ~40%), and a `github-script` step keeps one
`beta-clippy`-labeled issue per lint: opened on first sighting, body rewritten
each night, closed automatically once the lint stops firing. Above 20 distinct
lints it opens nothing and fails instead — truncating the set would make the
auto-close wrongly retire the lints left out, so a mass rename upstream goes to
a human.

The property that makes this cheap: because `stable / clippy` gates every PR at
`-D warnings`, `main` is silent on stable, so **every** finding beta reports is
new on the beta channel. No stable-vs-beta set differencing is needed.

`notify-clippy-failure` now covers both jobs, and its body says a lint is *not*
the likely cause — lints have their own issues.

## L6b — `pedantic` / `nursery` at deny

Still 7,643 sites, wholly untouched by the knobs. L6a removes the reason the
earlier recommendation was `pedantic = "warn"` with `nursery` off: both groups
gain lints on the beta channel, and under the old single-job setup that meant a
recurrently red nightly. Beta no longer fails on lints, so `deny` on stable is
viable for both.

`nursery` still wants its own look before flipping — it is explicitly unstable,
and `needless_pass_by_ref_mut` alone is 1,172 mostly-test sites. Run L2 and L5
first; they remove most of the residue this rung would otherwise have to
absorb.

## L7 — dual-homed FFI crates

Adding `[lints] workspace = true` to `qhyccd-rs`, `zwo-rs`, `svbony-rs` and
their `-sys` shims affects what is published to crates.io, not just this repo.
Decide separately; 1,038 sites with the knobs applied.
