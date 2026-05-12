# Star Adventurer GTi Service

## Overview

The `star-adventurer-gti` service is an ASCOM Alpaca Telescope driver for the
Sky-Watcher Star Adventurer GTi German Equatorial Mount. It speaks the
[Sky-Watcher motor-controller command set] over either USB-CDC serial or WiFi
(UDP/11880) — the same wire protocol on both transports — and exposes the
mount as an ASCOM Telescope for any Alpaca-compatible client (NINA, SGPro,
`rp`, etc.).

The driver implements the subset of `ITelescopeV3` that the rusty-photon
ecosystem actually needs (slew, sync, track, park, abort, side-of-pier),
plus the standard device metadata. PulseGuide, MoveAxis, custom tracking
rates, and polar-alignment helpers are explicitly deferred (see [§MVP
Scope](#mvp-scope)).

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
We implement software park (slew to encoder 0/0) and require site lat/lon
in the config (see [§ASCOM Telescope Mapping](#ascom-telescope-mapping)).

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
| `CanSetGuideRates` | `false` | PulseGuide deferred |
| `CanPulseGuide` | `false` | PulseGuide deferred |
| `CanMoveAxis(any)` | `false` | manual slew deferred |
| `CanFindHome` | `false` | no hardware home position |
| `CanPark` | `true` | implemented (software park) |
| `CanUnpark` | `true` | implemented |
| `CanSetPark` | `false` | park position fixed at encoder 0/0 in MVP |
| `CanSetPierSide` | `false` | mount manages pier side via slew direction |
| `CanSetSideOfPier` | `false` | same as above |
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
| `AtPark` | driver-state flag; set by `Park`, cleared by `Unpark` and by any motion command |
| `AtHome` | `false` (no hardware home concept) |
| `SideOfPier` | derived from RA-axis encoder + site latitude (sign matters: spec defines E/W meaning differently in N vs S hemisphere) |
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
| `Park()` | stop tracking, slew both axes to encoder 0/0, when both report stopped set `AtPark=true`. **Tracking remains off after park** (per ASCOM) |
| `Unpark()` | clear `AtPark`. Does NOT auto-enable tracking |
| `SetPark()` | `NOT_IMPLEMENTED` in MVP |
| `Tracking = true` | refuse with `INVALID_WHILE_PARKED` when parked; otherwise issue `:G<RA>` (Tracking + sidereal) + `:I<RA>` (sidereal step period) + `:J<RA>`. Dec axis untouched |
| `Tracking = false` | issue `:K<RA>` (decelerate to stop). Allowed while parked — Park already left tracking off and a caller re-asserting that should not error |
| `FindHome()` | `NOT_IMPLEMENTED` |
| `MoveAxis(*)` | `NOT_IMPLEMENTED` |
| `PulseGuide(*)` | `NOT_IMPLEMENTED` |
| `Action(name, parameters)` | `ACTION_NOT_IMPLEMENTED` for all action names in MVP |

### Slew lifecycle

```
SlewToCoordinatesAsync(ra, dec)
   │
   ├─ validate: !AtPark, ra ∈ [0,24), dec ∈ [-90,90]
   ├─ remember: TargetRightAscension/Declination = (ra, dec)
   ├─ compute: (ra_target_ticks, dec_target_ticks) from
   │           ra/dec + LST(now) + sync offset + side-of-pier choice
   │           (the post-stop pickup loop closes the residual that
   │           arises from RA drifting at sidereal rate during the
   │           goto, so the LST snapshot is taken at issue time)
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
   │     :G<axis><goto-mode>
   │     :S<axis>800000     target = encoder 0
   │     :J<axis>
   │
   ├─ background poll :f1 / :f2 until both stopped
   │     (no auto-abort on timeout — caller must AbortSlew if stuck;
   │      matches rp.md park behaviour)
   ├─ AtPark = true
   └─ Tracking remains false
```

### Side-of-pier

`SideOfPier` is derived from the RA-axis encoder position and the site
latitude, following the ASCOM / EQMOD convention:

- Convert RA-axis encoder ticks → mechanical hour-angle (signed, range
  `[-12, +12)`).
- In **northern hemisphere** (`SiteLatitude > 0`): mechanical HA in
  `[-12, 0)` (object east of meridian, mount in the "normal" pointing
  state with counterweight east and OTA west) → `PierSide::West`;
  mechanical HA in `[0, +12)` (object past meridian, mount in the
  post-meridian-flip / "through-the-pole" state) → `PierSide::East`.
- In **southern hemisphere** (`SiteLatitude < 0`): the convention
  inverts.

The boundary at `HA = 0` (meridian) — not the earlier `HA = ±6` split —
matches what ConformU and standard GEM drivers (EQMOD, INDI eqmod)
expect.

The MVP does **not** implement `DestinationSideOfPier` (slew-target
prediction); reads always return one of `East` / `West` based on current
encoder position.

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
    "tracking_rate": "sidereal"
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
| `conformu_integration.rs` (gated on `conformu`) | ASCOM Telescope compliance via `ConformUTestBuilder::run()` — same shape as `qhy-focuser`. Currently **not wired into the nightly `conformu` workflow** (no `[package.metadata.conformu]` opt-in); see [§"Running ConformU manually"](#running-conformu-manually) for why and how to drive it by hand until `PulseGuide` is implemented |

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

### Running ConformU manually

This service is deliberately **not** in the nightly `conformu`
workflow rotation. `ConformUTestBuilder::run()` (which the
in-tree integration test uses, matching `qhy-focuser`) runs two
phases in sequence: `alpacaprotocol` then `conformance`. The
`alpacaprotocol` phase polls `IsPulseGuiding` while exercising
`PulseGuide` PUT-parameter-order tests *even when
`CanPulseGuide=false`*, and treats the spec-mandated
`NotImplementedException` from `IsPulseGuiding` as a fatal
protocol-test failure. The right structural fix is implementing
`PulseGuide` so `CanPulseGuide` flips to `true` and the polling
stops being a problem; until then the integration test cannot
be wired in without either deviating the driver from the ASCOM
spec or carrying a non-standard test harness that diverges from
the other services. Neither is wanted. Re-add
`[package.metadata.conformu]` in `services/star-adventurer-gti/Cargo.toml`
once PulseGuide lands.

In the meantime, run ConformU's `conformance` phase by hand:

```bash
# Terminal 1: start the service in mock mode on a fixed port.
cargo run -p star-adventurer-gti --features mock -- \
    --config services/star-adventurer-gti/conformu-test-config.json \
    -l info

# Terminal 2: drive ConformU against it.
conformu conformance \
    http://localhost:11117/api/v1/telescope/0
```

Expect the conformance run to report:

- **0 errors** — anything here is a real driver regression.
- **9 issues**, all of which are deferred-by-design or
  upstream-framework problems:
  - `DestinationSideOfPier` ×1 and `SOPPierTest` ×4 — the MVP
    explicitly defers `DestinationSideOfPier` (see [§MVP Scope](#mvp-scope)).
    `SOPPierTest` depends on it and inherits the failure four
    times under different RA/Dec inputs.
  - `SOPPierTest` ×2 — the safety envelope correctly rejects
    cross-meridian slews that would land at `RA mech-HA = ±9h`
    (well outside the default ±6h counterweight-horizontal
    envelope). `SOPPierTest` exercises the pier-flip code paths
    by commanding such slews; the driver returns
    `InvalidValueException` from the safety gate before any
    wire motion. This is the same envelope-rejection mechanism
    Phase 4 added — see [§Phase 4 driver-logic changes].
  - `TrackingRate Write` ×2 — an upstream `ascom-alpaca-rs`
    bug: invalid Alpaca enum values are rejected with HTTP
    `400 BadRequest` (axum/serde rejection) before reaching
    the driver's `set_tracking_rate` handler, instead of
    returning HTTP 200 with `ErrorNumber=0x401 (InvalidValue)`
    as the Alpaca spec requires. ConformU sends `5` and `-1`
    and flags both.

Any issue or error outside that list — and in particular any
`SlewTo*` / `SyncTo*` row reporting a tolerance exceedance
("`Actual RA: ..., Target RA: ...`" with > 10″ delta) — is a
real regression. Fix the driver, then refresh this section.

> Note: `conformu alpacaprotocol …` against this service will
> abort with a fatal `IsPulseGuiding` `NotImplementedException`
> until `PulseGuide` is implemented. That is the upstream
> ConformU bug above and is expected.

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
- **Mechanical safety envelope** — driving the mount into the
  counterweight-up region with ConformU's pier-flip tests stalled
  the motor against a hard stop while the encoder counter kept
  advancing (audible motor noise, OTA stationary). `MountConfig`
  now carries `ra_min_hours` / `ra_max_hours` /
  `dec_min_degrees` / `dec_max_degrees`. `SyncToCoordinates`,
  `SlewToCoordinatesAsync`, and `Park` reject targets outside
  the envelope with `INVALID_VALUE` before any wire motion.
  Defaults: `±6 h` RA (counterweight-horizontal east/west on a
  Northern-Hemisphere polar-aligned GTi), `±90°` Dec.
- **Slew watcher abort on `:f` blocked** — both the slew and
  park completion watchers issue `:L` on both axes and clear
  `slew_in_progress` if either axis reports `blocked=true`.
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


### In-scope (Phases 1–3, all landed)

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
- `Park` / `Unpark` (software park to encoder 0/0)
- `AtPark` / `AtHome` reads
- `SideOfPier` read
- `Slewing` poll
- `UTCDate` / `SiderealTime` (host-clock-derived)
- Mock transport for BDD tests; feature-gated mock for `test_lib.rs` and
  ConformU
- ASCOM Alpaca discovery on the standard port

### Deferred (not in MVP)

| Capability | Why deferred |
|---|---|
| `PulseGuide`, `CanPulseGuide`, `GuideRate*` | autoguiding via the mount's ST4-equivalent — protocol command exists (`P`) but driver-side rate calibration plus BDD coverage is a substantial extension; PHD2 already drives guide pulses through `phd2-guider`'s direct-mount path, so no rp-side dependency |
| `MoveAxis`, `CanMoveAxis(*)` | manual hand-paddle-style slew rates; not needed by `rp`; deferred along with custom tracking rates |
| `SlewToAltAz*`, `SyncToAltAz`, `CanSlewAltAz*` | RA/Dec coverage is sufficient for `rp`'s tools; Alt/Az slew is a separate path through the coordinate module |
| `RightAscensionRate`, `DeclinationRate` setters | custom-rate tracking; not needed for sidereal-only MVP |
| `TrackingRate` setter (lunar / solar / king) | sidereal covers astrophotography; lunar/solar are uncommon and add a small mode-switch matrix |
| `SetPark`, `CanSetPark` | park position is fixed at encoder 0/0 (the mount's natural power-up state); user-defined park positions are a follow-up |
| `FindHome`, `CanFindHome` | no hardware home sensor on the GTi |
| `Action` / custom commands | no driver-specific actions in MVP |
| `DestinationSideOfPier` | requires slew planner; current-pier read is enough for `rp`'s built-in tools |
| Polar-alignment helpers, TPOINT, cone error | observational pointing model is the host's concern, not the driver's |
| WiFi station mode (mount on a routed network) | AP-mode UDP is verified; station mode just changes the bind-address selection — straightforward to add once a station-mode test setup exists |
| Multi-mount support on a single binary | `rp` assumes one mount per service; multi-mount is a separate concern |

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
