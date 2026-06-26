# ToupTek Camera Alpaca Driver (`touptek-camera`) + `touptek-rs` FFI

## Status

**Phase A FFI binding spiked & proven (2026-06-25).** `crates/touptek-rs/libtoupcam-sys`
exists: vendored `toupcam.h` (v60.31631) + `bindgen` `build.rs` (parsed as plain C)
+ the `TOUPCAM_SKIP_NATIVE_LINK` env gate. `bindgen` generates **compiling**
bindings on **aarch64 Linux** (the Raspberry Pi target) — 225 `Toupcam_*`
functions, 17 structs, 498 constants, with every needed API/type/flag present
(`EnumV2`, `StartPullModeWithCallback`, `PullImageV4`, `Trigger`, `put_Roi`,
`put_ExpoTime`/`put_ExpoAGain`, `get_Temperature`, `ST4PlusGuide`,
`OPTION_BINNING/TEC/RAW/BITDEPTH/TRIGGER`, `FLAG_TEC/RAW16/ST4`,
`PIXELFORMAT_RAW16`, `ToupcamDeviceV2/ModelV2/FrameInfoV3`). `cargo clippy` + `cargo
fmt` clean. The **real native link** (each platform) and **Bazel wiring**
(`BUILD.bazel` two-variant + `crate_universe` repin + `parity.yml`) are the
remaining Phase A work; the safe `touptek-rs` wrapper is Phase B. Everything else
below remains **planned**.

