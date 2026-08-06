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
| L5 | `as_conversions`, `arithmetic_side_effects`, `indexing_slicing` | In progress | #854 (sign flips), #863 (step params); L5a complete in #862/#864; L5b in #870/#871/#878, SDK frame buffers in #883, QHY index casts in #890; L5c (simulator pixel loops) in this PR |
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

Saturation was the first answer for a length that cannot fail into a `Result`:
`usize::MAX` makes every buffer too small, so the caller's existing
`BufferTooSmall` arm reports it and no arm has to be invented. Review caught
that this is only half true, and the half it gets wrong is the dangerous one.

> **Saturate what you compare, fail what you allocate.**

A saturated length is exact for a caller comparing a buffer against it and
catastrophic for one that *allocates* it — `vec![0u8; usize::MAX]` aborts the
process instead of reporting anything. The same slice that added the saturating
`buffer_len` also added the first callers that allocate from it, in three
places. Both vendor crates' length functions are therefore fallible
(`RoiFormat::buffer_len -> Option<usize>`,
`Camera::frame_buffer_len -> Result<usize>`), and a second saturating entry
point was deliberately *not* kept beside them: leaving one available leaves the
trap set for the next allocating caller. Saturation survives only where the
result is compared and never allocated — `to_image_array`, where a saturated
`needed` lands in the "buffer too small for frame" answer the function already
returns.

Unlike the `NAXIS` arm above, these arms are reachable and tested: `RoiFormat`
and `CaptureRequest` are plain structs a test builds with `u32::MAX`.

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
`set_if_available(TransferBit, 16.0)` and `GetQHYCCDMemLength`. #881 was then
closed by #887, which negotiates `SupportedVideoFormat` and carries the choice
in `CaptureRequest::image_type` — the shape #884 established for svbony — so no
driver assumes its download format any more. #887 consumed the fallible
`buffer_len` unchanged, which is the check that the rule above survives contact
with a caller that did not write it.

### L5b — QHY index casts

14 production sites across `qhyccd-rs` and `qhy-camera`, and unlike the frame
buffers none of them is a length. Three shapes, each with one answer:

- **An SDK `u32` indexing a `Vec`** (readout-mode tables, 5 sites). Every one
  already had an out-of-range answer — `ok_or(QHYError::Sdk)`, or a fall back to
  the full chip — so the conversion folds into it:
  `usize::try_from(i).ok().and_then(|i| modes.get(i))`. No arm is invented and
  none becomes unreachable, because a failed conversion and an out-of-range
  index are the same event.
- **`Vec::with_capacity` from a device-reported count** (3 sites). Capacity is a
  hint, so `unwrap_or(0)` is total and honest: a count too large to address just
  means no preallocation, and each loop is bounded by that same count. This is
  *not* the allocating case the rule above covers — nothing is sized by it.
- **ASCOM's `usize` surface** (6 sites). `FilterWheel` is `usize` throughout
  (`Position`, `set_position`, and the `Names` / `FocusOffsets` lengths that must
  match it), so the wheel's cached slot state was retyped to `usize` and the
  conversion moved to the SDK seam it actually belongs at.

That retype was worth more than the lint. `set_position` had been narrowing the
client's slot to the SDK's `u32` *before* range-checking it, so every value past
2^32 wrapped onto a real slot — an Alpaca client sending `Position=4294967299`
moved the wheel to slot 3 and got no error. Checking in the type ASCOM sends
makes the wrap impossible rather than merely detected, and the arm is reachable,
so it has a test. `GetQHYCCDMemLength` is the one genuinely allocating site here
and takes the fallible form per the rule above.

### L5c — the simulator's pixel loops

Re-measuring after the QHY index casts put one file at the top of all three
lints at once: `crates/qhyccd-rs/src/simulation.rs` held 218 of the 1,120
production sites left — 136 `as_conversions`, 65 `arithmetic_side_effects`, 17
`indexing_slicing`, where the next-biggest `as_conversions` file has 26. It is
also the only file where all three land on the *same lines*, which is the tell
for what they were pointing at: ten generator functions each walking a frame by
hand.

```rust
let idx = ((y * width + x) * channels) as usize;
for c in 0..channels as usize {
    data[idx + c] = value;
}
```

