# Align `qhyccd-rs` to the `zwo-rs` / `svbony-rs` conventions

**Status:** Draft — not started. Analysis complete 2026-07-23; awaiting
prioritisation/scheduling.
**Author:** drafted 2026-07-23 on `docs/qhyccd-convention-alignment`.
**Depends on:** the vendoring of `qhyccd-rs` + `libqhyccd-sys` into the workspace
([vendor-qhyccd-rs.md](vendor-qhyccd-rs.md), Phases 1 & 2 DONE) — this plan is
only tractable now that both crates are first-party and editable in-tree.
**Related:** [vendor-zwo-rs.md](vendor-zwo-rs.md), ADR-009
(`docs/decisions/009-vendor-qhyccd-rs.md`).

## Motivation

The workspace vendors three camera FFI wrapper crates. `zwo-rs` established the
convention; `svbony-rs` was deliberately modelled on it, so where svbony differs
from zwo it is almost always because the **SVBony SDK forces it**. `qhyccd-rs`
was **transcribed from an earlier hand-written crate**, not modelled on zwo — and
it diverges from the sibling conventions in many places that the **QHYCCD SDK
does not force**. That divergence is the cost: three crates the services consume
side-by-side, one of which reads and behaves differently for historical rather
than technical reasons, which raises the friction of every cross-camera change
and hides at least one real correctness gap (see Phase 1).

This plan separates the **genuinely SDK-forced** differences (keep them — fighting
the C ABI is not the goal) from the **incidental historical** ones (unify them
toward the zwo/svbony conventions), and sequences the unification by
value-to-risk, correctness first.

The goal is **not** a shared types crate. The dual-homed, independently-published
FFI crates (ADR-009/010) cannot depend on an internal workspace crate, and the
genuinely-forced differences (below) cannot be abstracted away regardless. The
goal is to make `qhyccd-rs` *read and behave* like its siblings so that the
residual cross-vendor surface (a service-side camera trait + neutral value enums,
mapped per-vendor in the service — the existing `PierSide → SideOfPier`
precedent) becomes small and regular. That shared-crate/trait question is
explicitly **follow-on work, out of scope here.**

## What must stay per-crate — genuinely SDK-forced (do NOT unify)

These are real; a shared abstraction cannot absorb them and this plan does not
try to. Verified against the `libqhyccd-sys` bindings.

1. **Value/return convention.** Most QHY calls return a bare `u32`
   (`0` == success, `u32::MAX` == error, **no discriminating error codes**), and
   the getter `GetQHYCCDParam(handle, controlId) -> f64` returns the value
   *directly* with `u32::MAX as f64` as its error sentinel and no out-param
   (`libqhyccd-sys/lib.rs:56-57`). zwo/svbony return a small signed error-code
   enum and pass results through out-pointers. This one ABI fact legitimately
   forces the `Option<u32>`-returning `is_control_available`, the epsilon-compare
   error detection in `get_parameter`, and the **impossibility of a `from_code`
   error map**.
2. **Opaque-pointer handle.** `QhyccdHandle = *const c_void`
   (`libqhyccd-sys/lib.rs:16`), vs zwo/svbony's `int` id/index. The
   `unsafe impl Send/Sync` on the handle wrapper is a forced consequence.
3. **Discriminant values.** `BayerMode` is 1-based `GBRG=1..RGGB=4` and `Control`
   discriminants are the SDK's `CONTROL_ID` constants (incl. the 1024+ block and
   the gap at 38); `StreamMode` is consumed as `u8`. These *values* match the
   SDK's own numbering, exactly as zwo's 0-based `Rg..Gb` match ASI's.
4. **Two capture paths.** `ExpQHYCCDSingleFrame`/`GetQHYCCDSingleFrame` *and*
   `BeginQHYCCDLive`/`GetQHYCCDLiveFrame` are four separate C entry points
   (`libqhyccd-sys/lib.rs:58-93`) — the dual single-frame/live API is real, not a
   wrapper invention. (zwo is snap-only; svbony is video-only — all three differ,
   all three forced.)
5. **Separate bin / resolution calls.** `SetQHYCCDBinMode(h, wbin, hbin)` and
   `SetQHYCCDResolution(h, x, y, w, h)` are two distinct entry points
   (`libqhyccd-sys/lib.rs:53-54`), unlike ASI's single bundled `ASISetROIFormat`.
