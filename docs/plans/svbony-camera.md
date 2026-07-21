# SVBony Camera Alpaca Driver (`svbony-camera`) + `svbony-rs` FFI

## Status

**Phase E landed (2026-07-21): full `Camera` implementation.**
`services/svbony-camera`'s `SvbonyCamera` now implements the complete
`ascom_alpaca::api::Camera` surface over an expanded `backend::CameraHandle`
seam, following the design doc's soft-trigger video-capture state machine
(mode-select + video-capture start once at connect; each `StartExposure` =
set `SVB_EXPOSURE` → `SVBSendSoftTrigger` → `SVBGetVideoData` polled to a
`exposure*2+500ms` deadline). All nine BDD feature files are green — 60/60
scenarios, 242/242 steps, including `enumeration_connection`'s C3b
(disconnect cancels an in-flight exposure) — and 65 unit tests cover what
BDD structurally cannot: E9 (mid-exposure SDK failure and an exceeded
`SVBGetVideoData` deadline, as two distinct `MockCameraHandle` injection
points) and the generation-counter abort/disconnect race. `cargo fmt` and
`cargo clippy --all-targets --all-features -- -D warnings` are clean.

Design decisions made in this phase, each recorded in
[`docs/services/svbony-camera.md`](../services/svbony-camera.md) at its
relevant contract rather than only here:

- **`AbortExposure` never touches the SDK.** Unlike `zwo-camera`'s
  `ASIStopExposure`, this driver's abort path only bumps the exposure
  generation counter — it does not call `SVBStopVideoCapture` concurrently
  with an in-flight `SVBGetVideoData` on the same handle, since the two
  SDK entry points running on different threads against one camera handle
  is exactly the undocumented-thread-safety risk this plan already flags
  generally. Left open for Phase G: whether the SDK tolerates that
  concurrent call safely (some vendor video APIs are designed to unblock a
  pending read exactly this way), which would make `AbortExposure`
  responsive mid-exposure instead of only at the next natural
  `SVBGetVideoData` return.
- **`CameraXSize`/`CameraYSize` report the raw sensor extent**, not
  `zwo-camera`'s R4-style "aligned down so every binned full frame is a
  valid ROI" — the design doc left this an open question; Phase E resolved
  it in favor of exact, eyeball-able values (3008×3008 for the simulated
  SV605CC) over ConformU-driven alignment, since ConformU wiring is still
  Phase F. Revisit if Phase F's binned-full-frame coverage proves this too
  strict.
- **`CCDTemperature` shares `CanSetCCDTemperature`'s gate** (both keyed on
  `SVB_CAMERA_PROPERTY_EX.bSupportControlTemp`) rather than `zwo-camera`'s
  separately-cached temperature-availability flag — `SVB_CAMERA_PROPERTY_EX`
  has only the one capability flag, unlike `zwo-rs`'s `CameraInfo`, so
  there is no second flag to decouple from.
- **`PulseGuide` stays a literal blocking `SVBPulseGuide` call**, not
  `zwo-camera`'s asynchronous fire-and-forget-with-deadline wrapper — no
  ST4-capable SVBony model exists yet to validate a pulse-duration profile
  against (the SV605CC has no ST4 port; this whole path is mock-backend
  unit-test-only). Revisit if a future ST4-capable model's pulses risk
  ConformU's ~1s response budget.
- **A concurrency bug was found and fixed mid-phase**: `backend.rs::capture`
  originally held the SDK mutex across the (simulation-only) artificial
  exposure-duration wait added so BDD's in-flight scenarios are observable
  (`svbony-rs`'s simulated `get_video_data` never literally blocks). That
  starved every other SDK-backed read — including `is_open`, which
  `ensure_connected` calls at the top of every `Camera` method — for the
  whole exposure, and surfaced as a genuine BDD failure in the "second
  exposure while one is in flight is rejected" scenario (the second
  `StartExposure` request blocked for the full 30s exposure instead of
  being rejected instantly). Fixed by (1) backing `is_open` with its own
  atomic, independent of the mutex `capture` holds, and (2) releasing that
  mutex between `capture`'s ROI/control setup and its trigger +
  `SVBGetVideoData` call — mirroring `zwo-camera`'s own release-during-
  integration pattern, which exists for exactly this reason.

