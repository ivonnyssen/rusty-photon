# Star Adventurer GTi Service

## Overview

The `star-adventurer-gti` service is an ASCOM Alpaca Telescope driver for the
Sky-Watcher Star Adventurer GTi German Equatorial Mount. It speaks the
[Sky-Watcher motor-controller command set] over either USB-CDC serial or WiFi
(UDP/11880) — the same wire protocol on both transports — and exposes the
mount as an ASCOM Telescope for any Alpaca-compatible client (NINA, SGPro,
`rp`, etc.).

The driver implements the subset of `ITelescopeV3` that the rusty-photon
ecosystem actually needs (slew, sync, track, park, abort, side-of-pier,
PulseGuide), plus the standard device metadata. MoveAxis, custom
tracking rates, and polar-alignment helpers are explicitly deferred
(see [§MVP Scope](#mvp-scope)).

[Sky-Watcher motor-controller command set]: https://inter-static.skywatcher.com/downloads/skywatcher_motor_controller_command_set.pdf

## Architecture

```
┌──────────────────────────────┐         ┌──────────────────────────────┐
│  ASCOM Alpaca clients        │         │  rp service                  │
│  (NINA, SGPro, Voyager, ...) │         │  (mount tools)               │
└──────────────┬───────────────┘         └──────────────┬───────────────┘
               │ HTTP                                    │ HTTP
               └──────────────────┬─────────────────────┘
                                  ▼
                ┌─────────────────────────────────┐
                │  star-adventurer-gti service    │
                │                                 │
                │   ┌──────────────────────────┐  │
                │   │ MountDevice              │  │
                │   │  (ASCOM Telescope impl)  │  │
                │   └────────────┬─────────────┘  │
                │                │ commands       │
                │                ▼                │
                │   ┌──────────────────────────┐  │
                │   │ TransportManager         │  │
                │   │  (ref-counted, polls     │  │
                │   │   axis status + posn)    │  │
                │   └────────────┬─────────────┘  │
                │                │ bytes          │
                │                ▼                │
                │   ┌──────────────────────────┐  │
                │   │ Transport (trait)        │  │
                │   │  ┌──────┐    ┌──────┐    │  │
                │   │  │serial│    │ udp  │    │  │
                │   │  └───┬──┘    └───┬──┘    │  │
                │   └──────┼───────────┼───────┘  │
                └──────────┼───────────┼──────────┘
                           ▼           ▼
                       /dev/ttyACM0   192.168.4.1:11880
                       (USB-CDC)      (WiFi UDP)
                           │           │
                           └─────┬─────┘
                                 ▼
                    ┌────────────────────────────┐
                    │  Star Adventurer GTi mount │
                    │  (Sky-Watcher motor ctrl)  │
                    └────────────────────────────┘
```

The wire protocol is the same on both transports — the [Sky-Watcher
motor-controller command set] §6 (Wi-Fi Connection) is explicit that the
WiFi adapter / built-in WiFi module simply runs a UDP server on port 11880
and accepts the same `:cmd<axis><payload>\r` frames the serial port does.
Empirically verified on this mount in both modes.

### Crate vs service boundary

Protocol logic lives in the `skywatcher-motor-protocol` crate (under
`crates/`); transport, polling, state, and ASCOM mapping live in the
service. The crate is pure (no I/O, no async, no ASCOM types) so it is
miri-clean and trivially property-testable.

| Component | Crate (`skywatcher-motor-protocol`) | Service (`star-adventurer-gti`) |
|---|---|---|
| Frame encode (`Command` → `Vec<u8>`) | ✓ | |
| Frame decode (`&[u8]` → `Response` / `ProtocolError`) | ✓ | |
| 24-bit LE nibble-pair hex codec | ✓ | |
| 0x800000 position-bias add/subtract | ✓ | |
| Mount-error-code → `ProtocolError` | ✓ | |
| Async I/O (serial, UDP) | | ✓ |
| Polling loop, state machine | | ✓ |
| ASCOM trait implementation | | ✓ |
| RA/Dec ↔ encoder-tick math | | ✓ |
| Local-sidereal-time computation | | ✓ |
| Mount-side parameter discovery (`:a`, `:b`, `:g`) | | ✓ |

The crate is intentionally not Sky-Watcher-mount-specific beyond the wire
format: it does not know about RA/Dec, ASCOM, or any particular mount's
gear ratios. A future `az-gti` service or any other mount that speaks this
protocol could reuse the crate verbatim.

## Hardware Constraints

The Star Adventurer GTi is a portable German Equatorial Mount with a
counterweight bar. Two control paths from the host:

- **USB-CDC** (USB-C on the mount). Enumerates as `/dev/ttyACM*`
  (Linux), `/dev/cu.usbmodem*` (macOS), or `COM*` (Windows). The vendor
  spec says **9600 8N1**; in practice the GTi USB also accepts
  **115200 baud**, which we recommend (matches EQMOD documentation and
  is faster). Use the stable `/dev/serial/by-id/...` symlink rather
  than `/dev/ttyACM0` — the unsuffixed device path can shuffle on
  reboot when other USB-CDC peripherals (PPBA, focuser, etc.) are
  present.
- **WiFi** (built-in, AP mode by default). The mount self-hosts an open
  access point and listens on **UDP/11880** at `192.168.4.1`. Same
  protocol; one command per UDP packet, one response per UDP packet.
  The host **must bind the local socket to a 192.168.4.x source
  address** explicitly — relying on the kernel to pick a source IP
  fails when there is a competing default route, and the mount
  silently drops packets it can't reply to.

Mount-side parameters are queried at connect time rather than hard-coded:

| Parameter | Command | Typical GTi value | Used for |
|---|---|---|---|
| Counts per revolution (per axis) | `:a1` / `:a2` | `0x375F00` = 3,628,800 | encoder-tick ↔ angle conversion |
| Timer-interrupt frequency | `:b1` | `0xF42400` ≈ 16 MHz | step-period (T1 preset) calculation |
| High-speed ratio | `:g1` / `:g2` | mount-specific (e.g. 16, 32, 64) | high-speed-slew step-period scaling |
| Motor board version | `:e1` | `0x03300C` (mount type 0x03, fw v0x30.0x0C) | mount-family detection (EQ vs AZ) |

CPR varies between the GTi's RA and Dec axes and between firmware
revisions; the driver reads both rather than assuming.

The protocol exposes **no native park position** and **no site lat/lon**.
We implement software park (target encoder pair sourced from config,
or — failing that — the live snapshot after the optional `home_pose`
encoder seed, see [§Park lifecycle](#park-lifecycle) and
[§Park persistence](#park-persistence)) and require site lat/lon in the
config (see [§ASCOM Telescope Mapping](#ascom-telescope-mapping)).

## Protocol Reference

The motor-controller protocol is a request/response ASCII protocol with
unusual hex encoding and a position-bias convention. Both are easy to get
wrong and both are codec-layer concerns the crate isolates.

### Frame format

```
Command:    : <cmd> <axis> <payload?> \r           1–9 bytes
Response:   = <payload?>                  \r       1–8 bytes  (success)
Response:   ! <2-hex-errcode>             \r       4 bytes    (error)
```

- `<cmd>` is one ASCII letter (case-sensitive). Uppercase = setter / motion
  command; lowercase = inquiry.
- `<axis>` is `'1'` (RA / Az motor), `'2'` (Dec / Alt motor), or `'3'`
  (both axes — only valid for some commands).
- `<payload>` is 1–6 ASCII hex bytes (`'0'`–`'9'`, `'A'`–`'F'`). Empty for
  payload-less commands.
- All frames terminate with a single `\r` (`0x0D`). **No `\n`.** UDP
  responses do include a trailing `\r`.
- If the controller sees a second `:` before `\r`, it discards the partial
  frame and starts fresh — useful for resync after a corrupted packet.

### UDP framing strictness

When the transport is UDP, framing is unforgiving (empirically verified on
the GTi):

| Variant | Behaviour |
|---|---|
| `:e1\r` | reply received |
| `:e1\r\n` | reply received (trailing `\n` tolerated) |
| `:e1` (no `\r`) | silent — controller waits for terminator |
| `:e1\r` + trailing zero-padding | silent — extra bytes after `\r` reject the packet |
| `\xff…:e1\r` (junk-prefixed) | silent — bytes before `:` reject the packet |

The codec must enforce: exactly one well-formed frame per UDP packet,
nothing trailing.

### Hex encoding (24-bit data — low byte first, nibbles in order)

For the 24-bit value `0x123456`, the wire bytes are ASCII
`"5" "6" "3" "4" "1" "2"` — i.e., low byte first, with each byte's nibbles
in normal high-then-low order.

```
0x123456  →  bytes 0x56, 0x34, 0x12  →  ASCII "56" "34" "12"
                                      →  "563412"
```

This is the source of the most common implementation bug; the codec
exposes typed `encode_u24` / `decode_u24` (and `_i24`) helpers and the
service is forbidden from rolling its own hex translation.

### Position-bias offset 0x800000

Axis positions are conveyed with a fixed bias of `0x800000` to keep them
unsigned on the wire while permitting signed-positive and signed-negative
encoder counts:

| Encoder count (true) | Wire value |
|---|---|
| `0` | `0x800000` |
| `+1` | `0x800001` |
| `-1` | `0x7FFFFF` |
| `+0x1234` | `0x801234` |
| `-0x1234` | `0x7FEDCC` |

The codec subtracts on decode and adds on encode; service code sees
signed-`i32` encoder counts only.

### Error codes

| Code | Name | ASCOM mapping |
|---|---|---|
| `0` | UnknownCommand | `INVALID_OPERATION` (programmer error, log) |
| `1` | CommandLengthError | `INVALID_OPERATION` (programmer error, log) |
| `2` | MotorNotStopped | `INVALID_OPERATION` |
| `3` | InvalidCharacter | `INVALID_OPERATION` (programmer error, log) |
| `4` | NotInitialized | propagate as transport error; driver re-issues `:F` |
| `5` | DriverSleeping | propagate; user must re-power mount |
| `7` | PECTrainingRunning | `INVALID_OPERATION` (PEC out of MVP scope) |
| `8` | NoValidPECData | `INVALID_OPERATION` (PEC out of MVP scope) |

### Initialisation sequence

After opening the transport and before the first motion command:

1. `:F1` → expect `=\r` (Initialize axis 1)
2. `:F2` → expect `=\r` (Initialize axis 2)
3. `:a1` → record RA-axis CPR
4. `:a2` → record Dec-axis CPR
5. `:b1` → record TMR_Freq
6. `:g1` → record RA-axis high-speed ratio
7. `:g2` → record Dec-axis high-speed ratio
8. `:e1` → log mount type / firmware version
9. `:j1` / `:j2` → record initial encoder positions

These values seed the in-memory mount-parameters cache used by the
coordinate module and the slew planner.

### Commands used by the MVP

| Command | Purpose | MVP usage |
|---|---|---|
| `:E<axis><pos24>` | Set axis position (sync) | implements `SyncToCoordinatesAsync` |
| `:F<axis>` | Initialize axis | connect handshake |
| `:G<axis><mode2>` | Set motion mode (Goto/Tracking, fast/slow, dir, etc.) | every slew, every track-state change |
| `:H<axis><inc24>` | Set goto-target by increment (magnitude; sign from `:G` CCW) | every slew (INDI-style sequence) |
| `:M<axis><breaks24>` | Set goto break-point increment | every slew (INDI-style sequence) |
| `:S<axis><pos24>` | Set goto absolute target | `Park` only (target is encoder 0) |
| `:I<axis><period24>` | Set step period (T1 preset) | tracking (computed sidereal period from CPR / TMR_Freq); slew (fixed period 6, INDI `minperiods` default) |
| `:J<axis>` | Start motion | every slew, every track start |
| `:K<axis>` | Stop motion (decelerate) | `Tracking = false` (sidereal RA tracking gracefully decelerates) |
| `:L<axis>` | Instant stop | `AbortSlew`; preflight stop-and-wait before every `:G` (slew, park, `Tracking = true`); slew watcher's blocked-axis abort path |
| `:a<axis>` | Inquire CPR | connect handshake |
| `:b1` | Inquire TMR_Freq | connect handshake |
| `:e<axis>` | Inquire motor-board version | connect handshake (logging only) |
| `:f<axis>` | Inquire status | polled while slewing / tracking |
| `:g<axis>` | Inquire high-speed ratio | connect handshake |
| `:j<axis>` | Inquire current position | every position-read |

## ASCOM Telescope Mapping

Every property/method on `ITelescopeV3`, what the driver returns, and why.

### Capability flags

| Flag | Value | Reason |
|---|---|---|
| `AlignmentMode` | `GermanPolar` | GTi is a true GEM with counterweight; meridian-flip semantics apply |
| `EquatorialSystem` | `Topocentric` | Driver convention; the wire protocol is epoch-agnostic, and topocentric matches what host planning code (rp) is wired for |
| `CanSlew` | `true` | implemented |
| `CanSlewAsync` | `true` | implemented (non-blocking; poll `Slewing`) |
| `CanSlewAltAz` | `false` | RA/Dec only in MVP |
| `CanSlewAltAzAsync` | `false` | RA/Dec only in MVP |
| `CanSync` | `true` | implemented via `:E` |
| `CanSyncAltAz` | `false` | RA/Dec only in MVP |
| `CanSetTracking` | `true` | implemented (sidereal only) |
| `CanSetTrackingRate` | `false` | sidereal only in MVP |
| `CanSetRightAscensionRate` | `false` | custom rates deferred |
| `CanSetDeclinationRate` | `false` | custom rates deferred |
| `CanSetGuideRates` | `true` | implemented (`GuideRateRightAscension` / `GuideRateDeclination` are independently settable in deg/sec, defaulting to `0.5 × sidereal`) |
| `CanPulseGuide` | `true` | implemented as rate-shifted tracking (see [§PulseGuide lifecycle](#pulseguide-lifecycle)) |
| `CanMoveAxis(any)` | `false` | manual slew deferred |
| `CanFindHome` | `false` | no hardware home position |
| `CanPark` | `true` | implemented (software park) |
| `CanUnpark` | `true` | implemented |
| `CanSetPark` | runtime-determined | `true` when the driver was started with `--config <path>` (the path it would write back to); `false` for `Config::default()` runs (smoke-tests, no-arg launches). See [§Park persistence](#park-persistence) |
| `CanSetPierSide` | runtime-determined | `true` when `flip_policy.enabled` is set on a hardware-validated mount; `false` otherwise (`SetSideOfPier` returns `NOT_IMPLEMENTED`). See [§Meridian flip](#meridian-flip) |
| `DoesRefraction` | `false` | mount applies no refraction model; host (rp) provides refracted coords |
| `TrackingRates` | `[Sidereal]` | sidereal only in MVP |

### Reads

| Property | Implementation |
|---|---|
| `RightAscension` | from current RA-axis encoder + LST + sync offset |
| `Declination` | from current Dec-axis encoder + sync offset |
| `Azimuth` | derived from RA/Dec + site lat/lon + LST |
| `Altitude` | derived as above |
| `TargetRightAscension` | last `SlewToCoordinatesAsync` / `SlewToTargetAsync` target; `INVALID_OPERATION` if not set |
| `TargetDeclination` | as above |
| `Slewing` | `true` while either axis status (`:f`) reports running in Goto mode; cleared when both stop |
| `Tracking` | driver-state mirror of last `Tracking` setter |
| `TrackingRate` | always `Sidereal` in MVP |
| `GuideRateRightAscension` | RA guide rate (deg/sec). In-memory mirror of the last `SetGuideRateRightAscension` write, default `0.5 × SIDEREAL_DEG_PER_SEC ≈ 0.00209` (i.e. fraction = 0.5 of sidereal). Re-initialised to the default on each `Connected = true`. |
| `GuideRateDeclination` | Dec guide rate (deg/sec). Same shape as RA; settable independently per ASCOM. |
| `IsPulseGuiding` | `true` if either axis has an in-flight pulse (`pulse_guiding_ra || pulse_guiding_dec`); see [§PulseGuide lifecycle](#pulseguide-lifecycle) for the per-axis flag semantics. |
| `AtPark` | driver-state flag; set by `Park`, cleared by `Unpark` and by any motion command |
| `AtHome` | `false` (no hardware home concept) |
| `SideOfPier` | derived from Dec-axis encoder + Dec-axis CPR + site latitude — canonical INDI eqmod convention (`PierSide::East` when `\|dec_encoder\| > cpr_dec/4`, i.e. Dec rotated past either celestial pole). Southern hemisphere inverts. See [§Side-of-pier](#side-of-pier) |
| `DestinationSideOfPier(ra, dec)` | predicts the pointing state the driver would land at for a slew target. Runs the flip-policy decision (current side + target HA + `flip_policy.enabled`), maps to encoder ticks for the chosen side, and validates the per-side safety envelope. With `flip_policy.enabled = false` always returns the current side (driver never plans a flip). See [§Side-of-pier](#side-of-pier) and [§Meridian flip](#meridian-flip) |
| `SiteLatitude` | from config (read-only; setter returns `NOT_IMPLEMENTED`) |
| `SiteLongitude` | as above |
| `SiteElevation` | from config; defaults to `0` |
| `UTCDate` | host clock (per ASCOM convention; setter writes a host-side offset only) |
| `SiderealTime` | computed from `UTCDate` + `SiteLongitude` |
| `SlewSettleTime` | from config; setter is allowed (writes the in-memory cache only, not config file) |

### Writes / methods

| Method | Implementation |
|---|---|
| `Connected = true` | open transport, run init handshake, populate parameter cache, start polling |
| `Connected = false` | stop polling, close transport, clear parameter cache |
| `SlewToCoordinatesAsync(ra, dec)` | validate (not parked, valid coords), compute target encoder positions for `LST(now + MIN_SLEW_DWELL)` so the post-slew RA reading lands on `target_RA` instead of drifting at sidereal rate during the slew, issue `:G` `:S` `:J` per axis, set `Slewing=true`. Returns immediately; caller polls `Slewing` |
| `SlewToCoordinates(ra, dec)` | wraps the async variant and waits for `Slewing` to clear (bounded by a generous timeout) before returning. Mandatory per ASCOM when `CanSlew=true` |
| `SlewToTargetAsync()` | uses last-set `TargetRightAscension`/`Declination` |
| `SlewToTarget()` | synchronous variant of the above; same wait semantics as `SlewToCoordinates` |
| `SyncToCoordinates(ra, dec)` | issue `:E<axis><pos>` for each axis (set encoder position), update the cached snapshot so an immediate `RightAscension` / `Declination` read reflects the sync without waiting for the next background poll, and **update `TargetRightAscension` / `TargetDeclination`** to the synced coordinates (per ASCOM ITelescopeV3 — a successful Sync writes Target) |
| `SyncToTarget()` | uses last-set target |
| `AbortSlew()` | refuse with `INVALID_WHILE_PARKED` when parked; otherwise issue `:L1` `:L2` (instant stop), clear `Slewing`, do NOT auto-restore tracking |
| `Park()` | stop tracking, slew both axes to the in-memory park-target encoder pair (loaded from config or captured at handshake — see [§Park lifecycle](#park-lifecycle)), when both report stopped set `AtPark=true`. **Tracking remains off after park** (per ASCOM) |
| `Unpark()` | clear `AtPark`. Does NOT auto-enable tracking |
| `SetPark()` | capture current encoder pair, write back into the running config file (only the `mount.park_ra_ticks` / `mount.park_dec_ticks` keys are touched — see [§Park persistence](#park-persistence)), update the in-memory park target. Refuses if the driver was started without `--config`, if not connected, or while slewing |
| `SetSideOfPier(side)` | request a meridian flip to the named side. No-op success when `side == current_side`; otherwise issues a through-wrap flip slew to the current target. Returns `NOT_IMPLEMENTED` when `flip_policy.enabled = false`; `INVALID_VALUE` when `side` is `Unknown`; the usual `NOT_CONNECTED` / `INVALID_WHILE_PARKED` / `INVALID_OPERATION` (while slewing) refusals otherwise. See [§Meridian flip](#meridian-flip) |
| `Tracking = true` | refuse with `INVALID_WHILE_PARKED` when parked; otherwise issue `:G<RA>` (Tracking + sidereal) + `:I<RA>` (sidereal step period) + `:J<RA>`. Dec axis untouched |
| `Tracking = false` | issue `:K<RA>` (decelerate to stop). Allowed while parked — Park already left tracking off and a caller re-asserting that should not error |
| `FindHome()` | `NOT_IMPLEMENTED` |
| `MoveAxis(*)` | `NOT_IMPLEMENTED` |
| `PulseGuide(direction, duration)` | rate-shifted tracking pulse on the targeted axis; returns immediately after spawning the watcher task. Refuses when parked / disconnected / slewing / same-axis pulse already in flight. `duration = 0` is a no-op success. See [§PulseGuide lifecycle](#pulseguide-lifecycle) for the wire path. |
| `SetGuideRateRightAscension(deg_per_sec)` | validate `(0, SIDEREAL_DEG_PER_SEC)` exclusive; store as fraction = `deg_per_sec / SIDEREAL_DEG_PER_SEC`. Out-of-range → `INVALID_VALUE`. |
| `SetGuideRateDeclination(deg_per_sec)` | same shape as RA. |
| `Action(name, parameters)` | `ACTION_NOT_IMPLEMENTED` for all action names in MVP |

#### Host-clock dependency for LST-using reads

`RightAscension`, `Azimuth`, `Altitude`, `SiderealTime`,
`SyncToCoordinates`, `SlewToCoordinatesAsync`, and
`DestinationSideOfPier` all compute local apparent sidereal time
from the host's `SystemTime::now()`. (`Declination` reads the
Dec encoder directly and is not LST-dependent.) The conversion
goes through ERFA (`Dtf2d` → `Utctai` → `Gst06a`), and ERFA
refuses host UTCs that `eraCal2jd` rejects — in practice a year
below `IYMIN = -4799`. The leap-second-table boundary (years
before 1960 or beyond `IYV + 5`) is reported as a *warning* in
`Utctai`, not an error, so a future-shifted clock still produces
a valid LST. When ERFA *does* refuse, the trait method returns
`INVALID_OPERATION` with a `timekeeping error: ...` payload
rather than panicking the tokio task. The slew-completion
watcher's pickup loop matches the same pattern: on ERFA failure
it logs `warn!`, clears `slew_in_progress`, and exits cleanly so
the next Alpaca client read of `Slewing` flips to `false`.

### Slew lifecycle

```
SlewToCoordinatesAsync(ra, dec)
   │
   ├─ validate: !AtPark, ra ∈ [0,24), dec ∈ [-90,90]
   ├─ remember: TargetRightAscension/Declination = (ra, dec)
   ├─ pick pier side: flip policy (current side + target HA +
   │           `flip_policy.enabled`) — see [§Meridian flip](#meridian-flip).
   │           With `enabled = false` always chooses the current side.
   ├─ compute: (ra_target_ticks, dec_target_ticks) from
   │           ra/dec + LST(now) + sync offset + chosen pier side.
   │           Pre-flip (pierWest) target is `(LST − ra, dec)`;
   │           flipped (pierEast) target is `(LST − ra + 12 h)` mod 24
   │           folded signed, with dec past the pole at
   │           `sign(dec) · (180° − |dec|)`. The post-stop pickup loop
   │           closes the residual that arises from RA drifting at
   │           sidereal rate during the goto, so the LST snapshot is
   │           taken at issue time.
   ├─ validate per-side safety envelope (pre-flip side reuses
   │           `ra_*_hours` / `dec_*_degrees`; flipped side uses the
   │           mirror band through the encoder wrap at ±12 h —
   │           see [§Meridian flip](#meridian-flip))
   ├─ flip slew? route the RA axis through the negative-`mech_HA`
   │           half (counterweight-below-horizon arc). The wire
   │           sequence below is unchanged; only the encoder target
   │           and `:G`'s CCW bit differ.
   │
   ├─ for each axis (INDI-style wire sequence — see
   │   indi-eqmod/skywatcher.cpp::SlewTo):
   │     :L<axis>      instant-stop motor
   │     poll :f<axis> until Running=0 (max 2 s)
   │     :G<axis><mode>  motion mode = Goto+Fast, CCW bit from sign of delta
   │     :I<axis>6     step period (INDI minperiods default)
   │     :H<axis><|delta|>  target by increment (magnitude only)
   │     :M<axis><breaks>   break-point = min(|delta|/10, 3200)
   │     :J<axis>      start motion
   │
   ├─ Slewing = true
   └─ background poll :f1 / :f2 every 200 ms
        └─ when both axes report Running=0 in Goto mode → wait dwell
        └─ wait at least MIN_SLEW_DWELL (2 s) of wallclock so any
           Alpaca client polling Slewing within the typical
           round-trip window catches `Slewing = true` at least once.
           Tracking is off during this wait — the encoder is static
           but the apparent RA drifts at sidereal rate as LST
           advances. Dwell-before-pickup means the pickup loop sees
           a single accumulated residual once, instead of burning
           through PICKUP_MAX_ITERATIONS chasing dwell drift.
        └─ EQMOD pickup loop (≤ 5 iterations, matches INDI's
           GOTO_ITERATIVE_LIMIT):
             read current RA/Dec from encoders + current LST,
             if |target_RA - current_RA| > 5″ or |target_Dec - current_Dec| > 5″:
               recompute delta against current encoder + current LST,
               re-issue :L → :G → :I → :H → :M → :J for each axis,
               iterate.
        └─ if Tracking was on: re-issue tracking-mode :G + :I + :J on RA axis
        └─ apply config.settle_after_slew before clearing Slewing = false
```

### Park lifecycle

```
Park()
   │
   ├─ if AtPark: return immediately (idempotent)
   ├─ Tracking = false (issue :K on RA axis)
   │
   ├─ for each axis:
   │     :G<axis><goto-mode>      ccw chosen from sign(target − current)
   │     :S<axis><park_target>    target = in-memory park-target ticks
   │     :J<axis>
   │
   ├─ background poll :f1 / :f2 until both stopped
   │     (no auto-abort on timeout — caller must AbortSlew if stuck;
   │      matches rp.md park behaviour)
   ├─ AtPark = true
   └─ Tracking remains false
```

The **park-target encoder pair** is loaded per axis on every connect,
in this order of preference:

1. **Config-file value** (per axis). When the driver was started with
   `--config <path>`, the file is re-read on every connect and
   `mount.park_ra_ticks` / `mount.park_dec_ticks` are used when
   present. Per-axis: if only `park_ra_ticks` is set, RA comes from
   the file and Dec falls through to step 3.
2. **In-memory config value** (per axis). When no `--config` was
   provided (`Config::default()` run), the `MountConfig` defaults are
   used; in this mode `SetPark` is unreachable so these values do not
   change in-process.
3. **Live-snapshot fallback** (per axis). The current encoder
   reading from the [`TransportManager`] snapshot. Two cases:
   - **Fresh power-up with `home_pose` configured.** The driver
     runs [`seed_home_pose_after_connect`](#home-pose) before
     loading the park target, so the snapshot already reflects
     the home_pose's logical encoder values (e.g. `ap_park_3` →
     `mech_HA = -6h`, `mech_dec = +90°`). `Park` therefore
     defaults to "return to the pose the operator powered up at".
   - **Mid-session reconnect** *or* **fresh power-up without
     `home_pose`.** The home_pose seed skips on a non-zero
     firmware encoder (and is a no-op when no `home_pose` is
     configured), so the snapshot equals the
     handshake-captured `:j1` / `:j2` reading. This is the
     "park where the OTA already is" semantic operators expect
     from a reconnect.
4. **Last resort** — encoder `0`. Only reachable if the snapshot
   somehow produced no position read, which today is unreachable.

Re-reading the config file on every connect means a successful
`SetPark` (or a manual operator edit between connects) takes effect on
the next reconnect without restarting the driver. After load the
in-memory target is fixed for the session unless `SetPark` is called
again (see [§Park persistence](#park-persistence)).

### Park persistence

`SetPark` writes the current encoder pair back into the **same JSON
config file** the driver was started with (the `--config <path>`
argument). No separate state file. Concretely:

1. **Capability gate.** `CanSetPark` returns `true` only when the
   driver was started with `--config <path>`. Running on
   `Config::default()` (no `--config`) leaves `CanSetPark = false` and
   `SetPark` returns `ASCOM_NOT_IMPLEMENTED`. The capability flag is
   computed at startup; clients that cache it once per session see a
   stable answer.
2. **Refusal cases.** `SetPark` also refuses (`NOT_CONNECTED` /
   `INVALID_OPERATION`) when the device is disconnected or while a
   slew / park is in progress — the "current encoder pair" wouldn't be
   stable otherwise.
3. **Read-as-`Value`, write only park keys.** The driver reads the
   on-disk JSON via `serde_json::Value`, mutates *only*
   `mount.park_ra_ticks` and `mount.park_dec_ticks`, and serialises the
   result pretty-printed. Any field the driver doesn't recognise
   (future schema additions, operator-added comments-as-fields) is
   preserved as a JSON value. Note: this is a JSON-value preservation,
   not a byte-for-byte one — `serde_json::to_string_pretty` rewrites
   whitespace, and `serde_json::Map` is alphabetical by default
   (BTreeMap, without the `preserve_order` feature), so operator
   formatting and key order do not survive. The *content* outside the
   two park keys is unchanged. The driver **never re-serialises its
   in-memory typed `Config`** to disk — that path would round-trip the
   CLI overrides (`--port`, `--baud`, `--server-port`, `--transport`)
   back into the file and is structurally avoided here.
4. **Atomic rename via `tempfile::NamedTempFile`.** The temp file is
   created in the **same directory** as the destination (required for
   `persist` to use POSIX `rename` rather than copy-and-delete),
   `sync_all`'d (fsync the file data) so a crash after rename can't
   surface a renamed-but-zero-length file, `persist`'d on success,
   then on unix the parent directory is fsync'd so the rename itself
   is durable. Same pattern as
   [`services/rp/src/persistence/document.rs::write_sidecar_sync`](../../services/rp/src/persistence/document.rs).
   On any error path the temp file auto-deletes via `Drop`, so a
   panic mid-write doesn't leave a `*.tmp` artifact behind.
5. **Blocking I/O on the blocking pool.** The whole read+parse+stage+
   fsync+rename sequence runs inside `tokio::task::spawn_blocking` so
   the async runtime isn't held up. Same pattern as `write_sidecar`.
6. **In-memory update follows disk.** The in-memory park target is
   updated only after the file write succeeds. If the file write
   fails, the in-memory park is unchanged and the caller sees an
   ASCOM error — there is no "partial success" state.

Blast-radius bound: even with a logic bug, the only keys the driver
ever writes are `mount.park_ra_ticks` and `mount.park_dec_ticks`. A
broken `SetPark` cannot corrupt `transport`, `server.auth`,
`server.tls`, or any other operator-managed field.

ASCOM has no notion of `SetPark` being non-durable, so `CanSetPark =
false` is the right answer when the driver has nowhere to persist to —
returning `true` and silently losing the park across restarts would
violate the capability contract.

### PulseGuide lifecycle

A guide pulse is a temporary rate shift on the targeted axis,
implemented from the existing `:K` / `:G` / `:I` / `:J` tracking
primitives — no `:P` (that's the ST4 hardware-jack rate setter, not a
host-driven pulse command). This matches what `indi-eqmod`'s
`GuideNorth` / `GuideSouth` / `GuideEast` / `GuideWest` do.

```
PulseGuide(direction, duration)
   │
   ├─ validate: !AtPark, !disconnected, !slew_in_progress,
   │            !pulse_guiding_<targeted_axis>; duration = 0 → return Ok
   ├─ resolve direction → (axis, ccw, rate_factor):
   │     East  → (RA,  ccw=false, 1 - guide_rate_ra_fraction)
   │     West  → (RA,  ccw=false, 1 + guide_rate_ra_fraction)
   │     North → (Dec, ccw=false, guide_rate_dec_fraction)
   │     South → (Dec, ccw=true,  guide_rate_dec_fraction)
   ├─ compute shifted period:
   │     period = round(sidereal_step_period(tmr_freq, cpr_ra) / rate_factor)
   ├─ capture tracking_was_on = state.tracking_requested (RA pulses only)
   ├─ set pulse_guiding_<axis> = true                ; synchronous, pre-spawn
   │
   ├─ on the wire (in PulseGuide call thread):
   │     :K<axis>           stop and wait for running flag clear
   │     :G<axis> TRACKING + ccw
   │     :I<axis> period
   │     :J<axis>
   │
   ├─ spawn watcher task:
   │     tokio::sleep(duration)
   │     if !pulse_guiding_<axis>      → bail (external cancellation)
   │     if !transport.is_available()  → clear flag, bail
   │     if state.at_park || slew_in_progress → clear flag, bail
   │     ── otherwise restore prior state:
   │     RA  pulse: :K1 + stop-and-wait, then
   │                if tracking_was_on:
   │                    :G1 TRACKING (ccw=false)
   │                    :I1 sidereal_period
   │                    :J1
   │     Dec pulse: :K2 + stop-and-wait (Dec is normally idle; no restore)
   │     clear pulse_guiding_<axis>
   │
   └─ return immediately to the Alpaca caller
```

`IsPulseGuiding` returns `pulse_guiding_ra || pulse_guiding_dec` —
perpendicular pulses (one RA + one Dec) run concurrently; a same-axis
re-pulse while one is in flight is rejected with `INVALID_OPERATION`.

**Cross-cutting cancellation rule.** Any operation that mutates a given
axis — `set_tracking`, `slew_to_coordinates_async`, `park`,
`abort_slew`, `sync_to_coordinates`, `set_connected(false)` — clears
the corresponding `pulse_guiding_<axis>` flag *before* issuing its own
wire commands. The watcher's post-sleep restore step checks the flag
and bails out if cleared, so the new operation owns the axis without
racing the watcher. Without this, `set_tracking(false)` during an East
pulse would be silently undone when the watcher re-issued sidereal
tracking on restore.

**Dec sign convention.** `+Dec` always maps to `ccw=false`, regardless
of side-of-pier. The driver does not invert Dec direction after a
meridian flip — the existing slew/sync pipeline doesn't either; it
assumes a stable encoder-to-celestial-Dec mapping and requires the
user to `SyncToCoordinates` after a manual flip to recalibrate. A
PulseGuide call after a flip with no re-sync will guide in the wrong
celestial direction; this is consistent with the rest of the driver
and is the autoguider's responsibility to detect (via guide
calibration).

**Step-period unit reuse.** `sidereal_step_period(tmr_freq, cpr_ra)`
is also used for the Dec rate computation. The protocol's step-period
units are timer-counter ticks per motor step on both axes, and
`cpr_ra` ≈ `cpr_dec` on the GTi, so reusing the helper avoids a near-
duplicate `sidereal_step_period_dec`. INDI takes the same shortcut.

**Guide-rate fraction validation.** Internal storage is two fractions
in `(0.0, 1.0)` open interval (RA and Dec independently). The
exclusive upper bound matters: `fraction = 1.0` makes East's
`rate_factor = 0`, which divides by zero in the period formula. INDI
clamps at 0.9 for the same reason. ASCOM clients see deg/sec through
`GuideRateRightAscension` / `GuideRateDeclination`; the setter
validates `(0, SIDEREAL_DEG_PER_SEC)` and converts to fraction.

### Side-of-pier

`SideOfPier` is derived from the **Dec-axis encoder position**, the
Dec-axis CPR, and the site latitude — the canonical GEM convention used
by INDI eqmod (`eqmodbase.cpp::EncodersToRADec`):

- In **northern hemisphere** (`SiteLatitude ≥ 0`): a Dec encoder
  magnitude within ±90° (= `±cpr_dec/4` ticks) of home means the mount
  reached the target without a meridian flip — counterweight east, OTA
  west of pier → `PierSide::West`. A Dec encoder magnitude past 90° in
  either direction means the Dec axis has rotated through one of the
  celestial poles — counterweight west, OTA east of pier →
  `PierSide::East`. The boundary at exactly ±90° is included in `West`
  (the mount can sit at either celestial pole via normal pointing).
- In **southern hemisphere** (`SiteLatitude < 0`): the convention
  inverts.

Earlier revisions split on the RA mechanical hour angle at `HA = 0`.
The Dec-encoder split returns the same value as the HA split for any
pointing state reachable inside the safety envelope — which is why
ConformU's `SideofPier` test passed against both — but the two diverge
when the mount is manually positioned past the pole (e.g. a power-cycle
with the OTA pointing through-the-pole and the encoder reset placing
the initial position on the wrong side). The Dec-encoder convention
reports the right answer for that state; the HA-split convention
inherits the RA encoder's sign and misreports it. INDI's convention is
the canonical one.

`DestinationSideOfPier(targetRA, targetDec)` predicts the pointing
state without issuing wire traffic. It runs the same flip-policy
decision tree `SlewToCoordinatesAsync` uses to pick the target side
(see [§Meridian flip](#meridian-flip)), maps the target to encoder
ticks for the chosen side, and validates against the corresponding
per-side safety envelope with the same `INVALID_VALUE` rejection a
slew would issue. With `flip_policy.enabled = false` the decision
collapses to "current side", so for any target inside the (pre-flip)
safety envelope `DestinationSideOfPier` returns `West` in the
Northern Hemisphere (`East` in the Southern) — the driver does not
plan flips. With `flip_policy.enabled = true` the prediction can
return the opposite side when the target requires a flip to reach.

### Meridian flip

A meridian flip transitions the German Equatorial Mount from "normal
pointing" (`PierSide::West`: counterweight east, OTA west of pier) to
"flipped pointing" (`PierSide::East`: counterweight west, OTA east of
pier, Dec axis rotated past the celestial pole) while keeping the OTA
on the same celestial target. On a flip-capable mount the move is a
single slew with the RA encoder routed through a half of the encoder
range that keeps the counterweight clear of the pier.

The GTi's mechanical envelope — hardware-confirmed 2026-05-13 — is
`mech_HA ∈ [−6.99 h, +6.99 h]` for the pre-flip half. The post-flip
half is mechanically symmetric across the encoder wrap at `±12 h`: the
counterweight rises off the local horizon on the mirror side of the
pier. The routing detail and the geometric symmetry argument live in
[`docs/plans/star-adventurer-gti-meridian-flip.md`](../plans/star-adventurer-gti-meridian-flip.md)
§2.0; this section specifies the behavioural contract the driver
implements on top of it.

#### Flip policy

`MountConfig::flip_policy` controls whether and when the driver plans
a flip. Two fields in MVP (auto-flip-during-tracking knobs are
deferred to Phase 2.5 — see [§Deferred](#deferred-not-in-mvp)):

- **`enabled: bool`** (default `false`) — master switch. With
  `enabled = false`: `CanSetPierSide = false`, `SetSideOfPier` returns
  `NOT_IMPLEMENTED`, `DestinationSideOfPier` always returns the
  current pier side (driver never plans a flip), and
  `SlewToCoordinatesAsync` uses the pre-flip (`pierWest` in the
  Northern Hemisphere) coordinate pipeline only — behaviour identical
  to Phase 5. With `enabled = true`: the capability flag flips on
  and the slew planner picks the target pier side per the policy
  below.
- **`flip_range_hours: f64`** (default `0.5`) — half-width of the
  target-HA window around the meridian where the flipped state is
  mechanically reachable. Targets with `|target_HA| > flip_range_hours`
  are unflippable (the post-flip `mech_HA` would land outside the
  symmetric mirror band); the slew planner uses normal pointing only
  and `DestinationSideOfPier` returns the current side. Valid range
  `(0, 0.95]`. The upper bound matches the headroom past
  counterweight-horizontal on the pre-flip side (Phase 1.1 hardware
  verification); a larger value would push the post-flip `mech_HA`
  into the unverified mirror of the binding zone.

#### Per-pier-side safety envelopes

The pre-flip safe zone (`MountConfig::ra_min_hours` /
`ra_max_hours` / `dec_min_degrees` / `dec_max_degrees`, defaults
`[−6.95, +6.95]` h RA and `[−90, +90]°` Dec) covers `pierWest`
operation as in Phase 1.1.

The post-flip (`pierEast`) safe zone mirrors the pre-flip RA zone
through the encoder wrap at `±12 h`. For a target HA satisfying
`|target_HA| ≤ flip_range_hours`, the post-flip mech-HA is
`target_HA + 12 h` (folded signed into `[−12, +12)`), which lands in
the symmetric band `[+12 − flip_range_hours, +12] ∪ [−12, −12 +
flip_range_hours]`. The Dec encoder lands past either celestial pole
— `|dec_encoder| > 90°` (Northern Hemisphere; Southern inverts).

A flip-aware slew validates against the envelope for the *chosen*
side: a flip slew checks the post-flip RA band and the
past-the-pole Dec mapping; a normal slew checks the pre-flip zone.
Targets outside the relevant zone are rejected with `INVALID_VALUE`
before any wire motion.

#### Pier-side decision tree

`DestinationSideOfPier(ra, dec)` and `SlewToCoordinatesAsync(ra,
dec)` share the same selector:

1. If `flip_policy.enabled = false`, return the current `SideOfPier`.
2. Compute `target_HA = LST − ra` (signed, folded to `[−12, +12)`).
3. If the *current* side's safety envelope covers `target_HA`, stay
   on the current side (no unnecessary flip). The pre-flip side (the
   "normal" pointing — `pierWest` in the Northern Hemisphere,
   `pierEast` in the Southern) covers `target_HA ∈ [ra_min_hours,
   ra_max_hours]`; the post-flip side covers `target_HA ∈
   [−flip_range_hours, +flip_range_hours]`.
4. Otherwise return the *opposite* side.

The driver does not pre-emptively flip; it only flips when the target
can't be reached from the current side or when `SetSideOfPier` forces
it. The post-flip envelope's reach (`flip_range_hours`) is small —
`0.5 h` by default — so the practical pattern is: pre-flip side
covers most of the sky; an explicit or implicit flip rotates the
mount through the meridian to the post-flip side for tracking a
target past meridian crossing; the next slew that targets HA outside
the flip window auto-flips back to the pre-flip side.

When neither side's envelope covers the target (e.g. an
above-the-pole pointing past `ra_max_hours`), the selector returns
the *opposite* side; the subsequent envelope validation rejects the
slew with `INVALID_VALUE` regardless of which side was chosen.

#### `SetSideOfPier(side)`

`SetSideOfPier(side)` is the explicit flip trigger:

- `side == current_side`: no-op success.
- `side != current_side`: triggers a through-wrap flip slew that
  keeps the OTA on the current target while landing on the requested
  pier side.
- `PierSide::Unknown`: rejected with `INVALID_VALUE`.

`SetSideOfPier` returns `NOT_IMPLEMENTED` when `flip_policy.enabled
= false`, and follows the standard `NOT_CONNECTED` /
`INVALID_WHILE_PARKED` / `INVALID_OPERATION` (already-slewing) gate.
After a successful flip the next encoder snapshot's `SideOfPier` reads
the new value (the Dec encoder has moved past the pole);
`TargetRightAscension` / `TargetDeclination` remain unchanged.

#### Through-wrap slew routing

When the slew planner decides to flip (either via `SetSideOfPier` or
because the decision tree picked the opposite side), both axes are
routed through specific safe segments of their encoder ranges. The
RA axis stays in the counterweight-below-horizon half; the Dec axis
crosses the *visible* celestial pole rather than the below-horizon
one.

**RA axis:** routed through the negative-`mech_HA` half of the encoder
range (`mech_HA ∈ [−12, 0]`) — the half where the counterweight stays
at or below the local horizon. The mechanical binding zone at
`mech_HA ∈ (+6.95, +11.05)` is the same for every observer latitude,
so the rule is hemisphere-independent.

The safe direction depends on the *current* encoder position:

- `|current_ticks| ≤ cpr_ra/4` (current in the pre-flip envelope,
  `mech_HA ∈ [−6, +6]`): force CCW (negative delta). Path decreases
  through the negative half to the target. This is the forward-flip
  case (pre-flip → post-flip wrap).
- `|current_ticks| > cpr_ra/4` (current at or past the wrap,
  `mech_HA` near `±12`): force CW (positive delta). Path increases
  *away from the wrap* into the safe negative half. This is the
  flip-back case (post-flip → pre-flip).

A naive "always CCW for flip slews" rule works for forward flips but
drives the counterweight into the binding zone on flip-back: from
raw `−cpr/2` going CCW wraps the encoder past `+12` and crosses
`mech_HA = +6 to +9` (the binding peak). Hardware validation #3 hit
this exact failure mode on the back-to-Park-3 slew, slamming the CW
shaft into the pier and dropping the USB-CDC. The current-position-
aware rule (mirroring the Dec routing) selects the right direction
in both cases.

**Dec axis:** routed through the visible celestial pole, NOT the
below-horizon pole. For a polar-aligned mount, only one of the two
celestial poles is above the local horizon — NCP at altitude `+lat`
for Northern observers, SCP at altitude `+|lat|` for Southern. The
encoder positions are `+cpr_dec/4` (NCP) and `−cpr_dec/4` (SCP). The
Dec axis must traverse the visible pole during a flip slew; routing
through the below-horizon pole drives the OTA into the ground
(altitude `−|lat|` at the dip).

The rule, expressed by encoder side:

- **Northern** observer (safe pole `+cpr_dec/4`):
  - `current_dec_encoder` in the pre-flip half (`|enc| ≤ cpr_dec/4`):
    force the Dec slew CW (positive delta). The path crosses
    `+cpr_dec/4` from below.
  - `current_dec_encoder` in the post-flip half (`|enc| > cpr_dec/4`):
    force CCW (negative delta). The path descends back through
    `+cpr_dec/4`.
- **Southern** observer: inverted. Safe pole is `−cpr_dec/4` (SCP).

The canonical fold's boundary case at exactly `±cpr_dec/2` (e.g.
flipping from `dec_encoder = 0` to a celestial target of `dec = 0`
where the post-flip target is `+180° ≡ −180°`) lands on the negative
direction by default; for a Northern observer that routes through
SCP and is the failure mode the first real-hardware validation
exposed. The fix forces the long way around (`delta − cpr` or
`delta + cpr`) so the path always crosses the *safe* pole.

The slew lifecycle (see [§Slew lifecycle](#slew-lifecycle)) is
otherwise unchanged: the same `:K → :G → :I → :H → :M → :J`
wire sequence per axis, the same EQMOD pickup loop, the same settle
delay. The flip slew differs from a normal slew only in which
encoder target the planner computes and which CCW bit `:G` issues
per axis (forced to the safe-direction sign on flip slews, so the
routing goes "under" the polar axis on RA and through the visible
pole on Dec rather than taking the shortest-encoder path).

#### Hardware validation

`flip_policy.enabled` defaults to `false` until at least one
successful real-hardware flip on a GTi has been recorded. The
mechanical symmetry argument (plan §2.8) is strong, but the
through-wrap traversal is the first time the GTi's negative-`mech_HA`
half is exercised past the pre-flip safe envelope, and asymmetric
failure modes like cable wrap will surface there. The first real
`SetSideOfPier(East)` on hardware is the validation gate; until then
operators leave `flip_policy.enabled` at its default.

## Configuration

JSON, deserialised with `serde` + `humantime-serde` for `Duration`
fields. The transport block is a tagged enum: `usb` or `udp`.

```json
{
  "transport": {
    "kind": "usb",
    "port": "/dev/serial/by-id/usb-STMicroelectronics_STM32_Virtual_ComPort_4E8741795300-if00",
    "baud_rate": 115200,
    "command_timeout": "2s",
    "polling_interval": "200ms"
  },
  "server": {
    "port": 11117,
    "discovery_port": 32227,
    "tls": null,
    "auth": null
  },
  "mount": {
    "name": "Star Adventurer GTi",
    "unique_id": "skywatcher-sa-gti-001",
    "description": "Sky-Watcher Star Adventurer GTi German Equatorial Mount",
    "enabled": true,
    "site_latitude_deg": 37.7749,
    "site_longitude_deg": -122.4194,
    "site_elevation_m": 0.0,
    "settle_after_slew": "2s",
    "tracking_rate": "sidereal",
    "park_ra_ticks": null,
    "park_dec_ticks": null,
    "flip_policy": {
      "enabled": false,
      "flip_range_hours": 0.5
    },
    "home_pose": null
  }
}
```

The WiFi variant of `transport`:

```json
"transport": {
  "kind": "udp",
  "address": "192.168.4.1",
  "port": 11880,
  "bind_address": "192.168.4.2",
  "command_timeout": "2s",
  "polling_interval": "200ms"
}
```

Notes:

- `bind_address` is **mandatory** for UDP (must be a 192.168.4.0/24 host
  IP when the mount is in AP mode). Without it, the kernel may select a
  source IP that the mount cannot reach, and packets are silently
  dropped.
- `polling_interval` controls the rate at which the background loop reads
  `:f` (axis status) and `:j` (axis position). 200 ms is a reasonable
  default; `rp` polls `Slewing` no faster than 100 ms so this gives the
  driver headroom.
- `settle_after_slew` is applied *after* both axes report stopped, before
  `Slewing` clears. Mirrors `rp`'s `mount.settle_after_slew` config.
- `site_latitude_deg` is in WGS84 degrees, `+N`. `site_longitude_deg` is
  WGS84 degrees, `+E`. (ASCOM convention.)
- `tracking_rate` accepts `"sidereal"` only in MVP. Field is reserved
  for future expansion.
- `park_ra_ticks` / `park_dec_ticks` are written by `SetPark` and read
  on every connect; absent (or `null`) at first run, populated once
  `SetPark` is called. Operators may set them by hand to pin a known
  mechanical pose. See [§Park persistence](#park-persistence) for the
  rules around when the driver writes to this file. When absent, the
  park target falls back to the live snapshot reading. With
  `home_pose` set and a fresh power-up, that's the home_pose's
  logical encoder values (`Park` defaults to "return to the pose
  you powered up at"); otherwise it's the handshake-captured
  reading. See [§Park lifecycle](#park-lifecycle).
- `flip_policy.enabled` defaults `false`. Set to `true` only after
  the first real-hardware meridian flip has been verified on the
  specific mount (see [§Hardware validation](#hardware-validation)).
  While `false`, `CanSetPierSide` reports `false` and the driver
  ignores flip routing entirely.
- `flip_policy.flip_range_hours` defaults `0.5`. Half-width of the
  target-HA window around the meridian where the flipped state is
  reachable. Valid range `(0, 0.95]`; the upper bound is the verified
  safe headroom past counterweight-horizontal on the pre-flip side.
  See [§Meridian flip](#meridian-flip).
- `home_pose` defaults `null` — no encoder seeding on connect, the
  driver trusts whatever encoder value the firmware reports.
  Operators powering up the mount at a recognised Astro-Physics park
  position set it to the matching `ap_park_<n>` string (`"ap_park_1"`
  through `"ap_park_5"`) and the driver issues `:E1` / `:E2` on
  connect to seed the firmware encoder to the codebase's convention
  for that pose. See [§Home pose](#home-pose).

### Home pose

The Sky-Watcher firmware resets its encoder counter to `(0, 0)` on
every power-up. The codebase's coordinate math interprets that zero
against the configured `home_pose`, so the operator's physical
power-up pose lines up with the driver's celestial-coordinate
readings without an explicit `SyncToCoordinates` step.

The five named poses follow the Astro-Physics
["Park Positions Defined"](https://astro-physics.info/tech_support/mounts/park-positions-defined.pdf)
document. Each AP pose describes a fixed mechanical pier side
(OTA-on-west-of-mount or OTA-on-east-of-mount) that's the same for
both hemispheres — but the driver's natural-vs-flipped pier convention
flips between hemispheres (N natural = pierWest, S natural = pierEast,
see `pre_flip_side` in `mount_device.rs`). So a pose like Park 1
(OTA on the west side of the mount) is the **natural pier** in the
North and the **flipped pier** in the South, and its encoder
representation differs accordingly:

- Natural side: `mech_HA = celestial_HA`, `dec_enc = celestial_dec`.
- Flipped side: `mech_HA = celestial_HA + 12 h` folded to `[−12, +12)`,
  `dec_enc = sign(celestial_dec) · (180° − |celestial_dec|)` (past
  the celestial pole on the encoder).

| `home_pose` | AP description | Mech. pier | N hem (mech_HA, dec_enc) | S hem (mech_HA, dec_enc) |
|---|---|---|---|---|
| `null` (default) | No seeding — trust the firmware encoder as-is. Pre-Phase-6 behaviour. | — | — | — |
| `ap_park_1` | OTA on west, level, facing polar-side horizon. Celestial Dec = `±(90 − \|lat\|)`. | West | `(−12 h, +(90 − \|lat\|)°)` (natural) | `(0 h, −(90 + \|lat\|)°)` (flipped past pole) |
| `ap_park_2` | OTA level facing east horizon, counterweight straight down. Hemisphere-independent celestial coords `(HA=−6, Dec=0)`. | — (CW down) | `(−6 h, 0°)` | `(−6 h, 0°)` |
| `ap_park_3` | OTA along polar axis at the visible celestial pole. Sky-Watcher's stock power-up pose. | — (CW along polar) | `(−6 h, +90°)` | `(−6 h, −90°)` |
| `ap_park_4` | OTA on east, level, facing anti-polar horizon. Celestial Dec = `∓(90 − \|lat\|)` (sign anti-hemisphere). | East | `(−12 h, −(90 + \|lat\|)°)` (flipped past pole) | `(0 h, +(90 − \|lat\|)°)` (natural) |
| `ap_park_5` | OTA on east, level, facing polar-side horizon. Celestial Dec = `±(90 − \|lat\|)` (sign matches hemisphere). APCC-only in AP's own software. | East | `(0 h, +(90 + \|lat\|)°)` (flipped past pole) | `(−12 h, −(90 − \|lat\|)°)` (natural) |

The seed step is skipped when the firmware reports an encoder
reading beyond a small fresh-power-up tolerance
(`FRESH_POWER_UP_TICK_TOLERANCE`, currently 100 ticks ≈ 10″ at the
GTi's CPR). The tolerance exists because the Sky-Watcher firmware
does not always read exactly `(0, 0)` after a power-cycle —
empirically the validation GTi reports `dec = −1` on connect, a
single-tick initialisation artifact (~0.4″) that obviously still
represents the just-powered-up state. Any genuine post-slew
encoder is tens of thousands of ticks away from zero, so the
tolerance comfortably distinguishes "fresh power-up" from
"already slewed this session". Reconnecting mid-session after a
slew is therefore still safe.

Documented operator assumption: when `home_pose` is set, the operator
powers up the mount **at** the configured pose and connects the
driver before any slew or sync.

### CLI arguments

| Argument | Description |
|---|---|
| `-c, --config <PATH>` | Path to JSON config file |
| `--transport <usb\|udp>` | Override `transport.kind` |
| `--port <DEVICE_OR_HOST>` | Override `transport.port` (USB) or `transport.address` (UDP) |
| `--baud <RATE>` | Override `transport.baud_rate` (USB only) |
| `--server-port <PORT>` | Override `server.port` |
| `-l, --log-level <LEVEL>` | `trace` / `debug` / `info` / `warn` / `error` |

## Module Structure

### `crates/skywatcher-motor-protocol`

```
src/
  lib.rs       — re-exports, crate-level docs, the protocol overview
  error.rs     — ProtocolError (FrameError, HexError, MountError(code))
  command.rs   — Command enum, Command::encode_into(&mut Vec<u8>)
  response.rs  — Response enum, Response::decode(&[u8])
  codec.rs     — encode_u24 / decode_u24 (LE nibble-pair),
                 encode_position / decode_position (with 0x800000 bias),
                 frame helpers
```

Pure functions; no I/O, no async, no `tokio`, no `ascom-alpaca`. Unit
tests live alongside each module as `#[cfg(test)] mod tests` blocks;
property tests for round-trip invariants live in
`tests/property_tests.rs`.

### `services/star-adventurer-gti`

```
src/
  config.rs              — Config types (TransportConfig, ServerConfig,
                           MountConfig); humantime-serde for durations
  error.rs               — StarAdvError + ASCOM-error mapping
  transport/
    mod.rs               — Transport trait (async send_frame + recv_frame)
    serial.rs            — tokio-serial impl
    udp.rs               — tokio UdpSocket impl (with bind-IP enforcement)
    mock.rs              — feature("mock") in-memory state-machine impl
  transport_manager.rs   — ref-counted shared transport, background polling,
                           parameter cache (CPR, TMR_Freq, hsr per axis)
  coordinates.rs         — encoder-tick ↔ angle, LST, sync offset,
                           side-of-pier derivation
  mount_device.rs        — ASCOM Device + Telescope trait impl
  lib.rs                 — ServerBuilder, module declarations
  main.rs                — CLI entry point
tests/
  bdd.rs                 — cucumber harness (harness = false)
  bdd/
    world.rs             — World struct + helpers
    steps/
      mod.rs
      connection_steps.rs
      slew_steps.rs
      tracking_steps.rs
      park_steps.rs
      sync_steps.rs
      side_of_pier_steps.rs
  features/
    connection_lifecycle.feature
    device_metadata.feature
    coordinate_reads.feature
    slew.feature
    sync.feature
    tracking.feature
    park.feature
    abort.feature
    side_of_pier.feature
  test_lib.rs            — server-startup + CLI tests (gated on `mock`)
  conformu_integration.rs — ASCOM compliance (gated on `conformu`)
```

## Testing

The strategy follows
[`docs/skills/testing.md`](../skills/testing.md): BDD scenarios are the
canonical contract; unit tests cover protocol parsing and coordinate math;
ConformU verifies ASCOM compliance.

| Layer | Coverage |
|---|---|
| Crate unit tests (`#[cfg(test)]`) | command/response encode-decode, codec edge cases (0x000000, 0xFFFFFF, signed boundaries), error parsing, frame framing rules |
| Crate property tests (`tests/property_tests.rs`) | round-trip: random `Command` → bytes → `Command`; same for `Response`; bias-offset preservation across signed `i32` range |
| Service unit tests (`#[cfg(test)]` per module) | `coordinates`: encoder ↔ RA/Dec across edge cases (poles, meridian, hemisphere flip); `config`: defaults, JSON round-trips, CLI overrides; `error`: ASCOM mapping |
| Service BDD (cucumber) | every behaviour table-row above as a scenario, with the mock transport |
| Service `test_lib.rs` (gated on `mock`) | server starts, binds the configured port, exposes the configured device |
| `conformu_integration.rs` (gated on `conformu`) | ASCOM Telescope compliance via `ConformUTestBuilder::run()` — runs both `alpacaprotocol` and `conformance` phases. Wired into the nightly `conformu` workflow via `[package.metadata.conformu]` in `Cargo.toml`. See [§"Expected ConformU report"](#expected-conformu-report) for the known deferred-by-design / upstream issues. |

The mock transport is a feature-gated in-memory state machine that
simulates the motor controller — it accepts the same `:cmd<axis>...\r`
frames and emits well-formed `=...\r` / `!XX\r` responses, with internal
state for axis position, motion mode, running/stopped, and tracking. In
**tracking mode** the mock advances the axis encoder forward by a small
sidereal-equivalent chunk per `:j` poll (ignoring `goto_target_ticks`),
so post-slew `RA` reads stay constant — matching what real Sky-Watcher
firmware does once tracking is re-enabled. In **goto mode** the mock
walks toward `goto_target_ticks` and clears `running` on arrival. BDD
tests use the mock by default; ConformU and `test_lib.rs` use the
feature-gated mock so the binary itself runs against a fake mount.

### Expected ConformU report

A clean nightly ConformU run against this driver reports:

- **0 errors** — anything here is a real driver regression.
- **7 issues**, all of which are cosmetic / inherent to a
  non-flipping GEM:
  - `SOPPierTest` ×4 — the safety envelope correctly rejects
    cross-meridian slews that would land at `RA mech-HA = ±9h`
    (well outside the default ±6.95h envelope, which sits
    3 arcmin inside the GTi's mechanical limit of ±6.99h and
    INDI eqmod's baked-in ±7h envelope, `zeroRAEncoder ±
    (totalRAEncoder/4 + totalRAEncoder/24)`). `SOPPierTest`
    exercises the pier-flip code paths by commanding such
    slews. The driver returns
    `InvalidValueException` from both `SlewToCoordinatesAsync`
    and `DestinationSideOfPier` (the two go through the same
    `check_within_safe_envelope` gate) for each of the two
    ±9 h test points, so ConformU records four exception
    entries per run. This is the same envelope-rejection
    mechanism Phase 4 added — see
    [§Phase 4 driver-logic changes].
  - `SideofPier` ×1 and `DestinationSideofPier` ×1 —
    "`pierWest` is returned when the mount is observing at an
    hour angle between 0.0 and +6.0". ConformU's
    `SideofPier` test assumes the mount flips at the meridian
    (the EQMOD / Sky-Watcher Synscan / ASCOM-driver-pattern
    behaviour) and therefore expects `pierEast` for any target
    west of the meridian. The Star Adventurer GTi driver does
    not initiate flips — the safety envelope keeps the
    encoder within `[-CPR/4, +CPR/4]` of the home position,
    which the Dec-encoder side-of-pier convention correctly
    classifies as `pierWest` regardless of whether the target
    is east or west of the meridian. The ASCOM spec is
    explicit that `SideOfPier` reports the OTA's mechanical
    position, not the target's sky position, so the driver's
    answer is correct for this mount; ConformU's
    flip-assumption check just doesn't apply.
  - `DestinationSideOfPier` ×1 — "Same value for
    DestinationSideOfPier received on both sides of the
    meridian". Same root cause: the driver never plans a
    flip, so every in-envelope target lands in the same
    pointing state.

Issue counts shift on convention changes:

- Pre-#202 baseline (HA-meridian split, `DestinationSideOfPier`
  unimplemented): **9 issues** — `DestinationSideOfPier` ×1
  (NotImplemented), `SOPPierTest` ×4 (inherited from
  NotImplemented), `SOPPierTest` ×2 (safety envelope),
  `TrackingRate Write` ×2 (upstream `ascom-alpaca-rs`
  framework bug — Alpaca enum rejection at the axum/serde
  layer before reaching the driver).
- Post-#202 baseline (Dec-encoder split, `DestinationSideOfPier`
  implemented): **7 issues** as listed above. The
  `TrackingRate Write` ×2 entries disappeared after an
  unrelated upstream fix; the four `DestinationSideOfPier`
  "consistency" / "Exception" entries got reclassified by the
  Dec-encoder switch (the four inherited-from-NotImplemented
  ones became three of the new-cause ones plus the two
  `SideofPier` / `DestinationSideofPier` consistency
  entries).

Any issue or error outside that list — and in particular any
`SlewTo*` / `SyncTo*` row reporting a tolerance exceedance
("`Actual RA: ..., Target RA: ...`" with > 10″ delta) — is a
real regression. Fix the driver, then refresh this section.

## Connection Lifecycle

```
Connected = true
   ↓
open transport (serial: tokio-serial open + raw mode;
                UDP: bind to config.bind_address, set timeout)
   ↓
init handshake:
  :F1, :F2          (initialize axes)
  :a1, :a2          (CPR per axis)         → cache
  :b1               (TMR_Freq)             → cache
  :g1, :g2          (high-speed ratio)     → cache
  :e1               (motor board version)  → debug! log
  :j1, :j2          (initial positions)    → cache
   ↓
load park target from config / handshake → in-memory park ticks
   ↓
if home_pose is set AND firmware encoder is (0, 0):
  :E1, :E2  (seed encoder to the AP pose's codebase convention)
   ↓
start background polling task (interval = config.polling_interval)
   ↓
Connected reports true once handshake completes
```

```
Connected = false
   ↓
abort any in-progress motion (:L1, :L2 — instant stop)
   ↓
stop tracking if running (:K1)
   ↓
cancel polling task
   ↓
close transport
   ↓
clear parameter cache
```

The transport is reference-counted: if multiple Alpaca clients each call
`Connected = true`, the underlying transport is opened once and shared.
Disconnect tears down only when the last reference drops. (Same pattern
as `qhy-focuser` and `ppba-driver`.)

## MVP Scope

### Phase status

| Phase | Status |
|---|---|
| **Phase 1 — Design doc** | ✓ landed (this document, PR #178) |
| **Phase 2 — BDD scaffold** | ✓ landed: codec crate skeleton, service skeleton, all feature files (`@wip`), step stubs (PR #180) |
| **Phase 3 — Implementation** | ✓ landed: codec, transports (USB+UDP), `MountDevice`, ConformU integration (PR #188); BDD step bodies + `@wip` removal (PR #189). All 9 feature files / 77 scenarios green on Linux/Windows/macOS CI. |
| **Phase 4 — Real-hardware bringup** | partially landed — first hardware connect surfaced several protocol-decoding gaps that the mock had hidden. Details below. |
| **Phase A5 — `:I`/`:M` on slew + EQMOD pickup** | landed (issue #205) — reinstates `:I` on the slew path, switches goto to `:H` (delta target) + `:M` (break-point), and adds an iterative post-stop pickup loop to close the residual RA drift the Phase 4 ConformU run flagged. |
| **Phase A6 — Dec-encoder `SideOfPier` + `DestinationSideOfPier`** | landed (issue #202) — switches `SideOfPier` from the RA mech-HA split at `HA = 0` to the canonical INDI eqmod Dec-encoder convention (`East` when `\|dec_encoder\| > cpr_dec/4`), and lands `DestinationSideOfPier` reusing the same coordinate-math pipeline as `SlewToCoordinatesAsync`. ConformU expected-issues count moves from 9 to 7: the `DestinationSideOfPier` NotImplemented entry and the four inherited `SOPPierTest` entries clear, the two upstream `TrackingRate Write` entries disappear (unrelated framework fix), and three new "non-flipping mount" entries appear that reflect ConformU's flip-aware-GEM assumption rather than driver bugs. See [§Expected ConformU report](#expected-conformu-report). |
| **Phase A7 — PulseGuide** | landed (issue #206) — implements `PulseGuide` as a rate-shifted tracking burst on the targeted axis (no `:P`; that's the ST4-jack rate setter, not a pulse trigger), flips `CanPulseGuide` and `CanSetGuideRates` to `true`, and re-enables `[package.metadata.conformu]` so the full two-phase ConformU integration runs again. |
| **Phase 5 — user-defined `SetPark` + persistence** | landed (issue #203) — park target now sourced from `mount.park_ra_ticks` / `mount.park_dec_ticks` in the config (fallback: encoder positions captured at handshake), `SetPark` writes the current encoder pair back into the running config file via atomic rename, `CanSetPark` flips on when `--config` is provided. See [§Park lifecycle](#park-lifecycle) and [§Park persistence](#park-persistence). |
| **Phase 6 — meridian-flip support** | implementation in progress — adds `MountConfig::flip_policy` (`enabled` + `flip_range_hours`), per-pier-side safe envelopes, through-wrap slew routing for flip slews, `SetSideOfPier`, and flip-aware `DestinationSideOfPier`. `flip_policy.enabled` defaults `false` and awaits a successful first real-hardware flip on a GTi before the default is reconsidered. Auto-flip-during-tracking is intentionally deferred to a Phase 2.5 follow-up — the driver only flips on an explicit `SetSideOfPier` or a slew whose target requires the opposite side. Plan: [`docs/plans/star-adventurer-gti-meridian-flip.md`](../plans/star-adventurer-gti-meridian-flip.md). See [§Meridian flip](#meridian-flip). |

#### Phase 4 findings (hardware bringup)

First connecting the driver to a physical Star Adventurer GTi
revealed four wire-protocol issues the mock had not been
exercising. All are now patched, with regression tests in the
protocol crate and the BDD suite.

1. **`:g<axis>` payload width.** The spec is ambiguous about
   payload width for `Inquire High-Speed Ratio`. Real GTi returns
   a **2-hex-byte** payload (`=01\r`) on both axes — not the
   6-hex-byte u24 that the codec originally assumed. The codec
   now accepts both widths; the value (`0x01`) is stored as a
   plain `u32` in the parameter cache. Note that the documented
   "16, 32, 64" expected high-speed-ratio values do **not** match
   what this firmware returns. The driver no longer relies on
   the high-speed-ratio for slew-rate computation — see point 3.

2. **`!XX\r` error frame width.** Spec §4 documents a 2-hex-digit
   error code; empirical GTi returns a **1-hex-digit** form
   (`!4\r`) for the single-digit codes defined in §5. The codec
   accepts both 3- and 4-byte error frames.

3. **`:G` mode-byte semantics — most damaging bug.** The `:G`
   payload is **two independent hex nibbles** (DB1 then DB2 per
   spec §5), each with its own bit assignments. The original
   codec treated the byte as a flat 8-bit bitfield with
   `goto = 0x10, fast = 0x20, reverse = 0x01`. By coincidence the
   wire bytes it produced for `GOTO_FAST_FORWARD` (`"30"`) decode
   under the spec as **Tracking-Fast-CW**, which never auto-stops
   at the `:S` target. Every slew the driver issued was
   effectively a continuous-step command. The codec was rewritten
   to encode each nibble correctly:
   - `MotionMode::TRACKING` → wire `"10"` (DB1=1 Tracking, DB2=0 CW)
   - `MotionMode::GOTO_FAST_FORWARD` → wire `"00"` (DB1=0 Goto+Fast, DB2=0 CW)
   - Reverse direction flips DB2 bit 0 (e.g. `"01"` / `"11"`).

4. **`:f` status nibble-0 bit-1** decoded as "Forward" in the
   original codec. Per spec it is **CCW**. Renamed
   `AxisStatus.forward` → `AxisStatus.ccw`; `AxisStatus.blocked`
   and `AxisStatus.level_switch_on` (spec nibble-1 bit-1 and
   nibble-2 bit-1, respectively) added at the same time. The
   slew watcher now aborts on `blocked`.

#### Phase 4 driver-logic changes that real hardware required

In addition to the codec fixes:

- **`stop_and_wait`** — `:K` (decelerate) only *requests* a stop;
  the motor takes meaningful wallclock time to actually halt.
  `:G` against a still-decelerating axis returns `!2\r`
  (`MotorNotStopped`). The slew, park, and `set_tracking(true)`
  paths issue `:K` (decelerate) and then poll `:f` until
  `running == false`, before any subsequent `:G`/`:S`/`:J`. An
  early version of this path used `:L` (instant stop) instead;
  switched to `:K` in issue #207 to match the spec's recommended
  stop semantics and INDI eqmod's `StopWaitMotor`
  (`indi-eqmod/skywatcher.cpp:1741-1765`) — `:L` is harsher on
  the gearbox and is reserved for genuine emergency stops
  (`AbortSlew`, slew/park watcher abort on `blocked`).
  Mock hid the wallclock issue originally because it processes
  `:K`/`:L` instantaneously.
- **No mode-cache short-circuit (attempted and reverted)** — issue
  #207's initial plan included an INDI-style
  `LastRunningStatus == NewStatus` cache to skip `stop_and_wait +
  :G` when the requested mode matched the last one we acked. **The
  implementation we tried did not work on real hardware.**
  Mock-mode ConformU + the unit/BDD suites all passed cleanly, but
  the first ConformU run against the physical GTi triggered an
  unbounded Dec slew: the cache said `Goto-Fast-CCW` after a `:E`
  (sync), but the firmware-side mode had drifted; the resulting
  `:I/:H/:M/:J` started a slew that ran ~360° of unwanted Dec
  motion before the pickup loop fired a ~269° corrective slew back.
  See PR #210 for the wire trace. The cache was reverted to an
  unconditional `stop_and_wait + :G` on every slew/park/tracking
  prep — the spec-recommended sequence and what INDI's
  `StopWaitMotor` does internally.

  Two reasons not to retry naively if/when revisiting:

  1. **`:E` (sync) is one observed mode-state desync, but not
     necessarily the only one.** Tracking transitions, post-goto
     auto-engage on RA, `:L` aborts, blocked-axis recovery, and any
     other state-changing command can shift firmware-side mode
     without an explicit `:G` from us. A cache that mirrors only
     what we *issued* is structurally unsafe.
  2. **UDP transport widens the failure mode.** The wire protocol
     is identical on USB-CDC and UDP/11880, but UDP can drop,
     reorder, or duplicate packets without us noticing; a cache
     that records "we sent `:G CCW` and saw an ack" can lie even
     more easily if a `:G` response was actually a stale duplicate
     ack from a prior frame. Anything we cache about firmware state
     must survive both half of the codec layer being lossy.

  Any future cache implementation needs a **background validation
  loop** that re-reads the actual `:f` mode bits (per spec §5, the
  status nibble reports the live mode) often enough to catch
  desyncs *before* the next `:G`-eliding op fires — polling cadence
  needs to be comparable to or faster than the smallest slew a
  caller could chain, and a desync detection must invalidate the
  per-axis cache immediately and refuse short-circuits until the
  next confirmed `:G` ack. Without that — or without a different
  approach that doesn't depend on a snapshot of state we don't own —
  the cache is structurally unsound.
- **INDI-style slew sequence** — Phase A5 reinstated `:I` on the
  slew path and switched the goto from `:S` (absolute target) to
  `:H` (delta target) plus `:M` (break-point increment), matching
  what `indi-eqmod`'s `SlewTo` emits. The spec §3 phrasing that
  "the firmware picks goto speed internally" turned out to be
  half the story: the firmware does, but only when `:I` has
  primed `minperiods[axis]` (INDI defaults to `6`). Without it
  the goto runs at a slower step period and the deceleration
  ramp set by `:M breaks = min(|delta|/10, 3200)` is missing,
  which is what produced the post-stop residual RA drift the
  Phase 4 ConformU run flagged. Park still uses `:S` since its
  target is encoder 0 and the absolute form is the simpler fit.
- **Pickup-loop accuracy stack** — issue #207 follow-up; three
  layered fixes that together brought real-hardware ConformU
  residuals from mean 8.4″ / max 11.0″ (2 × 10″ tolerance
  crossings) down to mean 2.4″ / max 6.6″ on USB and mean 5.0″ /
  max 8.7″ on UDP, with **zero crossings on either transport**.

  1. **LST pre-compensation** —
     [`coordinates::pickup_target_ra_ticks`] computes each
     iteration's corrective encoder target for `LST(now +
     projection)` rather than `LST(now)`. Without it the slew
     lands at where the target *was* one iteration ago, and the
     residual floor matches the per-iteration LST drift (~6″ on
     USB, ~14″ on UDP).

  2. **Adaptive projection** — the watcher tracks the wall-clock
     interval between consecutive pickup decisions and uses it as
     the projection for the next iteration. Self-tunes per
     transport without per-transport config: USB iterations
     stabilise at ~400 ms, UDP at ~1100 ms. First iteration (no
     prior data) falls back to `polling_interval × 2`.

  3. **Pause background polling during the slew** — the watcher
     acquires a [`TransportManager::pause_background_polling`]
     RAII guard at the top of its spawn. While paused, the
     watcher owns the wire: pickup wire commands (`:K :G :I :H
     :M :J`) fire without contending with `:j` / `:f` polls for
     the `command_lock`, and the watcher's
     [`TransportManager::poll_axes_now`] drives the snapshot's
     freshness with one wire round-trip per loop iteration. The
     guard is dropped explicitly right after tracking restart,
     before the settle delay — so background polling resumes
     during settle and the snapshot reflects the actively-tracking
     encoder position by the time an Alpaca client reads
     `RightAscension` post-`Slewing`. Releasing the guard early
     vs. on watcher exit makes a measurable difference (UDP mean
     7.3″ → 5.0″ in the experiment runs). Park watcher follows
     the same pattern.

  See `docs/plans/star-adventurer-gti-pickup-accuracy.md` for the
  experiment plan and the diagnostic data that drove these
  choices.
- **Mechanical safety envelope** — driving the mount into the
  counterweight-up region with ConformU's pier-flip tests stalled
  the motor against a hard stop while the encoder counter kept
  advancing (audible motor noise, OTA stationary). `MountConfig`
  now carries `ra_min_hours` / `ra_max_hours` /
  `dec_min_degrees` / `dec_max_degrees`. `SyncToCoordinates`,
  `SlewToCoordinatesAsync`, and `Park` reject targets outside
  the envelope with `INVALID_VALUE` before any wire motion.
  Defaults: `±6.95 h` RA — `0.05 h` (`3 arcmin`) inside the
  GTi's hardware-verified `±6.99 h` mechanical limit (per the
  2026-05-13 hardware test) and INDI eqmod's baked-in `±7 h`
  envelope for every Sky-Watcher mount (`zeroRAEncoder ±
  (totalRAEncoder/4 + totalRAEncoder/24)` in
  `eqmodbase.cpp::Goto`). The buffer is deliberate: the ASCOM
  `SlewToCoordinates(ra, dec)` round-trip means the driver
  re-reads LST a few tens of ms after the client computed the
  target, so a target quantised exactly to the mechanical limit
  would drift past it; and the deferred Phase 2 meridian-flip
  planner will need headroom between the configured envelope
  and the mechanical stops to plan multi-stage flip slews —
  `±90°` Dec.
- **Slew watcher abort on `:f` blocked** — both the slew and
  park completion watchers issue `:L` on both axes and clear
  `slew_in_progress` if either axis reports `blocked=true`.
- **Transient-error tolerance with best-effort halt** — the
  slew/park watchers tolerate up to three consecutive
  `poll_axes_now` failures (with a 100 ms backoff between
  attempts) before giving up, so a single USB-CDC glitch
  doesn't take the watcher offline for the rest of a goto.
  On retry exhaustion the helper fires `:L` on both axes
  best-effort so the motor isn't left commutating with no
  observer, then clears `slew_in_progress` and exits.
  Every successful poll is also emitted at `debug` with the
  full per-axis snapshot (position, running, blocked, goto)
  so post-mortems can reconstruct the last-known-good state
  observed before any failure. See
  `mount_device::watcher_poll_with_retry`.
- **EQMOD-style iterative pickup** — Phase A5 added a pickup
  loop in the slew watcher: after both axes report stopped,
  the watcher reads current RA/Dec, compares against the
  latched target, and if either residual exceeds 5″ (matching
  INDI's `RAGOTORESOLUTION`/`DEGOTORESOLUTION`) recomputes the
  delta for the current LST and re-enters the wire sequence.
  Capped at 5 iterations (`GOTO_ITERATIVE_LIMIT`). On the GTi
  this converges in 1–2 iterations because the post-stop
  residual is bounded by `slew_duration × sidereal_rate` and
  the second pass starts from a near-zero delta.

What's still outstanding from Phase 4:

- **Empirical slew rate vs `:g`** — the formal high-speed-ratio
  formula in spec §3 gives `1` for `hsr = 1` (sidereal rate),
  which is unusable. The firmware appears to pick its own
  Goto-mode rate (~5°/s observed). Documented; revisit if a
  tunable goto rate becomes a requirement.


### In-scope

- USB transport at 115200 baud (`/dev/serial/by-id/...` path in config)
- UDP transport at 192.168.4.1:11880 (bind to local 192.168.4.x)
- Connect / disconnect lifecycle, ref-counted transport
- Init handshake: `:F`, `:a`, `:b`, `:g`, `:e`, `:j`
- ASCOM device metadata (`Description`, `DriverInfo`, `DriverVersion`,
  `InterfaceVersion`, `Name`, `SupportedActions = []`)
- Site lat/lon/elevation from config (read-only via ASCOM)
- `RightAscension` / `Declination` reads (encoder + LST + sync offset)
- `Azimuth` / `Altitude` reads (derived from RA/Dec)
- `SyncToCoordinates` / `SyncToTarget`
- `SlewToCoordinatesAsync` / `SlewToTargetAsync`
- `AbortSlew`
- `Tracking` setter / getter (sidereal only)
- `Park` / `Unpark` (software park; target encoder pair sourced from
  config or captured at handshake — see [§Park lifecycle](#park-lifecycle))
- `SetPark` (writes the current encoder pair back into the running
  config file via atomic rename; `CanSetPark` reflects whether the
  driver has a config path to write to — see [§Park persistence](#park-persistence))
- `AtPark` / `AtHome` reads
- `SideOfPier` read (Dec-encoder convention)
- `DestinationSideOfPier(ra, dec)` prediction (flip-policy-aware when
  `flip_policy.enabled` — see [§Meridian flip](#meridian-flip))
- `SetSideOfPier(side)` — explicit meridian-flip trigger, gated on
  `flip_policy.enabled` (Phase 6, in progress; default `false` pending
  first-hardware verification — see [§Meridian flip](#meridian-flip))
- Per-pier-side safety envelopes and through-wrap slew routing for
  flip slews (Phase 6, in progress)
- `Slewing` poll
- `UTCDate` / `SiderealTime` (host-clock-derived)
- Mock transport for BDD tests; feature-gated mock for `test_lib.rs` and
  ConformU
- ASCOM Alpaca discovery on the standard port

### Deferred (not in MVP)

| Capability | Why deferred |
|---|---|
| `MoveAxis`, `CanMoveAxis(*)` | manual hand-paddle-style slew rates; not needed by `rp`; deferred along with custom tracking rates |
| `SlewToAltAz*`, `SyncToAltAz`, `CanSlewAltAz*` | RA/Dec coverage is sufficient for `rp`'s tools; Alt/Az slew is a separate path through the coordinate module |
| `RightAscensionRate`, `DeclinationRate` setters | custom-rate tracking; not needed for sidereal-only MVP |
| `TrackingRate` setter (lunar / solar / king) | sidereal covers astrophotography; lunar/solar are uncommon and add a small mode-switch matrix |
| `FindHome`, `CanFindHome` | no hardware home sensor on the GTi |
| `Action` / custom commands | no driver-specific actions in MVP |
| Polar-alignment helpers, TPOINT, cone error | observational pointing model is the host's concern, not the driver's |
| WiFi station mode (mount on a routed network) | AP-mode UDP is verified; station mode just changes the bind-address selection — straightforward to add once a station-mode test setup exists |
| Multi-mount support on a single binary | `rp` assumes one mount per service; multi-mount is a separate concern |
| Auto-flip during tracking (Phase 2.5: `flip_policy.auto_flip_during_tracking` + `auto_flip_at_meridian_offset_hours`) | hosts like NINA / SGP / `rp` own flip timing themselves; mid-exposure auto-flip is a footgun for astrophotography and a separate state machine. Phase 6 lands explicit `SetSideOfPier`-driven flips only |
| Altitude-based safety floor (Phase 3 in [`docs/plans/star-adventurer-gti-meridian-flip.md`](../plans/star-adventurer-gti-meridian-flip.md)) | replaces the rectangular Dec envelope with an altitude floor; independent of Phase 6 and can land in either order |

## References

### Authoritative

- [Sky-Watcher motor-controller command set] — the wire protocol
  specification, including the §6 Wi-Fi note that the same protocol runs
  on UDP/11880. In-tree engineering notes (compatibility list, our
  empirical findings, implementation gotchas) live alongside at
  [`docs/references/skywatcher-motor-controller-command-set.md`](../references/skywatcher-motor-controller-command-set.md).
- [INDI eqmod driver source](https://github.com/indilib/indi-3rdparty/tree/master/indi-eqmod)
  — the canonical open-source reference implementation; we cross-check
  ambiguous bits of the spec against this driver.
- [EQMOD project](https://eq-mod.sourceforge.net/) — Windows-side
  reference driver and protocol-decoding documentation.
- [Astro-Physics "Park Positions Defined"](https://astro-physics.info/tech_support/mounts/park-positions-defined.pdf)
  — canonical reference for the five named park positions (Park 1
  through Park 5) the [§Home pose](#home-pose) config exposes,
  including the per-hemisphere celestial-Dec formulae and the
  east-side / west-side scope orientations.

### Misleading and explicitly out-of-scope

The Sky-Watcher developer downloads page hosts two other protocol PDFs
that look superficially relevant but are *not* applicable to the GTi USB
or WiFi connection — recording here so future readers don't fall into
the same trap:

- [SynScan v3.3 hand-control protocol](https://inter-static.skywatcher.com/downloads/synscanserialcommunicationprotocol_version33.pdf)
  — describes commands the SynScan **hand controller** accepts on its
  RS-232 port at 9600 baud (single-char ASCII, `#`-terminated, J2000
  RA/Dec). The GTi has no hand controller and its USB / WiFi do not
  speak this protocol. Empirically verified.
- [SynScan App Protocol](https://inter-static.skywatcher.com/downloads/synscan_app_protocol_20250930.pdf)
  — describes a remote-control protocol for the **SynScan app itself**
  (third-party software → SynScan app on phone/desktop → mount on
  UDP/TCP 11881). Useless as a direct-to-mount protocol because it
  requires SynScan app as a middleman.

### Related rusty-photon docs

- [`docs/services/rp.md`](rp.md) — the mount-tool consumer; defines the
  high-level slew/park/track/sync/abort tool surface and the
  `EquatorialCoordinateType` and `SiteLatitude`/`SiteLongitude`
  expectations.
- [`docs/services/qhy-focuser.md`](qhy-focuser.md) — the closest
  architectural sibling; same transport/manager/device/mock pattern,
  feature-flags, and BDD layout.
- [`docs/references/ascom-alpaca.md`](../references/ascom-alpaca.md) —
  ASCOM Alpaca protocol overview and error-code reference.
- [`docs/skills/development-workflow.md`](../skills/development-workflow.md)
  — the design-first / test-first / implementation workflow this doc
  is the Phase 1 deliverable for.