This is the agreed decision record that will precede
`docs/services/touptek-camera.md` (the service design doc) and the BDD scenarios,
per the design→BDD→implementation flow in
[`docs/skills/development-workflow.md`](../skills/development-workflow.md). It is
the **third** ASCOM Alpaca camera driver in the repo and the **second** built on a
`bindgen` FFI crate, following the now twice-proven template of
[`qhy-camera`](../services/qhy-camera.md) ([qhyccd-rs](vendor-qhyccd-rs.md),
ADR-009) and [`zwo-camera`](../services/zwo-camera.md)
([zwo-rs](vendor-zwo-rs.md), ADR-010 — explicitly "the reference for any future
bindgen-based `*-sys` crate"). The ToupTek SDK is OEM-rebranded as Altair, Omegon,
Meade, Bresser, Mallincam, RisingCam/Ogma, SVBony, StarShootG, Nncam, Tscam — all
the **same ABI with a swapped symbol prefix** — so one driver covers the whole
family.

**Goal of this plan: a cross-platform *working* driver** — green build/link and a
real ConformU pass on **Linux x86_64 + aarch64 (Raspberry Pi), macOS arm64/x64,
and Windows x64**, driving real ToupTek hardware. Scope sequence:
**Camera first**; ST4 `PulseGuide` is in-MVP (the SDK exposes it natively);
filter-wheel / focuser are out of scope (ToupTek does not ship those in this SDK).

> **Licensing is explicitly DEFERRED in this plan (per request).** We proceed on
> the de-facto-permitted footing that INDI and INDIGO already rely on (both vendor
> `toupcam.h` + the prebuilt per-arch `.so` directly). The redistribution question
> is real but is a *publish/vendor-the-binary* gate, **not** an engineering
> blocker for a working driver — see [Licensing (deferred)](#licensing-deferred).

## Motivation

rusty-photon needs a first-class ASCOM Alpaca driver for ToupTek cameras (and the
large family of OEM rebrands), exposing exposures, ROI/binning, gain/offset,
cooling, RAW readout, and ST4 pulse-guiding over Alpaca on a fixed port so the
`rp` orchestrator and any Alpaca client (NINA, SharpCap, PHD2) can drive them like
any other device. This mirrors what `zwo-camera`/`qhy-camera` do for ZWO/QHYCCD
hardware, reusing the same `ascom-alpaca` server framework and the
`sky-survey-camera` (simulator camera) scaffolding.

The behaviour is derived from open ToupTek drivers as a **behavioural reference
only** (no code copied — see [Behavioural reference](#behavioural-reference)), the
same posture `zwo-camera`/`qhy-camera` took toward `indi-asi`/`qhyccd-alpaca`.

## Headline: how ToupTek differs from ZWO/QHY (this drives every decision)

The two existing camera drivers assumed a **blocking snap-mode** SDK
(`start → poll → download`). ToupTek inverts exactly that one assumption, which is
the single biggest design item in this plan. Everything else is *easier* than ZWO.
All facts below are verified against primary sources (the vendored `toupcam.h`
v60.31631, ToupTek's own SDK readme + FAQ, INDI/INDIGO packaging); each
load-bearing claim was adversarially fact-checked.

| Concern | ZWO / QHY (the precedents) | ToupTek (this plan) |
|---|---|---|
| **SDK API model** | Blocking `ASIStartExposure`/`*GetExpStatus`/`*GetDataAfterExp` poll loop — maps trivially onto a synchronous `CameraHandle` | **Callback/event-driven *PullMode*** (`Toupcam_StartPullModeWithCallback` → frame-ready event → `Toupcam_PullImageV4`). **The #1 design pole**: bridge the event model onto the blocking seam (use `OPTION_TRIGGER=1` + `Toupcam_Trigger(h,1)` for discrete ASCOM exposures). |
| **Rust FFI layer** | ZWO: we built `zwo-rs`+`libzwo-sys` from scratch | **Same** — no usable Rust crate exists (`whitequark/rust-touptek` archived 2019, ToupLite-era); we build `touptek-rs` + `libtoupcam-sys` |
| **FFI input size** | ZWO: **three** headers + three libs (`ASICamera2`+`EFWFilter`+`EAFFocuser`) | **One** header (`toupcam.h`), **one** lib (`libtoupcam`), no separate wheel/focuser surface → *simpler* bindgen + link |
| **Binning** | ZWO/QHY: on-sensor (charge-domain) hardware binning | **Digital binning** (`OPTION_BINNING`: sum / average) — usable for `BinX`/`BinY` but **must not be advertised as hardware binning** |
| **OEM coverage** | One vendor each | **~11 OEM brands share one ABI** via a symbol-prefix swap → the whole family comes nearly for free |
| **Licensing** | ZWO MIT (public cache); QHY closed (internal tier) | **Deferred** in this plan; closed-blob, no written grant → treat like QHY until resolved |

Net: ToupTek is **mechanically easier than ZWO** (single header/lib, all arches
shipped incl. Apple-Silicon-universal and Pi) but has **one genuinely new design
pole** (the callback→blocking bridge) plus the usual new-vendor CI provisioning.

## Verified SDK facts

**SDK, platforms & arch** (header version `60.31631.20260606`)
- One native lib per platform: `libtoupcam.so` (Linux), `libtoupcam.dylib`
  (macOS), `toupcam.dll` (Windows); single header `toupcam.h`. Closed-source
  prebuilt binary (INDIGO states this verbatim).
- **Arch matrix covers every target**, including Raspberry Pi: Linux
  `x86_64` (glibc 2.14+), `x86` (2.8+), **`arm64`/aarch64 glibc 2.17+**,
  `arm64`/musl, **`armhf`**, `armel`, `ostl`; **macOS 11+ universal (x86_64 +
  arm64 / Apple Silicon)**; Windows `x64`/`x86`/`arm64`/WinRT; Android. Raspberry
  Pi OS (bookworm, glibc ~2.36) is well above the 2.17 floor → the 64-bit `arm64`
  `.so` is the right pick for a 64-bit Pi. *Confirmed in practice:* INDI vendors
  `libtoupcam.bin` under `arm64/` + `armhf/`; INDIGO ships per-arch
  `libtoupcam.so`. **Pitfall:** naive arch auto-detection can grab the `arm64`
  blob on an `armhf` host — select by **target triple, not host**.
- System deps: `libusb-1.0` + `libudev` on Linux, plus a udev rule for the ToupTek
  USB VID (INDI ships `99-toupcam.rules`). macOS `.dylib` needs an
  `install_name_tool` fixup before linking (INDI automates it).

**API model — plain flat C, bindgen-friendly, but event-driven**
- Lifecycle: `Toupcam_EnumV2()` → `ToupcamDeviceV2[]` (`id`, `displayname`,
  `ToupcamModelV2{flags, maxres, xpixsz/ypixsz}`) → `Toupcam_Open(id)` /
  `OpenByIndex(idx)` → `HToupcam` handle → configure (`put_Roi`, `put_ExpoTime`,
  `put_ExpoAGain`, `Toupcam_put_Option`) → start.
- **PULL mode** (the ASCOM path): `Toupcam_StartPullModeWithCallback(h, cb, ctx)`;
  the callback only delivers an **event code**
  (`IMAGE`/`STILLIMAGE`/`EXPOSURE`/`ERROR`/`DISCONNECTED`); the app then calls
  `Toupcam_PullImageV4()` to copy the frame (+ a `ToupcamFrameInfoV3/V4` metadata
  struct). `Toupcam_Stop()` / `Toupcam_Close()` to tear down.
- **Discrete exposures:** set `OPTION_TRIGGER=1` and call `Toupcam_Trigger(h, 1)`
  to take exactly one frame (vs free-running video). This is the
  `StartExposure`/`ImageReady` path.
- **Threading (load-bearing):** callbacks run on an **internal SDK thread**, and
  the header explicitly warns *"Do NOT call `Toupcam_Close`/`Toupcam_Stop` in this
  callback context — it deadlocks."* The clean Rust pattern: the `extern "C"`
  trampoline only signals a channel/`Notify`; **your own thread** does
  `PullImageV4` + `Stop`/`Close` (see [Concurrency](#concurrency)).
- Return convention is **Windows-style `HRESULT`** (`S_OK`/`S_FALSE`/`E_*`), unlike
  ASI/QHY enum returns → `error.rs` maps these.

**Capability coverage vs ASCOM `ICameraV3`** (all present in the current header)

| ASCOM need | ToupTek SDK | Notes |
|---|---|---|
| Exposure | `put/get_ExpoTime` (µs), `get_ExpTimeRange` | discrete via trigger mode |
| Gain | `put/get_ExpoAGain` (**percent**, 100 = 1.0×/min), `get_ExpoAGainRange` | expose the integer; no named `Gains[]` list. HCG/LCG via `OPTION_CG` |
| Offset | `OPTION_BLACKLEVEL` (also per-frame `FrameInfo.blacklevel`) | **max scales with bit depth** (×4…×256) — **no `OffsetMin/Max` accessor; driver computes the range per bit depth** |
| ROI / subframe | `put/get_Roi(x,y,w,h)` | offsets **and** sizes must be **even**, min 8×8 |
| Binning | `OPTION_BINNING` (low nibble factor 2–8; high bits sum `0x40` / avg `0x80`) | **digital**, not hardware |
| Cooler / TEC | `OPTION_TEC` (on/off), `OPTION_TECTARGET` or `put_Temperature` (0.1 °C), `get_Temperature` (0.1 °C); `OPTION_FAN` | capability flags `FLAG_TEC`/`TEC_ONOFF`/`GETTEMPERATURE` |
| Cooler power | `OPTION_TEC_VOLTAGE` / `OPTION_TEC_VOLTAGE_MAX` | **0–100 % mapping unconfirmed** → may be model-specific or `NotImplemented` |
| Bit depth | `OPTION_BITDEPTH` (0=8-bit, 1=high), `OPTION_PIXEL_FORMAT` | flags `FLAG_RAW8/10/12/14/16` |
| RAW readout | `OPTION_RAW=1` + `OPTION_BITDEPTH=1` + `PIXELFORMAT_RAW16` | **required for astro**; `get_PixelFormatSupport` enumerates RAW8…RAW16 |
| Sensor type | `get_MonoMode` (`S_OK` mono / `S_FALSE` color), `get_RawFormat` (Bayer FOURCC), `ModelV2.xpixsz/ypixsz` | → `SensorType` + `BayerOffsetX/Y` + `PixelSizeX/Y` |
| Pulse guide | `Toupcam_ST4PlusGuide` (gated on the ST4 capability flag) | **native** → cheap `CanPulseGuide` |

**ASCOM optional metadata the SDK does *not* expose** → `NotImplemented` or
model-specific (matching ZWO/QHY): `FullWellCapacity`, `ElectronsPerADU`, named
`ReadoutModes`/`FastReadout` for HCG/LCG conversion gain.

## ASCOM mapping — wins and watch-outs

**Wins (ToupTek supports things cleanly):**
- **Native ST4 `PulseGuide`** (`Toupcam_ST4PlusGuide`) → `CanPulseGuide = true`,
  in-MVP (QHY deferred this; ZWO has it).
- **Trigger mode** gives clean, defined single-frame exposures — a good match for
  `StartExposure`/`ImageReady` without wrestling a free-running video stream.
- **16-bit RAW + Bayer FOURCC** are first-class → faithful `SensorType` and
  `BayerOffsetX/Y` for the color OEM models.
- **One ABI, all platforms** — the same flat C header for Linux/macOS/Windows +
  Apple-Silicon-universal `.dylib` + Pi `arm64` `.so` makes the cross-platform
  goal tractable.

**Watch-outs (ToupTek-specific friction):**
- **Callback/PullMode → blocking bridge** — the exposure state machine
  (`StartExposure → ImageReady → ImageArray`) must be driven by the frame-ready
  event, not a poll. This is where the ConformU timing bugs will surface (cf.
  ZWO's macOS `StartExposure` real-clock-deadline bug).
- **`ImageArray` transpose** — ASCOM `ImageArray` is column-major `[x, y]`; the
  pulled frame is row-major → transpose on readout (same as ZWO/QHY).
- **Offset range computed, not read** — derive `OffsetMin/Max` per bit depth.
- **`CoolerPower`** — confirm `OPTION_TEC_VOLTAGE` yields a clean 0–100 %; else
  `NotImplemented` or model-specific scaling.
- **Even-number ROI** constraint when validating `StartX/Y`/`NumX/NumY`.
- **Device identity / OEM rebrands** — `Toupcam_EnumV2` gives a `displayname` + a
  device `id`/serial string; mint a stable `UniqueID` (`TOUPTEK:{name}:{id}`,
  `noserial-{index}` fallback as ZWO does for the ASI1600) that survives
  reconnects, and handle the rebrand VID/PIDs in the udev rule.
- **No SDK simulator** — like ZWO (unlike QHY), the `simulation` path must
  fabricate frames in `touptek-rs`; fabricated frames must respect ConformU's
  10 s `StartExposure` timeout (reuse zwo-rs's seeded-xorshift `fill_noise`).

## Behavioural reference

- **INDI `indi_toupbase`** (indilib/indi-3rdparty, C++, maintained through 2026) —
  the **gold-standard reference**: ~1,650 LOC over `indi_toupbase.cpp` +
  `libtoupbase.cpp`, exercising the entire ASCOM-relevant surface over the public
  API (trigger exposure, analog gain, black-level/offset, TEC setpoint + voltage
  read, fan, binning, ROI, 8/16-bit RAW + Bayer, pull-mode callback lifecycle,
  white balance, auto-exposure, framerate, **ST4 pulse-guide**). Use it as the
  behavioural spec for `OPTION_*` flags and the pull-mode lifecycle. **License is
  LGPL-2.0-or-later** → read for behaviour, **clean-room reimplement, never copy
  code** into MIT/Apache rusty-photon (same discipline as ZWO/QHY). Its CMake
  build also proves the OEM family via an `FP()` symbol-prefix macro:
  Toupcam, Altaircam, Bressercam, Ogmacam, Tscam, SVbonycam, StarShootG, Nncam,
  Mallincam, OmegonProcam, Meadecam.
- **INDIGO `ccd_touptek`** (C) — a co-equal second cross-platform reference and the
  source of the in-tree header/binaries.
- **Vendor closed ASCOM `ICameraV3` driver** (C#/.NET over `toupcam.dll`, drives
  NINA/PHD2/SharpCap) — the **existence proof** that the public SDK is
  feature-complete for a full ASCOM camera (no private entry points needed).
- **`whitequark/rust-touptek`** — **archived 2019, do NOT adopt.** Its only value
  is proving the ABI binds cleanly to Rust (a broad 100-method safe wrapper over
  the same header). There is **no `toupcam-sys` raw crate** and no maintained Rust
  binding — we write it from scratch (bindgen + the INDI reference de-risk it).

## Licensing (deferred)

Per request, licensing is **out of scope for this plan** and must not block the
engineering work. For the record, so it is not forgotten:

- `toupcam.h` carries **no license header** (only a version comment) — materially
  weaker than ZWO's embedded MIT grant. The runtime libs are **proprietary
  closed-source blobs**; the SDK ships **no EULA/LICENSE/COPYING**. The
  `COPYING.LGPL` in `indi-3rdparty/libtoupcam` covers INDI's *packaging glue*, not
  the ToupTek binary. The driver is not in Debian main/non-free.
- Redistribution is **de facto practiced everywhere** (INDI + INDIGO commit the
  header and per-arch `.so` directly) but rests on **no written grant**.
- **Working assumption for this plan:** follow INDI/INDIGO precedent for *local
  dev/CI*. **Before any `cargo publish` or vendoring the `.so` into the public R2
  cache**, this must be resolved — either confirm redistribution in writing with
  ToupTek (`support@touptek.com`) or vendor **only the header** and fetch the
  binary at build/CI time. Until then, treat the SDK like the **QHY closed blob**:
  authenticated/internal cache tier, not ZWO's public anonymous-read mirror.

## Decisions (proposed)

| Area | Decision |
|---|---|
| **FFI crates** | New first-party siblings, **nested** like zwo-rs (ADR-010): **`crates/touptek-rs/`** (safe wrapper) with **`crates/touptek-rs/libtoupcam-sys/`** (raw `bindgen` over `toupcam.h`). The `lib`-prefixed sys name matches `libzwo-sys`/`libqhyccd-sys`. |
| **FFI pattern** | **ZWO bindgen model** (single header + `dylib=`), *not* QHY's hand-written `extern "C"`. `links = "toupcam"`; `build.rs` bindgen (allowlist `Toupcam.*`/`TOUPCAM.*`), per-OS link dirs, **`TOUPCAM_SKIP_NATIVE_LINK` env gate** (env, not a feature — `--all-features` would flip a feature on everywhere and stop real builds linking). |
| **`simulation` feature** | In `touptek-rs` (`= ["rand", "rayon"]`), fabricating frames in-Rust (no SDK simulator). `simulation` removes the **camera**, **not** the link. `mock = ["simulation"]`, `conformu = ["mock"]` on the service (workspace-wide convention). |
| **Callback bridge** | Pull mode + trigger; `extern "C"` trampoline **signals only** (channel/`Notify`), a dedicated owner thread does `PullImageV4` + `Stop`/`Close`. Never re-enter the SDK from the callback. |
| **SDK delivery / link** | System-installed / CI-provisioned; the native lib links at compile time on every real build (sim links nothing via `TOUPCAM_SKIP_NATIVE_LINK=1`). |
| **Cargo wiring** | `touptek-rs = { path = "crates/touptek-rs" }` in `[workspace.dependencies]`; service uses `{ workspace = true }`. `ascom-alpaca`, `rand`, `rayon`, `bindgen` are **already** in the graph → no new crates.io dep. Keep a `touptek-rs = { workspace = true, features = ["simulation"] }` **dev-dep** on the service as the `crate_universe` rand/rayon nudge (see rule 10). |
| **Bazel wiring** | Own `BUILD.bazel` with the **two-variant** pattern (ADR-010): `touptek-rs` (real, `crate_features=[]`) + `touptek-rs_sim` (`testonly=True`, `crate_features=["simulation"]`); sys crate via `cargo_build_script` running bindgen in-sandbox (`data` = vendored header + `wrapper.h`; `LIBCLANG_PATH` already forwarded in `.bazelrc`). Service = 6 targets (lib+binary real, lib_sim+sim binary, unit_test, conformu_integration, bdd). `parity.yml` is unforgiving → all targets present from PR 1. |
| **Service shape** | One `touptek-camera` (Camera only), enumerate-all, port **`11123`** (next free in the camera `1112x` family after qhy `11121` / zwo `11122`; the `11123` in `rp.md` is an illustrative placeholder, not a real assignment). |
| **Device identity** | `Toupcam_EnumV2` `displayname` + device `id`/serial → `UniqueID = TOUPTEK:{name}:{id}`; `noserial-{index}` fallback; no `unique_id` config field (follows zwo/qhy). |
| **Cross-platform target** | **Linux x86_64 + aarch64 (Pi), macOS arm64/x64, Windows x64** — all must build/link and pass ConformU. |
| **Branch discipline** | All work on a feature branch (never `main`). |

## Provisioning

- **CI (`.github/actions/install-toupcam-sdk`, NEW):** a composite action mirroring
  `install-zwo-sdk`, run only where the **real** link is exercised — `bazel.yml`,
  `bazel-coverage.yml`, `conformu.yml`, `native.yml` (Cargo MSVC source-of-truth),
  `pi-nightly.yml`, `scheduled.yml`. Each OS is its own bring-up: Linux
  x86_64/aarch64 (`+ libusb-1.0` + udev rule), macOS arm64/x64 (`install_name`
  fixup), Windows x64 (import lib + DLL on `PATH`), and the **sudo-free Pi aarch64**
  path (stage under `RUNNER_TEMP`, symlink `libusb`/`libudev`, export
  `TOUPCAM_SDK_LIB_DIR` / `LD_LIBRARY_PATH`). This is the practical long pole.
- **Simulation path links no SDK:** `libtoupcam-sys`'s `build.rs` honours
  `TOUPCAM_SKIP_NATIVE_LINK=1` and emits no link directives, so the default
  `bazel test //...` (sim variant) and any ASan/LSan jobs build/test
  `touptek-camera` with **zero** SDK provisioning.
- **`ascom-alpaca` prerequisite:** the workspace already pins the
  `ivonnyssen/ascom-alpaca-rs` fork (branch `integration`, features
  `server`/`camera`/`client`).

## Delivery phasing

The `touptek-rs` crate is the long pole (~40–50 % of effort) and holds the real
unknowns: the **callback→blocking bridge**, a faithful simulator, and the 4-target
cross-platform link (incl. Pi aarch64 + Apple-Silicon-universal). Once
`simulation` works, the driver builds entirely against it, leaning on the
`sky-survey-camera` + `zwo-camera` scaffolding.

- **Phase A — `libtoupcam-sys`:** ✅ *bindgen spiked & proven* — `bindgen` over
  `toupcam.h` parsed as **plain C** (no bare `bool`; `windows.h` is `_WIN32`-guarded;
  `HRESULT`→`int` on non-Windows), `build.rs` per-OS link of `toupcam` (+ `usb-1.0`,
  `udev` / IOKit+CoreFoundation), `TOUPCAM_SKIP_NATIVE_LINK` gate. Compiling
  bindings generated on **aarch64 Linux** via cargo (link-skipped); clippy/fmt
  clean. *Remaining:* the **real native link** against a provisioned SDK on each
  platform — Pi5 aarch64, macOS arm64 (`install_name`), Windows x64
  (`HRESULT`/`c_long` widths — bit ZWO) — and the **Bazel wiring** (`BUILD.bazel`
  two-variant + `crate_universe` repin + `parity.yml`).
- **Phase B — `touptek-rs`:** safe `Sdk`/`Camera`/`Error` surface + the
  **callback/PullMode → blocking bridge** + the `simulation` backend (fabricated
  frames; ConformU-safe fill). The genuinely net-new design work.
- **Phase C — bare service:** `touptek-camera` serving an empty/sim Camera on
  `:11123`; prove build/link, the new CI SDK provisioning, Pi5 aarch64, the Bazel
  two-variant + `parity`, repin-twice — *before* device-trait work.
- **Phase D — design doc + workspace row + BDD feature files**
  (`docs/services/touptek-camera.md` with the "Native dependency & build gating"
  crux section, then ~7 `.feature` files via the typed `ascom-alpaca` client):
  enumeration/connection, exposure (incl. trigger + abort/stop), binning_and_roi,
  gain_offset_readout, cooling, sensor_properties, pulse_guide, config_actions.
- **Phase E — full Camera:** `Device + Camera` over `touptek-rs` (ROI/bin,
  gain/offset, cooling, RAW readout + transpose, exposure state machine driven by
  the frame-ready event, abort + stop, ST4 `PulseGuide`, sensor type),
  config-actions, serial identity, `spawn_blocking` bridge, `backend.rs` mock seam.
- **Phase F — test + gate + ConformU:** BDD + ConformU on the sim backend to
  **0 errors / 0 issues** (like zwo/qhy); wire ConformU into `conformu.yml` +
  the `native.yml` nightly real-link build (Linux/macOS/Windows); the Bazel
  real/sim two-variant build is the coverage source.
- **Phase G — consumer + cross-platform sign-off:** the `rp` `CameraConfig`
  consumer (`alpaca_url: http://localhost:11123`); confirm a real-hardware
  ConformU pass on **each** target platform (the working-driver goal).

**Local quality gate (cargo-rail retired, #406):** before every commit run
`bazel build //... && bazel test //...`, then `cargo fmt` and
`cargo clippy --all-targets --all-features -- -D warnings`. Run the camera BDD
explicitly with `bazel test --test_tag_filters=bdd //...`. A final `cargo build`
is still worth it for the linker-visible SDK link. Adding the workspace members
needs `CARGO_BAZEL_REPIN=1 bazel mod tidy && bazel mod tidy` (rule 10).

## Concurrency

The toupcam SDK is blocking C FFI **and** delivers frames via a callback on an
**internal SDK thread** that **deadlocks if you call `Stop`/`Close` inside it**.
Design: device state (ROI, binning, gain, target temp, exposure state machine) is
held under `parking_lot::RwLock`; the `extern "C"` callback trampoline is
`Send`-safe, **non-re-entrant**, and only signals a `tokio::sync::Notify`/channel;
a single logical owner per device does `PullImageV4` + `Stop`/`Close` off the
callback thread, and **every** SDK call funnels through `tokio::task::spawn_blocking`
(the same discipline as zwo/qhy, plus the callback-bridge constraint).

## Future Work

- **Push mode / video** (`Toupcam_StartPushModeV4`, high frame rate) as a future
  high-FPS guiding path.
- **OEM-brand expansion** — the `FP()` prefix-swap makes Altair/Omegon/Meade/
  Bresser/RisingCam/etc. a thin follow-on once the Toupcam path works.
- **Vendoring the SDK binary** into the public cache — **gated on the licensing
  resolution** (see [Licensing (deferred)](#licensing-deferred)).
- **Conversion gain (HCG/LCG)** as named `ReadoutModes` / `Gains[]` if a model
  warrants it.

## References

- Same-vendor-class precedents: [`zwo-driver.md`](zwo-driver.md) ·
  [`zwo-camera.md`](../services/zwo-camera.md) ·
  [`qhy-camera.md`](../services/qhy-camera.md)
- Vendoring playbook: [`vendor-zwo-rs.md`](vendor-zwo-rs.md) ·
  [`vendor-qhyccd-rs.md`](vendor-qhyccd-rs.md) ·
  [ADR-010](../decisions/010-vendor-zwo-rs.md) ·
  [ADR-009](../decisions/009-vendor-qhyccd-rs.md)
- Camera scaffolding template: [`sky-survey-camera.md`](../services/sky-survey-camera.md)
- [`config-actions.md`](../services/config-actions.md) ·
  [`service-lifecycle.md`](../skills/service-lifecycle.md) ·
  [`development-workflow.md`](../skills/development-workflow.md) ·
  [`testing.md`](../skills/testing.md) · [`pre-push.md`](../skills/pre-push.md)
- ToupTek SDK (header + per-arch binaries): INDI `indi-3rdparty/libtoupcam`
  (`toupcam.h`, `99-toupcam.rules`, per-arch `libtoupcam.bin`); INDIGO
  `indigo_drivers/ccd_touptek/bin_externals/libtoupcam`; ToupTek download centers
  (`touptek-astro.com/downloads`, `touptekphotonics.com` SDK) + FAQ 47 (ARM/Linux).
- Behavioural references (read-only, clean-room): INDI `indi_toupbase`
  (LGPL-2.0-or-later), INDIGO `ccd_touptek` (C).
- FFI crates to be created: `touptek-rs` + `libtoupcam-sys` (this repo's author,
  siblings to `zwo-rs`/`libzwo-sys`).
