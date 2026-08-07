# qhy-camera on Linux — QHY178M + CFW, 2026-08-07 (`rusty-photon-camera-core`)

Recorded Linux ConformU run against the same physical QHY178M and 7-slot CFW as
the [July records](../2026-07-28-qhy-camera-qhy178m-cfw-linux-4.4.0/README.md),
taken after the
[`rusty-photon-camera-core`](../../../crates/rusty-photon-camera-core) refactor
that moved the ROI/binning rules, the Bayer offsets, the frame-to `ImageArray`
unpack and the `GeometryError → ASCOMError` conversion out of the three camera
drivers and into one shared crate.

This is the **QHY leg** of that refactor's hardware evidence; the ZWO leg is the
[three-camera run](../2026-08-07-zwo-camera-three-cameras-linux/README.md) taken
the same day. Both put the *same* shared code on real hardware through two
different vendor SDKs, which is the point — the refactor's risk was never one
driver's arithmetic, it was whether one implementation still suits all three.

It also brings the QHY records onto **ConformU 4.5.0**; the newest previous QHY
record was taken on 4.4.0.

## What was tested

| | |
|---|---|
| Commit | [`54a7a168`](https://github.com/rusty-photon/rusty-photon/commit/54a7a168) — `main` at the merge of the refactor (PR #926) |
| Service | `qhy-camera`, **real-SDK** build (default features, no `QHYCCD_SKIP_NATIVE_LINK`) |
| Build | `cargo build --release -p qhy-camera`; rustc 1.96.0 (ac68faa20 2026-05-25) |
| SDK | QHYCCD SDK **26.06.04** — `/usr/local/lib/libqhyccd.so` → `libqhyccd.so.26.6.4.16`, sha256 `f51b92f9189fae7707e98ad334cf52d3c1493a6485f33394b39a18a3f4d5c738` (byte-identical to the July records, so the SDK is not a variable here) |
| Platform | Fedora Linux 44 (Workstation Edition) x86_64, kernel 7.1.4-204.fc44 |
| Camera | QHY178M, 3056×2048, mono, `MaxADU` 65535, `MaxBinX/Y` 2, readout modes `STANDARD MODE`, gain `[0, 51]`, offset `[0, 1023]` — SDK id `QHY178M-222b16468c5966524` |
| FilterWheel | The CFW on that camera's port, 7 slots, same physical `OpenQHYCCD` handle — `CFW-QHY178M-222b16468c5966524` |
| ConformU | **4.5.0** build 53834.49ab847, run against `http://127.0.0.1:11121/api/v1/camera/0` and `.../filterwheel/0` |

`UniqueID`s are unchanged from every earlier QHY record.

## Verdicts

Both devices, both suites, clean:

| Device | `alpacaprotocol` | `conformance` |
|---|---|---|
| Camera | 0 errors, 0 issues, 16 information messages — [log](alpacaprotocol-camera.log) | *"no errors, warnings or issues found"*, 70 timed members — [log](conformance-camera.log), [results](conformance-camera-results.json) |
| FilterWheel | *"no errors, issues or information alerts"* — [log](alpacaprotocol-filterwheel.log) | *"no errors, warnings or issues found"*, 33 timed members, all within target — [log](conformance-filterwheel.log), [results](conformance-filterwheel-results.json) |

`ErrorCount` / `IssueCount` / `ConfigurationAlertCount` / `TimingIssuesCount`
are **0** in both results files.

The camera's 16 informational items are the familiar set — the protocol suite's
four casing variants against each of `ImageArray`, `ImageArrayVariant`,
`LastExposureDuration` and `LastExposureStartTime` before any exposure exists,
answered with in-protocol ASCOM errors over HTTP 200. The July records carried
the same 16.

## What this pins about the shared crate

- **The ROI bounds rules are the shared ones, on the wire.** ConformU's
  `StartExposure` rejection tests are answered with `StartX + NumX exceeds
  CameraXSize / BinX` and `StartY + NumY exceeds CameraYSize / BinY` — those
  strings live in
  [`crates/rusty-photon-camera-core/src/lib.rs`](../../../crates/rusty-photon-camera-core/src/lib.rs),
  not in the driver, so they only reach a client through the new
  `From<GeometryError> for ASCOMError` conversion. All 8 rejection cases (X/Y ×
  size/start × bin 1 and bin 2) pass. The neighbouring `bin 0` / `bin 3`
  rejections are still driver-local text, as intended — supported bin *sets* are
  a vendor fact, bounds arithmetic is not.
- **The `ImageArray` unpack works through a second SDK.** The conformance suite
  reads `ImageArray` and `ImageArrayVariant` off real 16-bit frames; the QHY
  driver now reaches them through `camera_core::to_image_array`, dispatching on
  `image.bits_per_pixel` where the ZWO/SVBony drivers dispatch on their own
  `ImageType` enums.
- **Bayer gating holds in the mono direction.** `SensorType` is `Monochrome` and
  `BayerOffsetX/Y` return `NotImplemented`, as ASCOM requires. The colour
  direction is covered by the ASI120MC-S in the ZWO record, which reports
  `BayerOffset (1, 0)` — this camera cannot exercise it.

## A service-log artefact worth not misreading

At `RUST_LOG=info` each `alpacaprotocol` run on the **FilterWheel** leaves
exactly four `CameraNotOpen` / `NOT_CONNECTED` `ERROR` lines in the service log,
always in a burst at the end. This is ConformU racing itself, not a driver
fault, and it reproduced identically in four consecutive runs. Debug-level
tracing gives the sequence:

1. ConformU fires the four ClientID/ClientTransactionID casing variants at the
   **asynchronous** `connect` endpoint.
2. ~0.5 s later, with all four still in flight, it disconnects the device with
   `Connected=False`.
3. ~3 s later the four connects finish their handshake against the handle that
   disconnect has already closed, and answer in-protocol `NotConnected`.

ConformU accepts all four (HTTP 200, and it reports **zero** information alerts
for the wheel), the device is left disconnected — matching the last command it
was given — and every subsequent connect succeeds, including the full
`conformance` run that immediately follows. Recorded here only so the next
operator who greps the service log does not read four red lines as a regression.