6. **Filter wheel is driven through the camera handle** — see Phase 1; this is
   the finding that *revises* the handle-model verdict rather than a pure
   "incidental" cleanup.

## What to unify — incidental divergence (toward the zwo/svbony conventions)

All of the below is inside `qhyccd-rs` only — internal refactoring, **no
publishing/dependency friction**. Ordered by the phases that follow.

| Divergence | qhy today | zwo/svbony convention (target) |
|---|---|---|
| Handle model | `Arc<RwLock<Option<handle>>>`, clonable alias, **no `Camera: Drop`** | single owner, RAII `Drop`-closes — but see Phase 1 caveat |
| Control representation | exhaustive ~90-variant `Control` + `control as u32` + raw `get_parameter() -> f64` | small semantic subset + `Other(i32)` + explicit `to_raw` + typed `gain()`/`exposure_us()` accessors |
| Error shape | ~45 per-call-site `QHYError` variants | flat enum carrying an op label |
| Simulation | `simulation/` subtree + `mocks.rs` `#[automock]` FFI-mock layer | inline `SimState` + `#[cfg]` fork, no mock layer |
| Module layout | 6-file `camera/` + `backend.rs` + `control.rs` + `mocks.rs` | one file per device with `impl` blocks |
| Frame buffer | fresh `Vec` per frame | caller-owned `&mut [u8]` |
| Public surface | hides `sys`, no `check` helper | re-exports `sys` + a `check` helper |

Worth **propagating the other direction:** qhy's `QHYCCD_SKIP_NATIVE_LINK`
no-link build capability is genuinely useful and zwo/svbony lack the full
equivalent — keep it, and consider lifting it into the siblings later (not this
plan).

## Phases

