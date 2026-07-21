# SVBony Camera Alpaca Driver (`svbony-camera`) + `svbony-rs` FFI

## Status

**Proposed (2026-07-21). Not started.** Hardware is on order: an SV605CC
(IMX533 OSC, two-stage TEC), current ("B") revision. This plan is the SVBony
analogue of [`zwo-driver.md`](zwo-driver.md) and follows the same
design→BDD→implementation flow
([`development-workflow.md`](../skills/development-workflow.md)); the service
design doc (`docs/services/svbony-camera.md`) is Phase D work, not this file.

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

- **Phase A — `libsvbony-sys`:** bindgen over `SVBCameraSDK.h`, `build.rs`
  dynamic link (`SVBCameraSDK` + `usb-1.0`) with `SVBONY_SKIP_NATIVE_LINK`
  gate, green on Linux x86_64 + aarch64; byte-verify the official SDK zip
  (static libs? mac_arm64? Windows import libs?) and record findings here.
- **Phase B — `svbony-rs`:** safe handles/enums/error mapping ported from
  `zwo-rs`; `simulation` backend incl. soft-trigger flow and cooling ramp.
- **Phase C — bare service:** `svbony-camera` serving a sim Camera on
  `:11125`; prove build/link both variants, Bazel two-variant build,
  `install-svbony-sdk` action, repin-twice.
- **Phase D — design doc + ADR + BDD:** `docs/services/svbony-camera.md`
  (behavioural contracts incl. the exposure state machine and tenet-3
  connect-path statement), the ADR-013-extension ADR, workspace.md rows, and
  the BDD feature files (`@wip`) mapped from the design doc — mirroring
  `zwo-camera`'s six camera features plus an exposure-mode feature for the
  soft-trigger specifics.
- **Phase E — full Camera:** `Device + Camera` over `svbony-rs` — exposure
  state machine, ROI/bin, gain/offset (`SVB_BLACK_LEVEL`), cooling,
  `backend.rs` mock seam, `spawn_blocking` bridge with generation counter,
  config actions, serial identity. Unit + BDD green.
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
