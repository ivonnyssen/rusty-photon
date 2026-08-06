# zwo-camera `MaxADU` re-validation — ASI1600MM-Cool, 2026-08-05

The **12-bit half** of [issue #888](https://github.com/rusty-photon/rusty-photon/issues/888),
measured against the physical ASI1600MM-Cool on the same evening as the
[ASI178MM run](../2026-08-05-zwo-camera-asi178mm-maxadu/README.md). Together
they close #888 and supply the evidence behind the one-step `MaxADU` margin
([#898](https://github.com/rusty-photon/rusty-photon/issues/898)).

This camera's `MaxADU` had never been compared against its delivered pixels.
The 2026-06-20 record listed 4095 — the pre-#887 `2^BitDepth - 1` figure — and
after #887 it was *expected* to report 65520 (`4095 << 4`). It does not.

## What was tested

| | |
|---|---|
| Commit | `222ffe08` on `fix/898-maxadu-quantization-margin`, i.e. `origin/main` (`82cad62b`) plus the margin change under test |
| Service | `zwo-camera`, **real-SDK** build (default features, no `ZWO_SKIP_NATIVE_LINK`): `cargo run -p zwo-camera -- --port 11122`; rustc 1.96.0 x86_64-unknown-linux-gnu |
| SDK | `libASICamera2.so` in `/usr/local/lib`, `ASIGetSDKVersion` → **1.41.0.0** |
| udev | `99-asi.rules` (VID `0x03c3`, world-RW node, `usbfs_memory_mb` 200) |
| Platform | Fedora Linux 44 (Workstation), kernel `7.1.4-204.fc44.x86_64`, x86_64 dev box; camera direct on a USB 3 port |
| Camera | ZWO ASI1600MM-Cool, 12-bit, **cooled** mono, 4656×3520 reported as 4608×3504 (R4), `UniqueID` `ZWO:ZWO-ASI1600MM-Cool:noserial-0` — the documented serial-less fallback |
| Power | 12 V TEC supply connected (see *Cooling* — the first part of the session ran **without** it, so the earlier ConformU pass did not exercise cooling) |
| ConformU | 4.4.0 build 52526, against `http://127.0.0.1:11122/api/v1/camera/0` |

## Verdicts

- **`alpacaprotocol`** — *"no errors, issues or information alerts"*:
  [alpacaprotocol.log](alpacaprotocol.log).
- **`conformance`** — *"no errors, warnings or issues found"*, all 87 timed
  members inside target: [conformance.log](conformance.log),
  [conformance-results.json](conformance-results.json)
  (`ErrorCount`/`IssueCount`/`ConfigurationAlertCount`/`TimingIssuesCount` all
  0). `MaxADU OK 65504`, `ReadoutModes Read OK Raw16` / `OK Raw8`.

Both suites were run **with the TEC powered**, so the cooling members
exercised real hardware rather than an unpowered stub.

## The delivered ceiling: 65504, not 65520

`SupportedVideoFormat` is `[Raw8, Raw16]`, and at bin 1 every `Raw16` pixel
carries **four always-zero low bits** — the `16 - 12` left shift, as expected.

The ceiling is not the shifted full scale. Driven to complete saturation
(gain 600, 15 s, room light):

```
bin 1 gain 600 15000000us: mean 65504, top 65504×16389120
bin 2 gain 600 15000000us: mean 65504, top 65504×4097280
bin 3 gain 600 15000000us: mean 65504, top 65504×1818944
bin 4 gain 600 15000000us: mean 65504, top 65504×1020800
```

Every pixel of the frame — all 16 389 120 of them at bin 1 — sits at exactly
**65504 = `4094 << 4`**, with nothing above, at every bin. The sensor's usable
top ADC code is 4094, not the 4095 the shift predicts.

At lower gains the same ceiling appears as a clip rather than a flat frame
(gain 100 → `65504×28`, gain 300 → `65504×246`), so it is fixed and not an
artifact of over-driving the gain register.

End-to-end through the driver over Alpaca (ImageBytes, transmission element
type 8 = `UInt16`), full frame, gain 600, 15 s:

```
pixels        : 16146432 (4608x3504)
mean          : 65504
delivered max : 65504  (16146432 px, 100.00%)
advertised MaxADU: 65504
pixels >= MaxADU : 16146432
```

**This is the second camera to clip one ADC count short, and the margin lands
exactly on its ceiling.** `((2^12) - 2) << 4 = 65504` is not a conservative
approximation here — it is the measured value. The ASI178MM's
`((2^14) - 2) << 2 = 65528` was likewise exact. The one camera on record as
reaching full scale is the ASI120MC-S at `4095 << 4 = 65520`, but that figure
predates the deliberate-overexposure method used here and is worth re-checking.

> A reporting artifact worth knowing: on a *fully* saturated frame the probe's
> packing column reads "low 5 bits always zero (shifted by 5)". That is just
> `65504 = 0xFFE0` having five trailing zero bits when every pixel is identical
> — the packing test is meaningless on a flat frame. The real signature is the
> four-bit one measured at ordinary exposure.

## Binning changes the packing, not the ceiling

| bin | packing signature |
|---|---|
| 1 | low 4 bits always zero — left-shifted by 4 |
| 2 | low 2 bits always zero — left-shifted by 2 |
| 3 | low bits populated |
| 4 | low bits populated |

Consistent with the SDK averaging the binned pixels: the mean of four
`adc << 4` values is `(Σadc) << 2`, which keeps two zero bits, and at bin 3
the division by 9 fills them entirely. The same effect appears on the
ASI178MM one step earlier (its 2-bit shift is already consumed at bin 2). The
ceiling is unchanged across all four, so one published `MaxADU` describes them
all.

## Cooling (K1-K4), with the TEC powered

The 2026-06-20 record noted the cooler path was exercised live; this run
re-confirms it against the current driver, and adds the measured ramp:

| contract | result |
|---|---|
| K1 | `CanSetCCDTemperature` and `CanGetCoolerPower` both `true` |
| K2 | `CCDTemperature` reads the live sensor value (17.7 °C ambient) |
| K3 | `SetCCDTemperature` 0 °C set and read back |
| K4 | `CoolerOn` → `true`, `CoolerPower` ramps 0 → 24 % while the sensor falls 17.7 °C → 6.2 °C in 120 s |

Switching the cooler off returned `CoolerPower` to 0 % and the sensor began
warming immediately. **Tenet 3 re-confirmed on hardware:** `CoolerOn` read
`false` immediately after every connect — the driver pushes no cooler state on
connect, and no setpoint is restored.

A deliberately modest 0 °C setpoint was used rather than a deep one: it proves
the TEC ramps and regulates without driving it hard on an open bench, and the
camera was left with the cooler off and warming.

## Scope

With this run, **both** cameras named in #888 are measured against their
delivered pixels, and #888 is closed. The ASI120MC-S full-scale figure quoted
in the design doc is the remaining unverified claim in this area; it was taken
before the overexposure method existed.
