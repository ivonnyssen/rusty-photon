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
| L2 | Mechanical `cargo clippy --fix` sweep | Complete | #846, #850 |
| L5 | `as_conversions`, `arithmetic_side_effects`, `indexing_slicing` | In progress | #854 (sign flips), #863 (step params); L5a complete in #862/#864; L5b in #870/#871/#878, SDK frame buffers in this PR |
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

`cargo clippy --fix` per crate, one PR per crate so review stays tractable.
This phase flips no lint to `deny`; it only removes debt so later phases are
smaller. Re-measured before running: **3,649 machine-applicable sites across
41 crates**, of 10,589 total. The sweep cleared 3,644 of them and took the
workspace from 10,589 sites to 6,381.

The six dual-homed FFI crates are out of scope — they carry no
`[lints] workspace = true` and belong to L7.

### `suboptimal_flops` is excluded

Its 87 fixes fold expressions into `mul_add`, which changes the result in the
last ulp and, in the image-analysis code, hides the shape of the maths:
`(1.0 - (smin/smax).powi(2)).sqrt()` becomes
`(smin/smax).mul_add(-(smin/smax), 1.0).sqrt()`, and the Gaussian model in
`fwhm.rs` goes the same way. That is per-site judgment in the code that feeds
autofocus, so it is deferred to L6b — recorded as a decision, like `exit` in L4.

`imprecise_flops` stays in: it yields `E.powf(x)` → `x.exp()` and
`(dx*dx + dy*dy).sqrt()` → `dx.hypot(dy)`, both strict improvements.

### Three things `cargo fix` does that a sweep has to plan for

1. **A single non-compiling suggestion reverts the whole crate, silently.**
   `cargo fix` applies, re-checks, and rolls back everything on error, exiting
   0. `pa-falcon-rotator` lost all 84 fixes because `missing_const_for_fn`
   made `mock::bit` `const` while `cast_lossless` rewrote its body to
   `u8::from(b)` — `From` is not const-stable, so the pair does not compile.
   `rp-catalog` lost all 21 because `string_lit_as_bytes` yields
   `&[u8; 8290]`, which does not implement `Read`. **Re-measure after every
   sweep**; a residual count is the only signal that this happened.
2. **A fix can create a fresh on-by-default warning.** `single_match_else`
   rewrote two wait-then-force `match`es into `if let Ok(_) = .. {} else`, an
   empty then-branch that `redundant_pattern_matching` rejects and refuses to
   auto-fix (it changes drop order). `-D warnings` then fails the tree.
   `.is_err()` is the fix, by hand.
3. **One pass is not enough.** Some suggestions only appear once an earlier
   one lands. Two passes with the target set, then one with the default set to
   absorb (2), reached a fixed point everywhere.

The sweep runs on Linux only, so `#[cfg(windows)]` blocks keep their debt.

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

Re-measured after L2, over the 41 crates that inherit the workspace lints:

| Lint | Prod | Total | Where it concentrates |
|---|---:|---:|---|
| `as_conversions` | 508 | 604 | camera FFI boundaries (`qhy`/`svbony`/`zwo`/`sky-survey` `camera.rs`), `rp/src/mcp/internals.rs` |
| `arithmetic_side_effects` | 472 | 546 | `rp-catalog`, `rp/src/imaging/analysis/stars.rs`, `rp-fits/src/writer.rs`, `rp-ephemeris` |
| `indexing_slicing` | 250 | 525 | `ppba-driver/src/protocol.rs`, `skywatcher-motor-protocol`, `bdd-infra/src/rp_harness/config.rs` |

**L2 did not shrink these three, and `as_conversions` grew.** That is expected,
not a regression: `cast_lossless` converts exactly the casts that *are*
lossless, so what it leaves behind is the genuinely lossy set — and every
`f64::from(x)` it wrote in place of `x as f64` removes a site that was never
L5's problem. What remains is the work L5 was always going to be.

Crate by crate, `rp` last — it carries the largest share on its own.

### `as_conversions` is not one problem

Join each `as_conversions` span with whichever of clippy's five diagnostic cast
lints fired at the same span. That is the compiler's own verdict on what the
cast can lose, and it classifies far better than reading source text. Over the
485 sites left after #854:

| n | what also fired | what fixing it needs |
|---:|---|---|
| 162 | nothing | total by type — only a spelling to pick |
| 98 | truncation, float source | a rounding / clamp policy |
| 77 | truncation | genuine `try_from` candidates |
| 67 | sign loss / possible wrap | same-width sign flip |
| 66 | precision loss | int → float; no `From` impl exists |
| 8 | truncation, bounded on the same line | `#[expect]` with a reason |
| 7 | — | FFI / opaque platform types |

The 162 + 67 that clippy proves total then split by *shape*, and a shape has one
answer rather than 229:

| n | shape | answer |
|---:|---|---|
| 101 | `x as usize` | no `From<u32> for usize` — a boundary question, L5b below |
| 62 | `i32 as usize` from a cucumber `{int}` parameter | change the step signature; `{int}` parses via `FromStr`, so `usize` works and 37 steps already do it |
| 32 | trait-object coercion | not a value conversion — L5a below |
| 12 | `x as u64` | as with `usize` |
| 16 | masked / const narrowing, `char` ↔ int, byte-string | per-site |

Two shapes look mechanical and are not. `hfr.rs`'s `r as usize` sits in a loop
whose body needs signed arithmetic (`(r - cx) * (r - cx)`) and whose bounds feed
`f64::from` — retyping the loop breaks both. And a `const` cannot use `From` at
all (`u32::from` is not const-stable), so `const RAW16_MAX_ADU: u32 = u16::MAX
as u32` has no `From` spelling available.

### L5a — trait-object coercions

32 sites cast to a trait object. These are unsizing coercions, not value
conversions: nothing can be lost, and the fix is to give the compiler a
coercion site instead of an `as`. Three shapes, and only one is subtle.

**`Arc::clone(&x) as Arc<dyn T>` cannot simply lose its cast.** `Arc::clone`
takes its type parameter from the *expected* type, so an `Arc<dyn T>`
expectation makes it demand `&Arc<dyn T>` and the unsizing never gets a chance:

```
808 |     Arc::clone(&manager),
    |     ---------- ^^^^^^^^ expected `&Arc<dyn ServiceManager>`, found `&Arc<RecordingManager>`
```

Two spellings work. Where the concrete `Arc` is not needed afterwards, coerce
the binding once and every later `Arc::clone` reads normally:

```rust
fn spawn(manager: Arc<ScriptedDiscovery>) -> Self {
    let manager: Arc<dyn ServiceManager> = manager;
```

Where it *is* needed — which is 17 of these 19 sites, all test fixtures that
hand the trait object to the code under test and keep the mock for assertions —
pin the type parameter with a turbofish so the coercion lands on the result:

```rust
FalconManager::new(Arc::<MockFalconTransportFactory>::clone(&factory))
```

`x.clone()` also compiles (method resolution takes the type from the receiver),
but it drops the explicit `Arc::clone` spelling the workspace uses to keep
refcount bumps visible.

`Arc::new(Concrete) as Arc<dyn T>` just loses its cast — the argument alone
fixes the type parameter, so the coercion applies to the result.

The last 7 of the 32 are `s as &dyn ProgressEmitter` in `rp`'s MCP tools, all
one shape: an `Option<ProgressSink>` becoming the `Option<&dyn ProgressEmitter>`
the helpers in `internals` take. Two things block the obvious fix at once —
unsizing does not reach inside `Option`, and a closure with an inferred return
type is not a coercion site — so neither the argument's type nor the two
adapters' declared `-> Option<&dyn ProgressEmitter>` can drive the coercion.
Collapsing the trait object is not an alternative either: `ProgressEmitter` has
a second impl that the unit tests count progress notifications through.

An inherent method on `ProgressSink` gives the coercion one named home and
leaves every call site shorter than the cast did:

```rust
impl ProgressSink {
    pub(crate) fn as_emitter(&self) -> &dyn ProgressEmitter {
        self
    }
}

let emitter = sink.as_ref().map(ProgressSink::as_emitter);
```

An explicit closure return type (`|s| -> &dyn ProgressEmitter { s }`) and a
turbofish on `map` both compile too, but each repeats the trait object at all
seven sites.

### L5b — `x as usize` is a boundary question

`usize` has no `From<u32>` (it may be 16 bits), so these have no total named
spelling and the lint cannot be satisfied by picking a better one. Two answers
were measured and rejected before the third:

- **`usize::try_from` per site.** `usize` is at least 32 bits on every target
  the workspace builds for, so the error arm cannot fire on anything we ship —
  76 unreachable arms, uncoverable by construction, against a repo that does
  not allow production coverage exclusions. Red `codecov/patch` by design.
- **`#[expect]` per site.** Honest and cheap, but it annotates the confusion
  instead of removing it, permanently.

What the sites actually say is that a value is being used as a length while
typed as something else. So the rule is about *where* the conversion belongs,
not how to spell it:

> **`usize` must never appear in a serialized format**, because it is
> platform-dependent. Anything bound for disk or a wire carries a fixed width.
> Anything that indexes a buffer is a `usize`. Convert once, where those meet.

That resolves each site without judgment. `rp-fits`' reader hands back a buffer
and its shape, so it yields `usize` — which also deletes a step, since it had
been parsing `NAXIS` from `i64` into a `u32` that every caller immediately
widened again. Alpaca `NumX`/`StartX`, the `ImageBytes` header, the sidecar
JSON's `width`, and a PNG's dimensions are all fixed-width for the same reason;
`crop_subframe`, `Array2::from_shape_vec`, and a preview's subsampling are all
`usize`.

Boundary conversions that survive get folded into an error the function already
returns — an ASCOM subframe too large for a `usize` cannot fit the source
buffer either, so it lands in the existing bounds check rather than earning a
variant of its own.