Bad-pixel correction (`SVB_BAD_PIXEL_CORRECTION_ENABLE`) remains
unimplemented — it was not part of this phase's six-area implementation
order and is not exercised by any BDD scenario or the `Camera` surface;
still future work, not a Phase E gap. The design doc's other pre-existing
"to be confirmed against real hardware" caveats (the `SVB_EXPOSURE`
microseconds unit assumption, the stale-frame-flush question, whether
`ElectronsPerADU` has a non-obvious SDK path) were deliberately left as
documented caveats rather than resolved, per this phase's own instructions.

**Phase C/D landed (2026-07-21): bare `services/svbony-camera` skeleton +
design doc + ADR-018 + `@wip` BDD scaffolding.** The service builds, binds
the Alpaca listener on port **11125**, and serves `/management/*` correctly
with zero (production) or one (simulation) registered device;
`--config`/`--port`/`--log-level` and `doctor` all work.
`SvbonyCamera` implements `ascom_alpaca::api::Device` for real
(name/description/driver info/version/connected/UniqueID, config actions)
and every `ascom_alpaca::api::Camera` method as an honest `NOT_IMPLEMENTED`
stub, pending Phase E. `svbony-rs`'s identity-minting win over ZWO (the
serial arrives pre-open at enumeration) is wired directly — no
open-then-close dance. Per Rule 10, `svbony-rs` is a **direct path
dependency** in `services/svbony-camera/Cargo.toml`, not promoted to the
root `[workspace.dependencies]` (still only one consumer). Packaging stubs
landed (`pkg/90-rusty-photon-svbony.rules` — group-scoped, vendor ID
`f266`; `pkg/doctor.toml`; `pkg/rusty-photon-svbony-camera.service`); the
SDK-download helper itself (`rusty-photon-svbony-sdk-install`) is explicitly
deferred to Phase G, per the new [ADR-018](../decisions/018-svbony-sdk-no-license-payload-policy.md),
which extends ADR-013's two-bucket framework with a third
no-license-at-all bucket. Nine BDD feature files exist
(`tests/features/*.feature`); four are genuinely green today
(`enumeration_connection` minus one `@wip` scenario, `config_actions`,
`auth`, `doctor` — their underlying functionality is real, not stubbed, so
they were left untagged rather than force-tagged `@wip`); five are `@wip`
pending Phase E's soft-trigger video-capture state machine (`exposure`,
`binning_and_roi`, `cooling`, `gain_offset_readout`, `sensor_properties`).
`crates/svbony-rs/libsvbony-sys/BUILD.bazel`'s `SVBONY_SKIP_NATIVE_LINK=1`
Bazel shortcut (Phase A/B) is unchanged — still zero SDK provisioning
required to build `//...` today; revisit at Phase G. Hardware is on order:
an SV605CC (IMX533 OSC, two-stage TEC), current ("B") revision. This plan
is the SVBony analogue of [`zwo-driver.md`](zwo-driver.md) and follows the
same design→BDD→implementation flow
([`development-workflow.md`](../skills/development-workflow.md)); the
service design doc is [`docs/services/svbony-camera.md`](../services/svbony-camera.md).

**Phase A/B landed (2026-07-21): `libsvbony-sys` + `svbony-rs` vendored at
`crates/svbony-rs/`.** Hand-written FFI (no bindgen, no vendored header — see
"Verified SDK ground truth" below) plus the safe wrapper (`Sdk`, `Camera`,
typed error mapping) and a `simulation` feature modelling the soft-trigger
video-capture flow and a poll-based cooling ramp. Hardware is on order: an
SV605CC (IMX533 OSC, two-stage TEC), current ("B") revision.

## Motivation

rusty-photon gains a third camera vendor: an ASCOM Alpaca driver for SVBony
cooled cameras (first target: SV605CC), exposing exposures, ROI/binning,
gain/offset, cooling, and readout on a fixed port so `rp` and any Alpaca
client (NINA, SharpCap) can drive it like the existing `qhy-camera` /
`zwo-camera` services. SVBony ships no Alpaca driver (Windows ASCOM binary
only), and its vendor-driver ecosystem is the weakest of the IMX533-class
vendors — which is exactly the gap this workspace fills by writing its own
driver against the native SDK.

## Headline: how SVBony differs from ZWO and QHY

The two prior camera tracks bracket this one. All load-bearing claims below
were verified 2026-07-21 against `SVBCameraSDK.h` and the packaging in
indi-3rdparty `libsvbony` (SDK **1.13.4**).

