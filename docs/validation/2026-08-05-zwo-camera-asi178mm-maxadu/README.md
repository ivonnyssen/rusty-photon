# zwo-camera `MaxADU` re-validation — ASI178MM, 2026-08-05

Re-validation of the **delivered-ceiling `MaxADU`** ([issue #888](https://github.com/rusty-photon/rusty-photon/issues/888))
against a physical ASI178MM, after [#887](https://github.com/rusty-photon/rusty-photon/pull/887)
corrected the formula from `(2^BitDepth) - 1` to the ceiling of the data the
SDK actually delivers.

The ASI178MM is the camera that decides the question. #887's correction was
measured on a single 12-bit ASI120MC-S, and a 12-bit camera cannot distinguish
"ASI left-shifts sub-16-bit data by `16 - BitDepth`" from any other rule that
happens to agree at 12 bits. A 14-bit sensor separates them: the shift model
predicts `MaxADU` 65532, while a model that delivered 14-bit data unshifted
would have needed 16383 after all — the value this driver used to report.

## What was tested

| | |
|---|---|
| Commit | [`269a4cc3`](https://github.com/rusty-photon/rusty-photon/commit/269a4cc3) (`origin/main` at test time) |
| Service | `zwo-camera`, **real-SDK** build (default features, no `ZWO_SKIP_NATIVE_LINK`): `cargo run -p zwo-camera -- --port 11122`; rustc 1.96.0 x86_64-unknown-linux-gnu |
| SDK | `libASICamera2.so` in `/usr/local/lib`, `ASIGetSDKVersion` → **1.41.0.0** |
| udev | `99-asi.rules` (VID `0x03c3`, world-RW node, `usbfs_memory_mb` 200) |
| Platform | Fedora Linux 44 (Workstation), kernel `7.1.4-204.fc44.x86_64`, x86_64 dev box; camera direct on a USB 3 port |
| Camera | ZWO ASI178MM, 14-bit, uncooled mono, `UniqueID` `ZWO:ZWO-ASI178MM:1915d5081b090900` (real `ASIGetSerialNumber`) |
| ConformU | 4.4.0 build 52526, against `http://127.0.0.1:11122/api/v1/camera/0` |

## Verdicts

- **`alpacaprotocol`** — *"no errors, issues or information alerts"*:
  [alpacaprotocol.log](alpacaprotocol.log).
- **`conformance`** — *"no errors, warnings or issues found"* and *"all members
  returned within their target response times"*: [conformance.log](conformance.log),
  [conformance-results.json](conformance-results.json)
  (`ErrorCount`/`IssueCount`/`ConfigurationAlertCount`/`TimingIssuesCount` all
  0, 82 timed members). ConformU validated the negotiated list directly —
  `ReadoutModes Read OK Raw16` / `OK Raw8`, `ReadoutMode Read OK 0`,
  `MaxADU OK 65532`.

## The driver's reported contract (RM1, ST3)

Driven over Alpaca against the running service:

| Check | Contract | Result |
|---|---|---|
| Advertised list | RM1 | `SupportedVideoFormat = [Raw8, Raw16]` → `ReadoutModes` `["Raw16", "Raw8"]` |
| Default mode | RM1 | index 0 (`Raw16`) |
| `MaxADU` per mode | ST3 | 65532 in `Raw16`, 255 in `Raw8` |
| Reconnect | RM1 | mode returns to 0, `MaxADU` to 65532 |
| Sensor extent | R4 | 3096×2080 reported as **3072×2064** (largest multiples of `lcm(8·bin)`=96 / `lcm(2·bin)`=24 over bins 1-4) |
| Identity | — | real serial, not the `noserial-{index}` fallback |

## What the hardware delivers

Measured with `crates/zwo-rs/examples/probe_ceiling.rs` (deliberately
overexposed frames plus the tail of the histogram) and
`probe_formats.rs` (packing at ordinary exposure).

### The shift model is confirmed

At bin 1 every `Raw16` pixel carries **two always-zero low bits** — a left
shift by `16 - 14`, the same rule the 12-bit ASI120MC-S showed with four. So
the packing generalises, and the 16383 previously recorded for this camera was
wrong by a factor of four.

### But the ceiling is 65528, not 65532

Blown-out frames clip hard, one ADC count below full scale — `16382 << 2` :

```
bin 1 gain 510  1000000us: mean  5565, top 65528×53     65520×1     61656×1  61212×2
bin 1 gain 510  5000000us: mean 16653, top 65528×458    65520×3     65460×2  65396×2
bin 1 gain 510 15000000us: mean 44314, top 65528×98477  65520×1327  65460×1307  65396×1381
```

Nothing above 65528 at any exposure; only the pile-up on it grows. The run
below it is smooth, so this is a clip and not the brightest object in view.

The clip does not move:

| varied | ceiling |
|---|---|
| gain 0 / 100 / 300 / 510 (bin 1, 15 s) | **65528** at every gain — at gain 0 a single hot pixel still reaches it |
| bin 1 / 2 / 3 (gain 510, 15 s) | **65528** at every bin |
| exposure 1 s / 5 s / 15 s | **65528** |

End-to-end through the driver (ImageBytes, transmission element type 8 =
`UInt16`), full frame, gain 510, 15 s:

```
pixels        : 6340608 (3072x2064)
mean          : 33889
delivered max : 65528  (13655 px, 0.22%)
advertised MaxADU: 65532
pixels >= MaxADU : 0
```

**Consequence:** a client testing `pixel >= MaxADU` detects no saturation on
this camera, ever — on a frame where 13 655 pixels are pinned at the ceiling.
The 12-bit ASI120MC-S *does* reach its full-scale `4095 << 4 = 65520`, so the
shortfall is a per-sensor property and is not derivable from `BitDepth` or
anything else `ASI_CAMERA_INFO` reports. Tracked as
[#898](https://github.com/rusty-photon/rusty-photon/issues/898); no code was
changed *in this run*, because the fix was a design decision rather than a bug
fix.

> **Resolved 2026-08-05.** ST3 now reports one quantization step below the
> shifted full scale — `((2^BitDepth) - 2) << (16 - BitDepth)` — so this
> camera advertises **65528**, exactly the ceiling measured above. Re-verified
> on the same hardware: the same blown-out frame that gave
> `pixels >= MaxADU` = 0 now gives **6 709**.
>
> **Two ConformU runs are archived here, and they report different `MaxADU`
> values by design.** The `conformance.log` / `alpacaprotocol.log` pair above
> is the *pre-change* run and shows `MaxADU OK 65532`; it is the evidence that
> motivated the margin and is left exactly as taken. The re-verification after
> the change is preserved separately as
> [post-margin-conformance.log](post-margin-conformance.log),
> [post-margin-alpacaprotocol.log](post-margin-alpacaprotocol.log) and
> [post-margin-conformance-results.json](post-margin-conformance-results.json)
> — also clean on both suites (0/0/0/0, 82 timed members), with
> `MaxADU OK 65528`. Read the pair by date: the numbers in the body of this
> record are the pre-change measurements.

### Binning changes the packing, not the ceiling

| bin | packing signature |
|---|---|
| 1 | low 2 bits always zero — left-shifted |
| 2 | low bits populated |
| 3 | low bits populated |
| 4 | low bits populated |

At bin ≥ 2 the SDK combines neighbouring ADC counts, which fills the low bits,
so the shift signature is visible only at bin 1. Since the ceiling is unchanged
across bins, the single published `MaxADU` still describes all of them.

## Scope

This closes the **ASI178MM** half of #888. The **ASI1600MM-Cool** half is
untouched — that camera was not attached. It is 12-bit, so it should now report
65520 (up from the 4095 in the 2026-06-20 record), and whether it reaches that
ceiling or clips short like the ASI178MM is exactly the open question. Re-run
both probes against it when it is next on the bench.