The cast, the index arithmetic and the indexing are one decision — to compute an
offset — and none of the three lints can be answered on its own. Iterating the
buffer instead deletes all three, because there is no offset to compute:

```rust
for row in data.chunks_exact_mut(frame.row_bytes) {
    for (x, pixel) in (0u32..).zip(row.chunks_exact_mut(frame.pixel_bytes)) {
        pixel.fill(value);
    }
}
```

Three things this settles that a per-site conversion would not have:

- **Coordinates stay `u32`, lengths become `usize`.** The same width is a
  coordinate the gradient ramp divides by and a stride that walks the buffer, so
  `Frame` names both and converts once. Zipping `(0u32..)` onto the chunk
  iterator is what keeps the coordinate in the type that *has* `f64::From` —
  `enumerate()` would have handed back a `usize` and relocated the cast rather
  than removing it, the trap the FITS writer hit above.
- **The bounds check moves into the conversion.** `Frame::pixel_mut` returns
  `None` for a pixel outside the frame, which replaced a four-way `x < 0 || x >=
  width || y < 0 || y >= height` guard in both star drawers: a negative
  coordinate is the `u32::try_from` error arm and an overhanging one is the
  `None`. Every arm is reachable and tested; none was invented.
- **The geometry is refused whole, or not at all.** `Frame::new` is the crate's
  one place that multiplies width by height by channels, and it does so with
  `checked_mul` in `usize`, so an unaddressable frame yields `None` instead of a
  wrapped length. `vec![0u8; frame.len]` is an allocating caller, so this is the
  fallible form the rule above requires — and the old `(width * height *
  bytes_per_pixel * channels) as usize` genuinely panicked (`attempt to multiply
  with overflow`) in any debug build rather than merely wrapping. It was not
  reachable in the field, because the only caller — `qhy-camera` — runs
  `check_geometry` against the chip first; `simulation`'s own `set_roi` stores
  whatever it is handed, so the contract was the caller's rather than the
  library's. Now it is the library's.

That last point has a consequence worth stating separately, because it is the
part that is easy to get wrong: **an entry-point guard is only as good as what
it promises downstream.** Two multiplies further in still took the old shape,
and one of them was a live panic — a starfield asks `random_range` for a centre
one pixel inside each edge, which is an empty range on a frame under three
pixels across. Reaching it needs a frame that is *both* narrow and large, since
the star count is 0.1% of the pixel count and rounds to zero below a thousand
pixels; 2 × 600 does it. The first test written for that guard passed without
the guard for exactly that reason, and only checking that it failed without the
fix caught it.

218 sites → 135, and `indexing_slicing` leaves the file entirely. What remains
is bucket F's value math (`(base as i16 + noise).clamp(0, 255) as u8`) and the
star geometry's signed detour, both of which want the rounding policy rather
than this shape.

### L5c — the simulator's value math

The second half of the same file: 51 lines carrying a cast clippy can
classify. Four groups, and only one of them needed a decision.

**Clamping to a type's own range is a saturating conversion, spelled out.**

```rust
let value = (base as i16 + gradient as i16 + noise).clamp(0, 255) as u8;
```

Two widening casts, a clamp whose bounds *are* `u8`'s, and a narrowing cast —
three lints on one line, nine lines like it. Naming what it does collapses all
of them, and the arms are live traffic rather than defensive padding, so this is
not the dead-arm trap `usize::try_from` was:

```rust
fn sample_u8(v: i32) -> u8 {
    u8::try_from(v).unwrap_or(if v < 0 { u8::MIN } else { u8::MAX })
}
```

**Float → int is already total; what was missing was saying so.** `as` truncates
toward zero and saturates at the destination's bounds, with NaN landing on zero
— and no `TryFrom<f64>` exists to spell it another way. So the 16 sites became
one `quantize` module of six one-line functions carrying a single `#[expect]`,
which is the file's only exemption. That is the template for bucket F's 98
sites: **one annotation at a named boundary, not 98 silent casts.** Truncation
was kept rather than switched to rounding, so no pixel value moves.

**Two total spellings worth carrying forward**, both found by asking whether the
conversion was needed at all rather than how to spell it:

- `Duration` has no `u64` accessor for whole microseconds — `as_micros` is
  `u128` — but `as_secs` is `u64` and `subsec_micros` is `u32`, so composing the
  two needs no conversion and saturates instead of wrapping.
