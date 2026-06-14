# ZWO ASI Camera + EFW Filter Wheel Alpaca Driver (`zwo-camera`) + `zwo-rs` FFI

## Status

**Design/planning — FFI crates stood up; `zwo-camera` service pre-implementation.**
The `zwo-rs` + `libzwo-sys` FFI crates now exist as a standalone repo
([github.com/ivonnyssen/zwo-rs](https://github.com/ivonnyssen/zwo-rs), MIT/Apache-2.0):
a `bindgen` FFI skeleton that generates and links the ZWO SDK with green CI
(`check` + `test` on Linux x86_64; built and tested locally on aarch64). The
rusty-photon side has **no code yet** — this plan remains the agreed decision
record that will drive `docs/services/zwo-camera.md` (the service design doc), the
BDD scenarios, and the implementation, per the design→BDD→implementation flow in
[`docs/skills/development-workflow.md`](../skills/development-workflow.md). The
workspace now pins the FFI crate at the lockstep rev (see *Monorepo integration
recipe*).

It is the ZWO analogue of the in-design
[`qhy-camera`](../services/qhy-camera.md) service. Where `qhy-camera` consumes the
author-maintained, already-published [`qhyccd-rs`](https://crates.io/crates/qhyccd-rs)
FFI crate, ZWO has **no usable equivalent**, so this plan also covers standing up
two new author-maintained FFI crates: **`zwo-rs`** (safe wrapper) and
**`libzwo-sys`** (raw bindgen), siblings to `qhyccd-rs`/`libqhyccd-sys`.

Scope sequence: **Camera first → EFW filter wheel fast-follow → EAF focuser
later.** Developed **standalone** (the parallel `qhy-camera` work is tracked
separately).

## Motivation

rusty-photon needs a first-class ASCOM Alpaca driver for ZWO ASI cameras (and ZWO
EFW filter wheels), exposing exposures, ROI/binning, gain/offset, cooling,
readout, and filter selection over Alpaca on a fixed port so the `rp`
orchestrator and any Alpaca client (NINA, SharpCap) can drive them like any other
device. This mirrors what `qhy-camera` does for QHYCCD hardware, reusing the same
`ascom-alpaca` server framework and the `sky-survey-camera` (simulator camera) /
`qhy-focuser` (hardware driver) scaffolding.

The behaviour is derived from open ZWO drivers as a **behavioural reference only**
(no code copied — see *Behavioural reference & licensing*), the same posture
`qhy-camera` took toward `ivonnyssen/qhyccd-alpaca`.

## Headline: how ZWO differs from QHY (this drives every decision)

The `qhy-camera` precedent assumed two things that are **both inverted** for ZWO,
plus one that is the same. All facts below are verified against primary sources
(ZWO/INDI SDK headers, crates.io, Debian packaging); each load-bearing claim was
adversarially fact-checked.

| Concern | QHY (the precedent) | ZWO (this plan) |
|---|---|---|
| **SDK license** | Closed/proprietary; redistribution terms *unresolved* → forced onto an authenticated internal cache tier | **MIT** ("Copyright 2015, ZWO Company") → blob may be cached/redistributed; can live on the **public** R2 cache mirror |
| **Rust FFI layer** | Mature published `qhyccd-rs`/`libqhyccd-sys` already exist; the driver just writes the device layer on top | **No usable equivalent** → we also build & maintain `zwo-rs` + `libzwo-sys` |
| **Build/link gating** | Native lib links at compile time on *every* machine | **Same constraint** (the `-sys` `build.rs` links unconditionally; the SDK must be present at link time even for `--features simulation`) |

Net: ZWO is **legally much easier** (MIT, redistributable, all target arches
shipped) but **mechanically more work up front** (we build the FFI that QHY got
for free). The device-trait layer itself is *easier* than QHY — a cleaner C API,
and more ASCOM features map natively (see *ASCOM mapping*).

> **Important nuance (verified):** MIT eases *redistribution/caching* but does
> **not** remove the *build-link* gating. The SDK is still a source-less native
> binary that must be present at link time; Debian ships `libasi` in `non-free`
> precisely because it is binary-only. So `zwo-camera` is still a native-SDK
> exception to the workspace's pure-Rust default link, exactly like `qhy-camera`.

## Verified SDK facts

**Packaging & license**
- Four independently-versioned SDKs (as of June 2026): **ASI Camera
  `libASICamera2` V1.41** (2026-01-12), **EFW `libEFWFilter` V1.8.4**
  (2025-12-01), **EAF `libEAFFocuser` V1.8.1** (2026-03-18), CAA rotator V1.5.9.
  Camera and EFW are **separate libraries with no shared handle** → co-hosting
  both in one service is a free choice, not a constraint.
- **License = MIT** (verbatim `license.txt`, "Copyright (c) 2015, ZWO Company"),
  confirmed via the INDI/Debian redistribution. Caveats: ZWO's own archive ships
  no `LICENSE` file (only a README), so the attribution comes from packagers; the
  notice must travel with any cached blob.
- **Arch matrix covers all targets** for both camera and EFW: `x64` (Linux
  x86_64), `armv8` (Linux aarch64 / **Pi 5**), `mac_arm64` (**Apple Silicon**),
  plus x86/armv6/armv7/mac_x64. The `mac_arm64` binaries are genuine Mach-O arm64
  (byte-verified) and actively maintained into 2026. macOS `.dylib`s need
  `install_name_tool` fixing before linking (INDI automates this).
- System deps: **libusb-1.0** + udev `99-asi.rules` (VID `0x03c3`, `MODE=0666`,
  `usbfs_memory_mb=200` for USB3 throughput). EFW is USB-HID (no kernel driver)
  but the SDK still talks libusb.

**The Rust crate gap (the single biggest finding)**
- **No `qhyccd-rs` analogue exists.** Inventory: `generic-camera-asi` v0.0.11
  (MIT/Apache, bindgen, **synchronous**, **camera-only**, doesn't vendor the SDK,
  upstream repo now 404s, ~16 mo stale); `cameraunit_asi` v4.1.0 (predecessor);
  `smroid/asi_camera2` (GitHub-only, **armv8-only** hardcoded, camera-only);
  `GreatAttractor/libasicamera-sys` (MIT, camera-only, unpublished, dormant);
  **`devDucks/asi-rs`** — the *only* Rust code covering camera + EFW, but
  **GPL-3.0** (copyleft, incompatible with our MIT/Apache), unpublished, sync, and
  MQTT-daemon-shaped (not Alpaca). **No Rust EAF binding exists.**
- ⟹ **We write our own** `libzwo-sys` (bindgen) + `zwo-rs` (safe wrapper), with a
  `simulation` feature — because there is **no SDK simulation backend** (unlike
  `qhyccd-rs`, which ships one).

## ASCOM mapping — wins and watch-outs vs `qhy-camera`

The ASI snap-mode API maps cleanly onto `ICameraV3`. Notable deltas:

**Wins (ZWO supports things QHY deferred):**
- **`StopExposure` works** — `ASIStopExposure` is a single **graceful,
  data-preserving** stop ("image can still be read out"). So ZWO sets **both**
  `CanStopExposure = true` and `CanAbortExposure = true` (back abort with the same
  call, discarding data). QHY ships `CanStopExposure = false`.
- **Native `PulseGuide`** via `ASIPulseGuideOn/Off` (ST4), gated on `ST4Port` →
  cheap `CanPulseGuide = true`. QHY deferred this.
- **`ElectronsPerADU` is native** in `ASI_CAMERA_INFO` → a real value, not the
  `NOT_IMPLEMENTED` placeholder QHY uses.
- Single `PixelSize` ⟹ `PixelSizeX == PixelSizeY` trivially.

**Watch-outs (ZWO-specific friction):**
- **Serial requires an open camera.** `ASIGetSerialNumber` (stable 8-byte → 16-hex
  UniqueID source, only since driver V1.14.0227) needs the camera *opened* first —
  unlike QHY's pre-open read. So enumeration must open each camera to mint its
  identity. Some older cameras report no serial. `ASIGetID` is a writable, USB3-only
  flash id (a weak fallback).
- **No SDK simulation backend** → the `zwo-rs` `simulation` feature must fabricate
  frames (and EFW position/moving) itself, modelled on `qhyccd-rs`'s sim.
- **Stricter ROI:** width % 8 == 0, height % 2 == 0 (ASI120 USB2: w·h % 1024 == 0).
- **Snap vs video are mutually exclusive** modes; snap is the ASCOM path, video is
  a future high-FPS guiding path.
- **EFW `-1`-while-moving is an out-parameter**, not the return value.
  `EFWGetPosition` returns `EFW_SUCCESS` during a move and writes `-1` into
  `*pPosition`; that maps directly onto ASCOM `Position`'s own `-1` moving sentinel
  (INDI uses `#define EFW_IS_MOVING -1`). `EFW_ERROR_MOVING` is a *different* enum
  value, not `-1`.
- **`EFWGetNum` reportedly not thread-safe on macOS** → serialize enumeration.
- EFW exposes **no per-slot names or focus offsets** → `Names`/`FocusOffsets` come
  from config (as for any wheel).

EFW → `IFilterWheelV2`: `EFWGetNum → EFWGetID → EFWOpen → EFWGetProperty
(EFW_INFO{ID, Name[64], slotNum}) → EFWGetPosition/EFWSetPosition → EFWClose`;
`EFWGetSerialNumber` for the UniqueID.

## Behavioural reference & licensing

- **INDI `indi-asi`** is the most complete maintained-through-2026 open driver
  (camera `asi_base`/`asi_ccd`, EFW `asi_wheel`, EAF `asi_focuser`, ST4, hotplug;
  default KStars/Ekos backend). **License is per-file:** camera/focuser are
  LGPL-2.1+, but **`asi_wheel.cpp` (EFW) is GPL-2.0+** → read for behaviour,
  clean-room reimplement, **never copy code** into MIT/Apache rusty-photon (same
  discipline `qhy-camera` used with `qhyccd-alpaca`).
- **`python-zwoasi`** (steve-marple, permissive) is the cleanest reference for the
  raw SDK call sequences. **INDIGO** (`ccd_asi`/`wheel_asi`/`focuser_asi`, C) is a
  co-equal cross-platform second reference.
- ZWO ships **no** open ASCOM/Alpaca driver (closed Windows ASCOM binary; "Air"
  cameras embed closed Alpaca firmware).

## Decisions (agreed)

| Area | Decision |
|---|---|
| **FFI crates** | New external siblings to `qhyccd-rs`: **`zwo-rs`** (safe) + **`libzwo-sys`** (raw bindgen over `ASICamera2.h` + `EFW_filter.h`; EAF later). Both names confirmed available on crates.io. |
| **`simulation` feature** | In `zwo-rs`, covering **camera + EFW** (mirrors `qhyccd-rs`'s sim; fabricates frames + EFW position/moving). |
| **Canonical home** | Each crate is its own repo, published to crates.io. **Not** vendored into the monorepo tree, **not** a git submodule. |
| **SDK delivery / link** | System-installed; `libzwo-sys` `build.rs` links **unconditionally** (`ASICamera2` + `EFWFilter` + `dylib=usb-1.0`), mirroring `libqhyccd-sys`. `simulation` removes the camera, **not** the link. |
| **Cargo wiring** | Declared once in `[workspace.dependencies]`, per-service `{ workspace = true }`. **Lockstep dev:** `zwo-rs = { git = "https://github.com/ivonnyssen/zwo-rs", rev = "1e978c4" }` → swap to `= "=0.1.0"` before the PR merges. Local edit loop: uncommitted `.cargo/config.toml` `paths = ["../zwo-rs"]`. |
| **Bazel wiring** | `@cr` `from_cargo` picks up the git dep automatically (the `ascom-alpaca` git dep already does this). Tag `zwo-camera` `requires-cargo` first → later a `crate.annotation` on `libzwo-sys` for the native link. SDK blob on the **public** R2 cache (MIT permits). Repin-twice (Rule 10) on every rev/version change. |
| **Service shape** | One combined `zwo-camera` (Camera + FilterWheel), enumerate-all, port **`11122`** (11121 is `qhy-camera`). |
| **Device identity** | `ASIGetSerialNumber` (open briefly at enumeration → close) → fall back to `ASIGetID` → else refuse + `warn!`. Serial-derived UniqueID; no `materialize_identity`, no `unique_id` config field (follows `qhy-camera`). |
| **Dev model** | **Lockstep** (driver tracks the crate via git rev; publish `zwo-rs`/`libzwo-sys` 0.1.0 + pin before merge). |
| **Sequencing** | Standalone in this track; **Camera → EFW → EAF**. |
| **Branch discipline** | All work on a feature branch (never `main`). |

## Monorepo integration recipe

`external/` is **not** a Bazel build input (no BUILD files, no references) and
first-party crate BUILDs are `@cr`-coupled (`load("@cr//:defs.bzl", …)`), so an
in-tree submodule member would force rusty-photon-specific BUILD files into the
published crate repos. Hence: **consume as external deps**, not as members.

```toml
# workspace Cargo.toml — [workspace.dependencies], declared once
zwo-rs = { git = "https://github.com/ivonnyssen/zwo-rs", rev = "1e978c461e52cd786b15d33708fceda170b23524" }  # lockstep
# zwo-rs = "=0.1.0"   # ← swap in before the PR merges
```
```toml
# services/zwo-camera/Cargo.toml
ascom-alpaca = { workspace = true, features = ["server", "camera", "filter_wheel", "client"] }
zwo-rs = { workspace = true }
# the service's `simulation`/`mock` feature forwards to `zwo-rs/simulation`
# for BDD + ConformU (the SDK still links — see gating note).
```
```python
# MODULE.bazel — once libzwo-sys's native link is Bazel-ized (Phase 6)
crate.annotation(crate = "libzwo-sys", ...)   # link-search→SDK, static/dylib + usb-1.0
# until then: the zwo-camera target is tagged `requires-cargo`
#             (excluded from `bazel test //...` by .bazelrc).
```

The `libzwo-sys` native link is the one genuinely new Bazel pattern: today's only
native sys crate (`aws-lc-sys`) *compiles vendored C source*; `libzwo-sys` instead
*links a prebuilt proprietary blob*. The recipe is the `qhy-camera` Phase 6 plan;
whichever vendor is Bazel-ized first pays that cost once.

`cargo rail`: a rev/version bump is a `Cargo.toml`/`Cargo.lock` delta, so the
consuming service rebuilds under the `commit` profile — no special handling.

## Provisioning

- **CI:** an explicit pre-build step pulling the pinned SDK from the cache + `apt
  install libusb-1.0-0-dev`, mirroring `test.yml`'s existing cross-spawn
  pre-build pattern. The SDK is required even for the `simulation`/ConformU jobs
  (only `cargo check`/clippy jobs that don't invoke the linker can skip it).
- **Pi nightly runner** (`scripts/setup-pi-runner.sh`): SDK + `libusb-1.0-0-dev`
  + udev `99-asi.rules`. aarch64 must be confirmed linking and added to the
  matrix.
- Write the CI/Pi/Bazel gating **generically** so the separate `qhy-camera` work
  can reuse it.
- **`ascom-alpaca` prerequisite:** the workspace already pins the
  `ivonnyssen/ascom-alpaca-rs` fork on branch `integration`; the
  `macos-trait-recursion-overflow` fix must be present there for macOS dev/test
  (shared with `qhy-camera`).

## Delivery phasing

The crate is the long pole (~40–50% of effort) and holds the real unknowns
(3-arch FFI linking, a faithful simulator, the first prebuilt-blob Bazel
`crate.annotation`). Once `simulation` works, the driver builds entirely against
it, leaning on the `sky-survey-camera` + `qhy-camera` scaffolding.

- **Phase A — `libzwo-sys`:** ✅ *skeleton stood up* — `bindgen` over `ASICamera2.h`
  + `EFW_filter.h` + `EAF_focuser.h` (parsed as C++ for the bare `bool`),
  `build.rs` unconditional system-link of `ASICamera2` + `EFWFilter` (+ `usb-1.0`,
  `stdc++`/`c++`, `udev`/IOKit; mirror `libqhyccd-sys`). Green `check` + `test` on
  CI (Linux x86_64) and built + tested locally on aarch64. *Remaining (long pole):*
  confirm green link on Pi5 aarch64 CI + macOS arm64.
- **Phase B — `zwo-rs`:** ⚙️ *skeleton stood up* — safe `Sdk`/`Error` surface +
  `simulation`-feature stub. *Remaining:* real safe handles/enums + error mapping +
  the `simulation` backend (camera frames + EFW position/moving); publish 0.1.0.
- **Phase C — Track A:** bare `zwo-camera` serving an empty/sim Camera on `:11122`;
  prove build/link, CI SDK provisioning, Pi5 aarch64, Bazel `requires-cargo`,
  repin-twice — *before* device-trait work.
- **Phase D — design doc + ADR + workspace row + BDD feature files** (design→BDD→
  implementation).
- **Phase E — Track B full Camera:** `Device + Camera` over `zwo-rs` (ROI/bin,
  gain/offset, cooling, readout, exposure state machine, abort + graceful stop,
  PulseGuide, sensor type), config-actions, identity, `spawn_blocking` bridge,
  mock seam.
- **Phase F — EFW `FilterWheel`** fast-follow (4-method trait, position/moving/
  names/offsets), config toggle, BDD/ConformU.
- **Phase G — test + gate + consumer:** BDD + ConformU on the sim backend; `cargo
  rail run --profile commit` + `cargo fmt` green; `rp` `CameraConfig` consumer;
  Bazel `crate.annotation` finish; READMEs/`docs/workspace.md`.

## Concurrency

The ASI/EFW SDKs are blocking C FFI and are **not** safe to call from arbitrary
threads concurrently for a single device. Device state (ROI, binning, gain, target
temp, exposure state machine, filter position) is held under
`parking_lot::RwLock`; **every** SDK call funnels through `tokio::task::spawn_blocking`
with a single logical owner per device (the same discipline `qhy-camera` mandates).

## Future Work

- **EAF focuser** (`EAF_focuser.h` / `libEAFFocuser`) → ASCOM `IFocuserV3`
  (absolute + temperature; `EAFIsMoving` is cleaner than the EFW `-1` trick). Could
  eventually supersede the serial `qhy-focuser` pattern for ZWO users.
- **Video mode** (`ASIStartVideoCapture`) as a high-FPS guiding path.
- **Vendoring the SDK** into `libzwo-sys` later (MIT permits) to drop external
  provisioning entirely — deferred in favour of mirroring `qhyccd-rs`.
- **Backport** the feature-gated-link / SDK-free-simulation improvement to
  `qhyccd-rs` so `qhy-camera`'s default build can also be pure-Rust.
- **CAA rotator** (`CAA_API.h`) if a ZWO rotator is ever in scope.

## References

- Camera scaffolding template: [`sky-survey-camera.md`](../services/sky-survey-camera.md)
- Same-vendor-class precedent: [`qhy-camera.md`](../services/qhy-camera.md) ·
  [`qhy-focuser.md`](../services/qhy-focuser.md)
- [`config-actions.md`](../services/config-actions.md) ·
  [`service-lifecycle.md`](../skills/service-lifecycle.md) ·
  [`development-workflow.md`](../skills/development-workflow.md) ·
  [`testing.md`](../skills/testing.md)
- ASI/EFW SDK (headers + per-arch binaries, MIT): INDI `indi-3rdparty/libasi`
  (`ASICamera2.h`, `EFW_filter.h`, `EAF_focuser.h`, `license.txt`)
- Behavioural references: INDI `indi-asi` (LGPL-2.1+ / GPL-2.0+ per file),
  `github.com/stevemarple/python-zwoasi`, INDIGO `indigo_drivers/{ccd,wheel,focuser}_asi`
- FFI crates to be created: `zwo-rs` + `libzwo-sys` (this repo's author, siblings
  to `qhyccd-rs` / `libqhyccd-sys`)