| Concern | QHY | ZWO | SVBony (this plan) |
|---|---|---|---|
| **SDK license** | Proprietary, unresolved → download-on-target | MIT → redistribute in-package | **No license text at all** (header, blobs, and INDI packaging carry none) → all rights reserved by default; treat like QHY, see *Packaging* |
| **Rust FFI layer** | Published `qhyccd-rs` existed | None usable → we built `zwo-rs` | None usable (see *crate gap*) → we build **`svbony-rs`** + **`libsvbony-sys`**, vendored first-party from day one |
| **Library form** | Static `.a` → baked into our binary | Dynamic `.so`, no SONAME | **Dynamic `.so` only** (INDI installs no `.a`) → link + runtime delivery both matter |
| **Exposure model** | Snap API | Snap API (`ASIStartExposure`) | **Video-only**: no snap API exists; exposures ride `SVBStartVideoCapture` + soft trigger + `SVBGetVideoData` |
| **Serial before open** | Yes | **No** (open required) | **Yes** — `SVB_CAMERA_INFO.CameraSN[32]` comes back from enumeration |

Net: legally SVBony is QHY-shaped (no redistribution grant), mechanically it
is ZWO-shaped (we own the FFI crate; API is closely modeled on ZWO's ASI SDK,
so `zwo-rs` ports with mostly renames) — **except for the exposure path**,
which is this plan's one genuinely new design problem.

## Verified SDK facts

**Packaging & license**
- Single library `libSVBCameraSDK`, SDK version **1.13.4** as carried by
  indi-3rdparty `libsvbony`. Blobs for `amd64`, `x86`, `armv6`, `armv8`
  (Linux aarch64 / Pi 5), `mac64` (macOS x86_64). **No `mac_arm64` in the
  INDI packaging** — macOS Apple-Silicon support must be confirmed from
  SVBony's own SDK download before macOS CI is promised (the third-party
  `ssmichael1/svbony` wrapper claims aarch64 macOS libs exist in the official
  1.13.4 zip; verify directly).
- **No license text anywhere**: the header carries no copyright notice, the
  INDI directory ships no LICENSE/COPYING, and openastroproject's GPLv3 covers
  only their packaging scripts. Redistribution by INDI is vendor-tolerated
  (SVBony supplies SDK updates and has a rep filing indi-3rdparty issues) but
  there is **no written grant**.
- System deps: libusb-1.0; udev rules ship as `90-svbonyusb.rules`.

**API surface (mirrors ASI closely)**
- Enumeration: `SVBGetNumOfConnectedCameras` → `SVBGetCameraInfo(index)`
  (yields `FriendlyName[32]`, `CameraSN[32]`, `CameraID`) → `SVBOpenCamera` →
  `SVBGetCameraProperty` (`MaxWidth/MaxHeight`, `IsColorCam`, `BayerPattern`,
  `SupportedBins[16]`, `SupportedVideoFormat[8]`, `MaxBitDepth`,
  `IsTriggerCam`) and `SVBGetCameraPropertyEx` (`bSupportPulseGuide`,
  `bSupportControlTemp`). `SVBGetSerialNumber` also exists post-open.
- Controls (`SVBGetControlCaps` / `SVBSet/GetControlValue`): `SVB_GAIN`,
  `SVB_EXPOSURE`, `SVB_BLACK_LEVEL` (the ASCOM *offset* analogue),
  white-balance, flip, frame-speed, plus cooling: `SVB_COOLER_ENABLE`,
  `SVB_TARGET_TEMPERATURE` (units of 0.1 °C), `SVB_CURRENT_TEMPERATURE`
  (read), `SVB_COOLER_POWER` (read, 0–100 %), and
  `SVB_BAD_PIXEL_CORRECTION_ENABLE`/`_THRESHOLD`.
- **Exposure = video capture.** There is no `StartExposure` analogue. Camera
  modes: `SVB_MODE_NORMAL` plus trigger modes incl. `SVB_MODE_TRIG_SOFT`
  (`SVBGetCameraSupportMode`/`SVBSetCameraMode`, `SVBSendSoftTrigger`);
  frames are fetched with `SVBGetVideoData(timeout)`.
- ST4: `SVBCanPulseGuide` / `SVBPulseGuide(direction, duration)`, gated by
  `bSupportPulseGuide` at runtime (the SV605CC has no ST4 port; the code path
  stays capability-driven, not model-driven).
- ROI: `SVBSetROIFormat` with the same constraints as ASI — width % 8 == 0,
  height % 2 == 0. Image types include RAW8…RAW16, Y8–Y16, RGB24.

## The Rust crate gap

