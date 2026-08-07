# zwo-camera — three cameras on one service, Fedora Linux (2026-08-07)

One `zwo-camera` instance serving **three physically attached ZWO bodies**,
each validated with both ConformU suites. The run exists to put the shared
`rusty-photon-camera-core` refactor on real hardware: the ROI rule set, the
Bayer offsets and the single-plane `ImageArray` unpack had just moved out of
the three drivers into one crate, and none of that had been exercised against
a physical sensor.

## What was tested

| | |
|---|---|
| Commit | `28bdd094` (branch `feat/camera-core-image-array`, PR #926 — `main` + the camera-core refactor under test) |
| Platform | Fedora Linux 44 x86_64 |
| Binary | `cargo run -p zwo-camera` — **default features**, i.e. the production non-`simulation` path `zwo-camera → zwo-rs → libzwo-sys → libASICamera2.so` |
| ZWO SDK | `/usr/local/lib/libASICamera2.so`, `99-asi.rules` udev rule |
| ConformU | 4.4.0 (Build 52526.0ad7f21) |
| Service | one instance, port 11122, three devices at `camera/0`, `camera/1`, `camera/2` |

## Devices and verdicts

All six runs report `ErrorCount` / `IssueCount` / `ConfigurationAlertCount` /
`TimingIssuesCount` **all 0**, and every member returned within its ConformU
response target.

| Device | UniqueID | `alpacaprotocol` | `conformance` |
|---|---|---|---|
| ZWO ASI1600MM-Cool | `ZWO:ZWO-ASI1600MM-Cool:noserial-0` | clean | clean |
| ZWO ASI178MM | `ZWO:ZWO-ASI178MM:1915d5081b090900` | clean | clean |
| ZWO ASI120MC-S | `ZWO:ZWO-ASI120MC-S:1f19470620070900` | clean | clean |

## What each camera reported

| | ASI1600MM-Cool | ASI178MM | ASI120MC-S |
|---|---|---|---|
| Sensor type | Monochrome | Monochrome | RGGB (colour) |
| `BayerOffsetX/Y` | — | — | **1 / 0** (GRBG) |
| Sensor, reported | 4608 × 3504 | 3072 × 2064 | 1280 × 960 |
| `MaxBinX/Y` | 4 | 4 | 2 |
| `PixelSizeX/Y` | 3.8 µm | 2.4 µm | 3.75 µm |
| `MaxADU` | 65504 | 65528 | 65504 |
| `ElectronsPerADU` | 0.00496 | 0.00258 | 0.055 |
| Gain range | 0–600 | 0–510 | 0–100 |
| Offset range | 0–100 | 0–600 | 0–20 |
| `ReadoutModes` | Raw16, Raw8 | Raw16, Raw8 | Raw16, Raw8 |
| Cooling (K1) | `true` | `false` | `false` |
| `CCDTemperature` | 0.0 °C at first read | 37.8 °C | live |

The reported extents are R4-aligned: the ASI1600MM-Cool's raw 4656 × 3520 and
the ASI178MM's 3096 × 2080 are reduced to the largest multiples the whole bin
range can address. The ASI120MC-S is already aligned at bins 1–2 and so is
reported unchanged.

`MaxADU` is the ST3 **saturation threshold**, one quantization step below the
delivered container's full scale — not the ceiling the sensor reaches. The two
12-bit bodies land on 65504 (`4095 << 4 = 65520`, one 16-LSB step down) and the
14-bit ASI178MM on 65528 (`16383 << 2 = 65532`, one 4-LSB step down).

## What this run adds

- **The shared Bayer table, on a colour sensor.** The ASI120MC-S reports
  `BayerOffsetX/Y = (1, 0)`, i.e. GRBG. That is the vendor's `ASI_BAYER_GR`
  travelling through `zwo-rs`'s `BayerPattern::Gr`, the driver's map onto
  `camera-core`'s `BayerPattern::Grbg`, and the shared `offsets()` rule — the
  whole chain the refactor introduced, confirmed against a physical mosaic.
  `Gr`/`Gb` are the pair whose offsets are transposes of each other, so this is
  the case that would have caught a mis-mapping.
- **The shared geometry errors reach the client verbatim.** ConformU's
  sub-frame rejection tests record the `camera-core` `GeometryError` text
  arriving through the new `From<GeometryError> for ASCOMError` conversion:

  ```
  Reject Bad XSize (bin 1 x 1)   OK   Received error: NumX must be a multiple of 8 and NumY a multiple of 2
  Reject Bad XStart (bin 3 x 3)  OK   Received error: StartX + NumX exceeds CameraXSize / BinX
  ```

  Both the message and the `INVALID_VALUE` code now come from one place; this
  is the first hardware evidence that the collapse changed neither.
- **The shared frame unpack, at both depths and on three sensors.**
  `ImageArray` and `ImageArrayVariant` were exercised repeatedly per camera
  across the negotiated `Raw16`/`Raw8` modes, all within response targets — the
  path that was three near-identical copies until this branch.
- **Cooling both ways in one service.** The cooled body ran a `SetCCDTemperature`
  write and returned to its initial cooler temperature in ConformU's post-run
  check, while the two uncooled bodies returned `NotImplemented` for the cooler
  getters throughout.

## Known benign observation

The ASI1600MM-Cool's first `CCDTemperature` read after connect returned
`0.0 °C`. This is the previously documented ASI SDK warm-up artifact — the
`ASI_TEMPERATURE` register is not populated until the SDK's first internal
measurement cycle (~1 s) — not a driver caching defect. The driver reads the
value live with no caching, and ConformU accepted the reading.

## Files

- `<device>-alpacaprotocol.log` — ConformU Alpaca wire-protocol suite
- `<device>-conformance.log` — ConformU full device-interface suite
- `<device>-conformance-results.json` — machine-readable verdict