- `(self.base_level >> 8) as u8` is the high byte of a `u16`, which
  `let [base, _] = self.base_level.to_be_bytes();` yields with no cast, no index,
  and no arm.

**And one more defect, with a blast radius worth stating precisely.**
`get_remaining_exposure_us` narrowed its `u64` remainder to the SDK's `u32`
microseconds, which run out at ~71 minutes, and nothing clamps the `Exposure`
parameter to its own control range on the way in. A two-hour exposure wrapped
round to 2,905,032,704 µs — about 48 minutes — so the value fell, jumped, and
fell again. Saturating is the honest answer: the SDK's word cannot express more.

What it does **not** do is shorten an exposure through `qhy-camera`, and the
distinction matters because the first write-up of this claimed otherwise.
`wait_for_exposure` times the exposure host-side from an `AtomicU64` of
microseconds and only *then* polls the camera for confirmation, bounded by a 5 s
`EXPOSURE_CONFIRM_TIMEOUT` it falls through regardless. A bogus remainder
therefore costs at most five seconds of extra polling. The real-hardware path is
untouched for a second reason: `GetQHYCCDExposureRemaining` is a `uint32_t` in
the vendor API, so no narrowing of ours sits on it — the ~71-minute ceiling is
the SDK's own shape. The defect is this crate's API contract, not the driver's
behaviour, and the guard belongs here because the next consumer may well poll it
as authority.

135 sites → 60.

#### What chunked iteration costs, and what actually pays for it

Worth reading before applying this shape to the remaining `indexing_slicing`
sites, several of which are in genuinely hot loops (`imaging/analysis/stars.rs`,
`ppba-driver/src/protocol.rs`). Full-frame 3072×2048, mean of 20, old → new:

| pattern | release | debug |
|---|---|---|
| gradient 16-bit *(the only pattern production generates)* | 1.66 → 1.74 ms | 6.2 → 18.4 ms |
| gradient 8-bit | 17.8 → 17.4 ms | 180 → 276 ms |
| starfield 16-bit | 23.6 → **12.1** ms | 181 → 555 ms |
| starfield 8-bit | 23.1 → **11.9** ms | 109 → 110 ms |
| flat 16-bit | 22.5 → **11.0** ms | 181 → 527 ms |
| flat 8-bit | 13.5 → **11.1** ms | 185 → **97** ms |
| test pattern 16-bit | 35.3 → **19.1** ms | 240 → 666 ms |
| flat, 3-channel 16-bit | 40.7 → **25.2** ms | 292 → 932 ms |

Release is uniformly faster or level. Getting there took two corrections, and
both are the transferable part:

- **Chunking by a runtime `1` is not free.** A mono 8-bit frame's pixel *is* its
  sample, and `chunks_exact_mut(1)` cost ~3.4 ns/pixel more than an indexed
  write — enough to make the cheapest loop in the file 2.6× slower than the code
  it replaced. Walking samples directly (`data.iter_mut()`) is both the faster
  and the more honest spelling for that case, so `generate_flat_8bit` branches
  on it. Passing the per-pixel value through a closure instead, to share one
  helper between the mono and colour arms, made it *worse* again (16-bit went
  10.4 → 23.8 ms): the noise state ends up behind a `&mut` capture and stops
  living in a register.
- **The loop shape was not the real cost — a divide was.** `PixelNoise` drew
  each sample with `next_u32() % span`, one hardware division per pixel, and the
  indexed loop happened to schedule around it better than any iterator spelling
  did (12.8 ms against ~25 ms, and every iterator form measured the same ~25 —
  `iter_mut`, `for_each`, `fill_with`, nested rows). Replacing the modulo with a
  high-word multiply by the span — same range, same negligible bias, one
  multiply — dropped the *iterator* form to 11.1 ms, below the indexed original.
  The indexed form gained nothing from it, which is the tell: it was never
  paying full price for the divide.

So the shape is worth having, but "remove the indexing" and "keep it fast" are
two separate pieces of work, and a cheap-looking loop body can be the thing that
decides. Debug is the other half of the trade: nothing is inlined there, so the
chunked 16-bit paths run ~3× slower and `#[inline(always)]` on the crate's own
helpers does not recover it — the cost is std's. That is accepted here because
production only ever generates the gradient and the debug cost lands on at most
14 BDD captures (~0.2 s against a 2.8 s suite), but CI builds fastbuild, so a
hot loop converted this way pays it on every run. Measure the site first.