Same situation as ZWO: no usable crate. Inventory: **`ssmichael1/svbony`**
(MIT, wraps SDK 1.13.4, sys + safe two-crate layout) is the only Rust code —
7 commits, zero stars, no releases, no cooling/temperature support, no
simulation backend. Useful as a bindgen sanity reference; not a dependency.
⟹ We write **`libsvbony-sys`** (bindgen over `SVBCameraSDK.h`) +
**`svbony-rs`** (safe wrapper + `simulation` feature), **vendored first-party
at `crates/svbony-rs/`** from the start — the zwo track's external-repo →
vendored detour (ADR-010) is a lesson already paid for; skip it. Confirm
crates.io name availability before any publish; publishing is deferred and
optional.

## ASCOM mapping — wins and watch-outs

**Wins:**
- **Identity is pre-open.** `CameraSN` arrives with enumeration — no
  open-to-mint-identity dance like ZWO's. Serial-derived UniqueID, no
  `unique_id` config field (follows `qhy-camera`/`zwo-camera`).
- **Cooling maps 1:1** for the SV605CC: `CoolerOn` ↔ `SVB_COOLER_ENABLE`,
  `SetCCDTemperature` ↔ `SVB_TARGET_TEMPERATURE` (÷10), `CCDTemperature` ↔
  `SVB_CURRENT_TEMPERATURE` (÷10, reported independently of cooling, as
  `zwo-camera` does), `CoolerPower` ↔ `SVB_COOLER_POWER`.
- **Offset is native** (`SVB_BLACK_LEVEL`), and OSC metadata
  (`BayerPattern` → `SensorType`/`BayerOffset`) comes from the property
  struct at runtime — never hardcode the SV605CC's pattern.
- ROI/binning constraints are byte-for-byte the ASI rules `zwo-camera`
  already enforces.

**Watch-outs (the real design work):**
- **Exposure state machine over video mode.** The plan of record: at connect,
  select `SVB_MODE_TRIG_SOFT` when `IsTriggerCam`, start video capture once,
  and run each ASCOM exposure as *set `SVB_EXPOSURE` → soft trigger →
  `SVBGetVideoData` with a deadline of exposure + margin*; non-trigger
  cameras fall back to normal mode with per-exposure capture restart. This is
  the INDI driver's shape — treat `indi_svbony_ccd` as the behavioural
  reference and verify each step against the real camera. Two sub-risks:
  **stale-frame flush** (buffered frames from before a ROI/exposure change
  must be drained or the first frame after a change is wrong) and **long
  exposures** (`SVBGetVideoData` timeout handling; StopExposure/AbortExposure
  map to stopping capture and discarding, since there is no graceful ASI-style
  data-preserving stop — expect `CanStopExposure = false`,
  `CanAbortExposure = true` unless hardware testing shows otherwise).
- **Tenet 3 — no actuation on connect — includes the cooler.** Connect,
  reconnect, and config-apply must not touch `SVB_COOLER_ENABLE` or
  `SVB_TARGET_TEMPERATURE`; the TEC engages only on operator command. Review
  this explicitly at the connect-path PR (workspace tenet list names cooler
  setpoints as actuation).
- **Bad-pixel correction default.** The SDK's on-camera hot-pixel correction
  (`SVB_BAD_PIXEL_CORRECTION_ENABLE`) silently alters raw data; for
  calibrated astrophotography it should default **off** — decide in the
  design doc, set explicitly at connect *(a read-modify-write of an image
  pipeline flag, not actuation)*, and verify the control exists on the 605CC.
- **Gain semantics drifted across vendor driver/SDK versions** (SVBony's own
  ASCOM Q&A documents gain-scale fixes). Pin the SDK version; hardware
  validation includes a gain/offset sweep against advertised e-/ADU curves.
- **SDK quality is the ambient risk.** The complaints in the field (gain
  changes, cooler quirks, early-revision banding) live below any driver we
  write. Budget hardware-validation time à la
  [`zwo-real-hardware-validation.md`](zwo-real-hardware-validation.md),
  including a dark-frame banding check to confirm the delivered unit is the
  fixed revision.
- **Thread safety is undocumented** — assume none: every SDK call funnels
  through `spawn_blocking` with a single logical owner per device, the same
  discipline as `qhy-camera`/`zwo-camera`, including the generation-counter
  guard so an aborted/disconnected exposure can't publish a late frame.

## Behavioural reference & licensing