Each phase is independently shippable and revertible, and follows the standard
[development-workflow.md](../skills/development-workflow.md) design→test→code
order (the crate's own unit/sim suite is the test surface here). Land them in
order; the correctness fix goes first.

### Phase 1 — Handle model + filter-wheel handle sharing (correctness first)

This is the highest-value phase because it is the one that is **both a
convention divergence and a latent correctness bug**, and because the
filter-wheel coupling constrains how it may be fixed.

**The filter-wheel constraint (why the naive "make it single-owner like zwo"
fix is wrong).** The QHY filter wheel is not a separate device — it is driven
*through the camera handle*:
- The CFW C functions all take the camera handle:
  `IsQHYCCDCFWPlugged(handle)`, `GetQHYCCDCFWStatus(handle, ...)`,
  `SendOrder2QHYCCDCFW(handle, ...)` (`libqhyccd-sys/lib.rs:108-117`).
- `FilterWheel { camera: Camera }` (`src/filter_wheel.rs`) delegates
  `open`/`close`/`is_open` straight to the wrapped `Camera`, and drives position
  through the **generic control path** — `Control::CfwPort` (with an ASCII ±48
  offset) and `Control::CfwSlotsNum` via `get_parameter`/`set_parameter`.

Contrast zwo, whose EFW is a **separate SDK and device id** (`EFWOpen(int ID)`,
`EFWSetPosition(int ID, int)`, `EFW_filter.h:113,172`) with its own handle and
its own `Drop`-close — so zwo's trivially-single-owner `Camera` works *because
the wheel is a different device*. That escape hatch does not exist for QHY: the
wheel **is** the camera. So a shareable / interior-mutable handle is
**SDK-justified here**, and Phase 1 must preserve it rather than force zwo's
single-owner shape.

**What is still wrong (independent of the coupling):**
- **No `Camera: Drop`.** Only `Sdk` has a `Drop` (→ `ReleaseQHYCCDResource`,
  `src/sdk.rs`). A dropped `Camera` never calls `CloseQHYCCD` — the device is
  closed manually or leaks. zwo/svbony close on `Drop` (`zwo camera.rs:802`).
- **The `Arc<RwLock<Option<handle>>>` is not actually exploited for the coupling
  it appears designed for.** In `src/sdk.rs` the camera in `cameras()` and the
  camera inside the `FilterWheel` are each built with a **fresh** `Camera::new(id)`
  (`sdk.rs:94,122`) — two independent `Arc`s keyed only by the same id **string**,
  not one shared handle cell. So today the camera↔wheel "sharing" is done by
  **re-opening the same device id twice**, and the `Arc<RwLock>` only provides
  interior mutability + `Clone`, not the sharing it looks built for.

**Target design:**
- Give the **shared handle cell** a `Drop` that calls `CloseQHYCCD` when the last
  strong reference is released (RAII cleanup that survives the shareable model —
  we do *not* have to choose between shareability and `Drop`-close).
- Make `FilterWheel::new` take a **clone of the camera's `Arc` handle cell**
  instead of minting a second `Camera::new(id)`, so the wheel and camera share
  **one** open handle. This both removes the double-open path and makes the
  shared-handle abstraction earn its keep.

**Go/no-go gate — RESOLVED 2026-07-23 from the SDK manual: share one handle
(the fix is required, not optional).** The QHYCCD SDK's documented model is
**one `OpenQHYCCD(id)` per physical camera, with the filter wheel driven through
that same single handle.** The manual's own canonical CFW workflow, Example 5
(`docs/references/qhyccd-sdk-manual.md:6760`), opens the camera exactly once
(`camhandle = OpenQHYCCD(id)`) and then drives the wheel on that handle
(`IsQHYCCDCFWPlugged(camhandle)`, `SetQHYCCDParam(camhandle, CONTROL_CFWPORT, …)`);
§45 Filter Wheel Control (`:4569`) confirms the CFW is not an openable device but
a control (`CONTROL_CFWPORT`) *"available after the camera is initialized with
`InitQHYCCD`."* A **second `OpenQHYCCD` on an already-open id is nowhere
documented** — every workflow, imaging and CFW alike, runs through one handle.
The manual doesn't promise a double open fails, but its silence is exactly why
the design must not *depend* on it (cf. the sibling `InitQHYCCDResource`,
explicitly *"do not call multiple times… may cause the program to crash"*,
`:205`). Therefore the current re-open-by-id path (a second `OpenQHYCCD` on the
same id) is unsound-by-design, and Phase 1 **must** move to one shared handle.
(Incidental confirmations from the same read: the crate's CFW ASCII ±48 offset is
correct — `CONTROL_CFWPORT` uses ASCII 48/49/50 for slots 0/1/2, `:4575`; and the
crate's use of `GetQHYCCDParam` over `GetQHYCCDCFWStatus` matches the manual's own
recommendation.)

**Exit:** `Camera`/shared-handle closes on last-drop; filter wheel shares the
camera's handle (or the double-open is proven safe and explicitly documented);
crate sim + unit suites green; `docs/services/qhy-camera.md` /
`docs/services/qhy-focuser.md` updated if the observable lifecycle changes.

### Phase 2 — Control representation

- Replace the exhaustive `Control` enum + `control as u32` with the zwo/svbony
  shape: a small semantic `ControlType` subset with an `Other(i32)` escape and an
  explicit `to_raw`. The numeric `controlId` addressing stays (SDK-forced); only
  the *representation* changes.
- Add typed accessors (`gain()`/`set_gain()`, `exposure_us()`,
  `target_temperature_celsius()`, …) over the raw `get_parameter`/`set_parameter`,
  mirroring `svbony camera.rs` — including the CFW controls (`CfwPort`,
  `CfwSlotsNum`) that Phase 1's wheel needs, so the wheel keeps working through
  typed accessors rather than raw `Control` variants.

**Exit:** services/tests consume typed accessors; raw `get_parameter(Control, )`
no longer part of the routine surface; suites green.

### Phase 3 — Error shape

- Collapse the ~45 per-call-site `QHYError` variants into a flat enum that
  carries an operation label plus the genuinely-distinct cases
  (`CameraNotOpen`, `Utf8`, control-name context, …). The SDK exposes **no error
  codes** to preserve (Phase-0 forced fact #1), so no information is lost.
- Add a `check`-style helper and re-export the raw `sys` crate at the crate root,
  matching zwo's `pub use libzwo_sys as sys` + `*_check` (folds Phase 6's
  public-surface item forward since it is cheap and touches the same files).

**Exit:** flatter `QHYError`; `sys` + `check` re-exported; suites green.

### Phase 4 — Simulation + mock-layer consolidation (largest structural change)

- Fold the `simulation/` subtree into an inline `SimState` + `#[cfg(feature =
  "simulation")]` per-method fork, matching `zwo camera.rs` / `svbony camera.rs`.
- Remove the `mocks.rs` `#[automock]` FFI-mock surface. zwo/svbony do **not**
  unit-test through a mocked FFI; confirm the qhy unit tests that currently rely
  on `#[automock]` can be re-expressed against the inline sim backend (this is the
  main risk of the phase — audit test coverage before deleting the mock layer).
- **Keep** `QHYCCD_SKIP_NATIVE_LINK` and its `unimplemented!()` stubs — that
  capability is orthogonal and worth retaining.

**Exit:** no `simulation/` subtree, no `mocks.rs`; unit + BDD + ConformU suites
green with no coverage regression.

### Phase 5 — Module consolidation + frame buffer

- Collapse the 6-file `camera/` split + `backend.rs` + `control.rs` into a
  single `camera.rs` with `impl` blocks (device-file-major, as zwo/svbony).
  Mechanical; do it last so it rebases cleanly over the semantic phases.
- Switch the single-frame/live download from a fresh-`Vec`-per-frame to a
  caller-owned `&mut [u8]` with a pre-call bounds check
  (`zwo camera.rs:754` pattern). The two capture *paths* stay (SDK-forced); only
  the buffer ownership changes.

**Exit:** one `camera.rs`; caller-owned frame buffer; suites green.

## Non-goals (explicitly out of scope)

- **A shared types/`common` crate or a service-side camera trait.** That is the
  natural follow-on once qhy is aligned and the residual surface is small — but it
  is a separate decision (and faces the dual-home publishing constraint), tracked
  elsewhere.
- **Any change to the SDK-forced items** in "What must stay per-crate."
- **Vendoring/publishing changes** — [vendor-qhyccd-rs.md](vendor-qhyccd-rs.md)
  owns those; this plan assumes the vendored, first-party state.
- **Touching `zwo-rs`/`svbony-rs`** beyond noting the `SKIP_NATIVE_LINK`
  back-port idea.

## Risks & mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| Handle-`Drop` + filter-wheel sharing rework closes a handle still in use by the other object | Medium | `OpenQHYCCD`-twice semantics settled (single-handle model — see Phase 1 gate); move to one shared `Arc` so there is a single close point; cover with a sim test that opens camera+wheel and drops in both orders |
| Removing `mocks.rs` drops unit-test coverage that only the FFI mock reached | Medium | Phase 4: audit which tests use `#[automock]` and re-express them against the inline sim backend **before** deleting; watch the coverage job |
| `Control` subset omits a variant a service actually uses (incl. CFW controls) | Low | Grep all `Control::` uses across services before subsetting; keep `Other(i32)` as the escape hatch |
| ConformU regression from lifecycle/accessor changes | Low | Run `--config=conformu` at each phase exit (qhy-camera's conformu suite is the acceptance gate) |
| Large rebase churn from module consolidation | Low | Phase 5 (mechanical) runs last, after the semantic phases have settled |

## Rollback

Each phase is an independent, revertible change. No phase touches production
release packaging or the SDK provisioning. Phase 1 is the only one with runtime
lifecycle implications; its rollback is restoring the manual-close / no-`Drop`
behaviour.

## Open questions

1. **`OpenQHYCCD(id)` twice — same handle, second handle, or failure?**
   **RESOLVED 2026-07-23 (SDK manual):** undocumented/unsupported — the SDK's
   model is one handle per camera with the CFW driven through it (Example 5,
   §45). Phase 1 must share one handle rather than re-open by id. See the Phase 1
   go/no-go gate for the full citation.
2. **Does any service depend on `Camera: Clone` / equality-by-id semantics** that
   the Phase 1 handle rework would change? Audit `services/qhy-camera` +
   `services/qhy-focuser` usage first.
3. **Phase ordering vs. an in-flight qhy-camera change** — if a service-level qhy
   change is active, land Phase 1 behind it to avoid a lifecycle-semantics clash.