The writer looked like the exception, because its dimensions are *both* a
`NAXIS` header card and the length of the buffer being validated against them.
Taking them as `u32` there was the first answer; measuring it changed the
verdict. `u32` parameters left the capture path narrowing `image_array.dim()`
from `usize` to `u32` only to widen it straight back for the `Array2` shape,
and left `FitsError::DimensionMismatch` carrying `got: usize` and
`expected: usize` beside `width: u32`. Taking `usize` collapses both, and the
`i64::try_from` it adds on the `NAXIS` side is offset by the `checked_mul`
overflow arm becoming *reachable* — with `u32` parameters that arm is dead code
on every 64-bit target, and with `usize` it is a two-line test. The narrowing
that remains sits at the JSON sidecar, which is genuinely a serialized field.

So the boundary belongs at the last consumer that needs a fixed width, not at
the first function that touches one. Two consequences worth carrying forward:

- **A conversion is not free wherever it lands.** Pushed into
  `gen_autofocus_fixtures`, one landed on `-D clippy::expect_used` and only
  resolved because `main` returns `Box<dyn Error>`, making `?` available.
- **Retyping a boundary can relocate rather than remove it.** The
  `sky-survey-camera` BDD harness holds dimensions that are simultaneously a
  `vec![0u16; n]` length and `f64` WCS header cards, and `f64::from(usize)`
  does not exist. It converts explicitly instead.

### L5b — SDK frame buffers

The camera backends size a download buffer from an ROI, which is the same
boundary in a different costume: the ROI is device state and arrives
fixed-width, the buffer length is a `usize`. Two things made this slice
cheaper than the FITS one.

Each vendor crate already had a function computing that length —
`zwo_rs::RoiFormat::buffer_len`, `svbony_rs::Camera::frame_buffer_len` — and
each vendor crate's download call already compares the caller's buffer against
it before handing the pointer to the SDK. So the conversion has exactly one
home per crate, and the drivers stopped recomputing the length from the ASCOM
request. `zwo-camera` had been restating the bytes-per-pixel as a literal `2`
while `zwo_rs` carried a real `bytes_per_pixel()` covering 1, 2 and 3.

Where the length cannot fail into a `Result` — `buffer_len` returns a plain
`usize` — saturation is the exact answer rather than a fallback: `usize::MAX`
makes every buffer too small, so the caller's existing `BufferTooSmall` arm
reports it, and no arm has to be invented. Same for `to_image_array`, where
a saturated `needed` lands in the "buffer too small for frame" answer the
function already returns.

Reading the drivers this closely surfaced two defects that have nothing to do
with the lint, both filed rather than fixed here: #881 (`zwo-camera` sets
`Raw16` unconditionally and never reads the SDK's `SupportedVideoFormat`, so
an ASI120/ASI130-class camera cannot expose at all) and #882
(`svbony-camera` read the format list into `CameraProperty` and then ignored
it). #882 was closed by #884 while this slice was in review, which changed the
answer here: the negotiated format now rides in `CaptureRequest::image_type`,
so the buffer length follows the format actually selected instead of a
restated constant. That is a better shape than reading it back from the SDK,
and this slice adopted it.

`qhy-camera` was already the one getting this right, via
`set_if_available(TransferBit, 16.0)` and `GetQHYCCDMemLength`; #881 remains
open, so `zwo-camera` is now the only driver that assumes its download format.

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

**4,257 sites after L2**, wholly untouched by the `clippy.toml` knobs. L6a
removes the reason the earlier recommendation was `pedantic = "warn"` with
`nursery` off: both groups gain lints on the beta channel, and under the old
single-job setup that meant a recurrently red nightly. Beta no longer fails on
lints, so `deny` on stable is viable for both.

The shape of what is left:

| Lint | Sites | Prod | Note |
|---|---:|---:|---|
| `needless_pass_by_ref_mut` | 1,190 | 1 | nursery; effectively all cucumber step fns |
| `missing_errors_doc` | 488 | 488 | pedantic; a docs project, not a code one |
| `needless_pass_by_value` | 399 | 73 | |
| `unused_async` | 266 | 2 | |
| `too_long_first_doc_paragraph` | 264 | 264 | pedantic; only 6 auto-fixable |
| `significant_drop_tightening` | 215 | 191 | nursery; lock-scope changes, needs care |
| `cast_possible_truncation` / `_sign_loss` / `_wrap` | 442 | 349 | overlaps L5's `as_conversions` |
| `suboptimal_flops` | 87 | 87 | deferred here by L2 — decide it explicitly |

`nursery` still wants its own look before flipping — it is explicitly unstable,
and its two biggest entries are a test-shaped false positive
(`needless_pass_by_ref_mut`) and a lint that rewrites lock scopes
(`significant_drop_tightening`). Run L5 first: its three lints overlap the 442
`cast_*` sites, so the two rungs are cheaper together than apart.

## L7 — dual-homed FFI crates

Adding `[lints] workspace = true` to `qhyccd-rs`, `zwo-rs`, `svbony-rs` and
their `-sys` shims affects what is published to crates.io, not just this repo.
Decide separately; 1,038 sites with the knobs applied.