- **INDI `indi_svbony_ccd`** (indi-3rdparty) is the only maintained open
  driver and encodes years of quirk workarounds (soft-trigger flow, buffer
  flushes, SDK-version guards). indi-3rdparty drivers are GPL/LGPL-family:
  **read for behaviour, clean-room reimplement, never copy code** — the same
  posture `qhy-camera` took toward `qhyccd-alpaca` and `zwo-camera` toward
  `indi-asi`.
- SVBony's closed Windows ASCOM driver is not a reference; their SDK demo
  code (in the SDK zip) documents intended call sequences.

## Decisions (proposed)

| Area | Decision |
|---|---|
| **Service** | `svbony-camera`, Camera only, port **11125** (11111–11124 are allocated). One service per device per ADR-014; SVBony filter wheels/other devices are out of scope and would be separate services on their own SDKs. |
| **FFI crates** | `svbony-rs` (safe + `simulation`) + `libsvbony-sys` (bindgen), **vendored first-party at `crates/svbony-rs/`** (nested, per ADR-010's end state). No external repo phase, no lockstep git pins. |
| **Link gating** | `build.rs` honours `SVBONY_SKIP_NATIVE_LINK=1` (no link directives emitted) so `test.yml`/`safety.yml` and the default Bazel build need zero SDK provisioning; the `simulation` feature removes the camera, not the link — identical semantics to `zwo-rs`. Single SDK library, so no per-device link features needed. |
| **Simulation** | In `svbony-rs`, modelled on `zwo-rs`'s: fabricated frames, cooling ramp, and — new — the **soft-trigger video flow**, so the service's exposure state machine is exercised sim-side exactly as against hardware. |
| **Identity** | UniqueID derived from enumeration-time `CameraSN`; refuse + `warn!` if empty. |
| **Exposure path** | Soft-trigger video mode as described in *Watch-outs*; `CanStopExposure = false`, `CanAbortExposure = true` (revisit after hardware validation). |
| **Bazel** | Copy `crates/zwo-rs`'s first-party two-variant (real/sim) `BUILD.bazel` + `cargo_build_script` pattern. Repin-twice per Rule 10 on the Cargo.toml change. |
| **Packaging** | **ADR-013 gains a third bucket** (new ADR, Phase D): *no-license + dynamic-only* → never redistribute; a `rusty-photon-svbony-sdk-install` root-only helper downloads the **pinned** SDK archive, verifies a **pinned sha256**, and installs `libSVBCameraSDK.so` to `/usr/lib/rusty-photon/` (RUNPATH resolution, ZWO's mechanics with QHY's delivery). Udev rules authored by us (vendor-ID match), group-scoped, per ADR-013 §3. If SVBony grants written redistribution permission (worth an email — they are responsive), collapse to the ZWO bucket with no layout change. |
| **CI provisioning** | New composite action `.github/actions/install-svbony-sdk` fetching the blob from a **pinned indi-3rdparty commit** (public fetch at build time, not redistribution by us — same source `install-zwo-sdk` uses). Real link exercised only in `conformu.yml` + nightly `native.yml`; macOS inclusion contingent on the mac_arm64 verification above. |
| **Branch discipline** | All work on feature branches; never `main`. |

## Delivery phasing

The FFI crate is again the long pole, but cheaper than ZWO's was: `zwo-rs` is
a structural template (API shapes, error mapping, sim backend, build gating,
Bazel files all port). The exposure-model difference concentrates in Phase B
(sim must model soft-trigger) and Phase E (state machine).

- **Phase A — `libsvbony-sys`:** ✅ *landed (2026-07-21).* Hand-written
  `extern "C"` bindings (**not** `bindgen` — corrects this bullet's earlier
  text: the no-license-grant finding above rules out vendoring the header the
  way `bindgen` needs, so `libsvbony-sys` mirrors `libqhyccd-sys`'s
  hand-transcribed-signatures approach instead), `build.rs` dynamic link
  (`SVBCameraSDK` + `usb-1.0`) with `SVBONY_SKIP_NATIVE_LINK` gate, no
  Windows branch (indi-3rdparty declares it unsupported). Byte-verifying the
  official SDK zip (static libs? mac_arm64? Windows import libs?) is
  deferred to Phase G (real hardware/packaging) — not done in this phase.
- **Phase B — `svbony-rs`:** ✅ *landed (2026-07-21).* Safe handles/enums/error
  mapping ported from `zwo-rs`'s shapes; `simulation` backend incl. the
  soft-trigger video-capture flow and a poll-based cooling ramp (mirrors
  `zwo-rs`'s EAF focuser position ramp — advance-on-poll, not wall-clock
  time). 25 unit tests, `cargo`+`bazel` quality gate green. Not yet consumed
  by any service (that's Phase C).
- **Phase C — bare service:** ✅ *landed (2026-07-21).* `svbony-camera`
  serving zero (production) or one (simulation) Camera on `:11125`.
  *Deviations:* (1) no `install-svbony-sdk` action exists yet (unchanged
  from Phase A/B's `SVBONY_SKIP_NATIVE_LINK=1` Bazel shortcut), so the
  **real** (non-`simulation`) `:svbony-camera` Bazel binary cannot link
  (verified: undefined `SVBOpenCamera`/`SVBCloseCamera`/etc. symbols) and is
  tagged `manual` so `bazel build //...`/`bazel test //...` skip it by
  default — only the library (no final link) and the `svbony-rs_sim`-backed
  binary/BDD/unit-test targets are first-class `//...` targets in this
  phase, unlike `zwo-camera`/`qhy-camera` whose real binaries link because
  CI provisions their SDKs. (2) No repin was needed (no new crates.io deps —
  `svbony-rs` is a first-party path dependency).
- **Phase D — design doc + ADR + BDD:** ✅ *landed (2026-07-21).*
  `docs/services/svbony-camera.md` (behavioural contracts incl. the
  exposure state machine and tenet-3 connect-path statement),
  [ADR-018](../decisions/018-svbony-sdk-no-license-payload-policy.md) (the
  ADR-013-extension), `docs/workspace.md` rows, and nine BDD feature files
  mapped from the design doc — mirroring `zwo-camera`'s six camera features
  plus `exposure.feature` for the soft-trigger specifics; four files are
  genuinely green (not `@wip`) since their underlying functionality
  (`Device`, config actions, TLS/auth, doctor) is real as of Phase C.
- **Phase E — full Camera:** ✅ *landed (2026-07-21, see this document's
  Status section for the full detail).* `Device + Camera` over `svbony-rs`
  — exposure state machine, ROI/bin, gain/offset (`SVB_BLACK_LEVEL`),
  cooling, `backend.rs` mock seam, `spawn_blocking` bridge with generation
  counter, config actions, serial identity. 65 unit tests + 60/60 BDD
  scenarios green.
- **Phase F — gates:** ConformU on the sim backend, wired into `conformu.yml`
  (per-service matrix + `install-svbony-sdk`); nightly `native.yml` real-link
  build; full local quality gate.
- **Phase G — packaging + real hardware:** the downloader-helper package per
  the ADR; then SV605CC validation — dark-frame banding check (revision
  confirmation), gain/offset sweep, cooler ramp/overshoot behaviour, long-
  exposure + abort timing, stale-frame flush verification, USB throughput on
  the Pi 5. Findings feed back into the design doc (Rule 2).

## Future work

- **SV605MC / other SVBony cooled cameras** — same driver, capability-driven;
  needs only hardware validation.
- **SVBony filter wheel (SV226)** — separate service on its own SDK if ever
  in scope (ADR-014 shape).
- **`rp` `CameraConfig` consumer** — shared tail item with `zwo-camera`
  Phase G; whichever lands first defines the pattern.
- **Vendor redistribution grant** — an emailed one-liner from SVBony would
  collapse the packaging to the ZWO bucket.

## References

- Template plan: [`zwo-driver.md`](zwo-driver.md); hardware-validation
  template: [`zwo-real-hardware-validation.md`](zwo-real-hardware-validation.md)
- Precedent services: [`zwo-camera.md`](../services/zwo-camera.md) ·
  [`qhy-camera.md`](../services/qhy-camera.md)
- ADRs: [008](../decisions/008-zwo-camera-native-sdk-ffi.md) ·
  [010](../decisions/010-vendor-zwo-rs.md) ·
  [013](../decisions/013-native-sdk-payload-policy.md) ·
  [014](../decisions/014-zwo-per-device-services-and-link-features.md)
- SDK ground truth: indi-3rdparty `libsvbony` (`SVBCameraSDK.h`, per-arch
  blobs, `CMakeLists.txt` — SDK 1.13.4, `.so` only, no license text);
  SVBony official SDK download (to byte-verify in Phase A)
- Behavioural reference: indi-3rdparty `indi-svbony` (GPL-family —
  behaviour only, no code copying)
- Rust prior art (reference only): `github.com/ssmichael1/svbony` (MIT)