### L5c — the star geometry's signed detour

The file's last structural shape, and 50 of the 60 sites it had left. Every one
of them dropped into `i32` to subtract two coordinates, and then threw the sign
away:

```rust
let x = cx as i32 + dx as i32 - size as i32;
let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) else { continue };
let dist = (((dx as i32 - size as i32).pow(2) + (dy as i32 - size as i32).pow(2)) as f64).sqrt();
```

`dx - size` is computed twice — once to place a pixel, once to measure it — and
what each use wants is not a signed number. The distance wants a **magnitude**;
the coordinate wants a **direction**. `abs_diff` yields the first without
leaving `u32`, and a comparison picks the arm for the second:

```rust
let (ox, oy) = (dx.abs_diff(radius), dy.abs_diff(radius));
let (Some(x), Some(y)) = (
    if dx >= radius { cx.checked_add(ox) } else { cx.checked_sub(ox) },
    if dy >= radius { cy.checked_add(oy) } else { cy.checked_sub(oy) },
) else { continue };
let (fx, fy) = (f64::from(ox), f64::from(oy));
let dist = (fx * fx + fy * fy).sqrt();
```

Three things worth carrying to the other `as_conversions` clusters:

- **Clippy exempts float arithmetic**, because a float operation cannot panic or
  wrap. So moving a computation *into* `f64` through a total magnitude removes
  the whole cluster at once — no `checked_*`, no `#[expect]`, nothing to justify
  per site. It only works when the result was going to be a float anyway, which
  a distance is.
- **`u8` is a bound the compiler can see.** `draw_star_*` took `size: u32` and
  wrote `0..=size * 2`, which lints. Retyping the parameter to `u8` makes the
  doubling provably total — 2 × 255 cannot leave `u32` — with no invented cap
  and no arm that never fires. The callers pass `random_range(1..=3)`, so the
  type merely records what was already true.
- **A guard clippy cannot see still needs the saturating spelling.**
  `random_range(1..frame.width - 1)` sits under an early `frame.width < 3`
  return; `saturating_sub(1)` says so without adding a second guard.

**The old form was wrong at the seam, and the rewrite fixes it silently.** An
exhaustive sweep of both forms over 168,611 (centre, size, offset) combinations
— every centre from 0 to 40, plus 1000, 3071 and the `i32`/`u32` boundaries, at
every size the generators draw — agrees **bit-for-bit on coordinate and
distance for every centre below `i32::MAX`**, and diverges only above it, where
`cx as i32` goes negative and the old code dropped a pixel that was inside the
frame. With debug assertions the old form additionally *panicked* on 10,970 of
those cases. Neither is reachable through `qhy-camera`, but both are gone.

60 sites → 10, and the file's only remaining shapes are the four in the tail
(a filter-wheel decrement, a `BayerPattern` discriminant, a row seed, and
`PixelNoise::next_signed`).

#### What the total spelling cost, measured properly

The cross-binary comparison used for the pixel loops could not resolve this
change: over seven passes, `generate_gradient_8bit` moved −6.5% and the colour
flat path +8.6%, and **neither function was touched**. At that noise floor a
real ±3% is invisible. Building both forms into one binary and interleaving them
— same process, same allocator, same cache state — resolves it to ±0.2%:

| ring-distance form | full 3072×2048 frame |
|---|---|
| old, `i32` square then one convert | 12.3 ms |
| **new, magnitude then float square** | **12.7 ms (+3%)** |
| rejected: `f64::from(ox.saturating_mul(ox))` | 18.2 ms (+50%) |

The integer variant looks closest to the original and is by far the worst: the
`saturating_mul` cmov defeats vectorization of the inner loop. The float form's
3% buys ten sites and the seam fix, on the one pattern production never
generates — `ImagePattern::TestPattern` is a test and BDD fixture. Two lessons
generalize: **a same-process A/B is the only comparison that survives layout
noise**, and **a saturating spelling in a vectorizable loop can cost far more
than the widening it avoids.**

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
