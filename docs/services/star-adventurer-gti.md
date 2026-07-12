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
                │   │ MountManager             │  │
                │   │  (Sky-Watcher handshake, │  │
                │   │   parameters + snapshot, │  │
                │   │   poll-pause guards)     │  │
                │   └────────────┬─────────────┘  │
                │                │ Session<SkywatcherCodec> + WhileOpen
                │                ▼                │
                │   ┌──────────────────────────┐  │
                │   │ rusty-photon-shared-     │  │
                │   │   transport              │  │
                │   │  (refcount, slot,        │  │
                │   │   open/close, hooks)     │  │
                │   └────────────┬─────────────┘  │
                │                │ bytes          │
                │                ▼                │
                │   ┌──────────────────────────┐  │
                │   │ FrameTransport (trait)   │  │
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

### Shared transport (Phase E of issue #257)

The connection lifecycle — refcounted multi-client open/close, handshake,
background poll task, request arbitration lock — lives in the workspace
crate `rusty-photon-shared-transport` (`crates/rusty-photon-shared-transport/`).
The service plugs into it via:

* **`SkywatcherCodec`** (`src/codec.rs`) — implements
  [`rusty_photon_shared_transport::Codec`]. The codec's `Response`
  type is the raw `Vec<u8>` frame; per-command typed decoding lives in
  [`MountManager::send`] and the handshake hook, because the Sky-Watcher
  protocol's success-body interpretation depends on the originating
  command (a 6-hex-byte body decodes as `Response::U24` for `:a` but
  as `Response::Position` for `:j`).
* **`SerialTransportFactory`** / **`UdpTransportFactory`** (`src/transport/{serial,udp}.rs`)
  — `TransportFactory` impls that hand back `Box<dyn FrameTransport>`s
  configured with `\r` framing on serial or one-datagram-per-frame on
  UDP. Selection at `ServerBuilder::build()` time picks one based on
  `config.transport`. The same `UdpFrameTransport` adapter from the
  shared crate preserves the mount's "exactly one frame per packet"
  framing strictness.
* **`MountManager`** (`src/manager.rs`) — thin wrapper around
  `Arc<SharedTransport<SkywatcherCodec>>`, plus the mount-specific
  parameter cache (`MountParameters`), background snapshot
  (`MountSnapshot`), and `PollPauseGuard` ref-counted polling-task
  pause. The handshake (`:F`/`:a`/`:b`/`:g`/`:e`/`:j`) and the
  poll-loop (`:j`/`:f`) live in `Hooks` closures inside
  `MountManager::new`, alongside two teardown hooks: `on_last_disconnect`
  issues the best-effort halt sequence (`:L1`, `:L2`, `:K1`) every time the
  last Alpaca client disconnects — in service-lifetime mode the transport
  stays open, so the parameter cache is retained — and `shutdown` runs the
  same sequence once at service shutdown before the transport drops,
  additionally clearing the parameter cache.

The device's session lives in `MountDevice::session:
RwLock<Option<Session<SkywatcherCodec>>>` — the slot's presence is the
single source of truth for "the user is connected" (the pre-Phase-E
`requested_connection` bool was removed). Watcher tasks (slew, park,
pulse-guide) acquire their own sessions on spawn so the user's
`set_connected(false)` returns immediately after closing the device's
session — the watcher's session keeps the underlying transport open
until it finishes naturally. Watchers peek at the device's session
slot (briefly, no wire I/O under the read lock) to detect "user
disconnected" and bail.

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
| Motor board version | `:e1` | `0x03300C` (mount type 0x03, fw v0x30.0x0C) | mount-family detection (EQ vs AZ), **and identity gate** (see [§Initialisation sequence](#initialisation-sequence)) |

CPR varies between the GTi's RA and Dec axes and between firmware
revisions; the driver reads both rather than assuming.

The protocol exposes **no native park position** and **no site lat/lon**.
We implement software park (target encoder pair sourced from config,
or — failing that — the live snapshot after the
`unpark_from_ap_position` encoder seed, see
[§Park lifecycle](#park-lifecycle), [§Park persistence](#park-persistence),
and [§Unpark from AP position](#unpark-from-ap-position)) and require
site lat/lon in the config (see
[§ASCOM Telescope Mapping](#ascom-telescope-mapping)).

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

1. `:e1` → **identity gate**. Decode as
   [`skywatcher_motor_protocol::Response::U24`]; the high byte must be a
   known Sky-Watcher mount-type ID (see
   [`MountType::from_motor_board_version`][mount-type]). On any other
   reply (framing malformed, payload wrong shape, mount-type byte
   outside the whitelist) the driver aborts the handshake with
   [`StarAdvError::WrongDevice`][wrong-device] *before* sending
   anything else — the wrong device sees exactly one frame (`:e1\r`).
2. `:F1` → expect `=\r` (Initialize axis 1)
3. `:F2` → expect `=\r` (Initialize axis 2)
4. `:a1` → record RA-axis CPR
5. `:a2` → record Dec-axis CPR
6. `:b1` → record TMR_Freq
7. `:g1` → record RA-axis high-speed ratio
8. `:g2` → record Dec-axis high-speed ratio
9. `:j1` / `:j2` → record initial encoder positions

Steps 4-9 seed the in-memory mount-parameters cache used by the
coordinate module and the slew planner. The motor-board-version reply
from step 1 is also cached (`MountParameters::motor_board_version`)
for diagnostic logging.

**Why `:e1` is first** (issue #254): the 2026-05-17 San Diego hardware
session pointed the driver at `/dev/serial/...` for a QHY focuser by
mistake. The pre-fix handshake had already sent seven mount-specific
commands (`:F1`, `:F2`, `:a1`, `:a2`, `:b1`, `:g1`, `:g2`) to the wrong
device before reaching the identity check at step 8. Reordering so
`:e1` is first and strictly-validated bounds the wrong-device blast
radius to a single innocuous inquiry, and the ASCOM error surfaced to
the operator names the configured port plus the wrong-device
hypothesis (see [`StarAdvError::WrongDevice`][wrong-device]'s `Display`
shape).

[mount-type]: ../../crates/skywatcher-motor-protocol/src/mount_type.rs
[wrong-device]: ../../services/star-adventurer-gti/src/error.rs
[`skywatcher_motor_protocol::Response::U24`]: ../../crates/skywatcher-motor-protocol/src/response.rs

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
| `CanSetPark` | effectively `true` by default | The driver now always resolves a config-file path at startup (the `--config <path>` argument if given, else the platform config dir — see [§Device identity (UniqueID)](#device-identity-uniqueid)), so it always has a path to persist `SetPark` writes to. At the library layer `CanSetPark` still keys on whether a config path was supplied to `MountDevice`; only `MountDevice::new` (no path) reports `false`, which the binary never does. See [§Park persistence](#park-persistence) |
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
| `Connected = true` | acquire a session on the already-open transport (opened eagerly at service start — see [§Connection Lifecycle](#connection-lifecycle)); refcount bump, then the post-acquire hooks `seed_after_connect` (fresh-power-up AP-pose encoder seed) and `load_park_target_after_connect` run |
| `Connected = false` | release the session. On the last client disconnect, issue the `:L1`/`:L2`/`:K1` safety stop; the transport stays open and background polling continues until service shutdown |
| `SlewToCoordinatesAsync(ra, dec)` | validate (not parked, valid coords), compute target encoder positions for `LST(now + MIN_SLEW_DWELL)` so the post-slew RA reading lands on `target_RA` instead of drifting at sidereal rate during the slew, issue `:G` `:S` `:J` per axis, set `Slewing=true`. Returns immediately; caller polls `Slewing` |
| `SlewToCoordinates(ra, dec)` | wraps the async variant and waits for `Slewing` to clear (bounded by a generous timeout) before returning. Mandatory per ASCOM when `CanSlew=true` |
| `SlewToTargetAsync()` | uses last-set `TargetRightAscension`/`Declination` |
| `SlewToTarget()` | synchronous variant of the above; same wait semantics as `SlewToCoordinates` |
| `SyncToCoordinates(ra, dec)` | issue `:E<axis><pos>` for each axis (set encoder position), update the cached snapshot so an immediate `RightAscension` / `Declination` read reflects the sync without waiting for the next background poll, and **update `TargetRightAscension` / `TargetDeclination`** to the synced coordinates (per ASCOM ITelescopeV3 — a successful Sync writes Target) |
| `SyncToTarget()` | uses last-set target |
| `AbortSlew()` | refuse with `INVALID_WHILE_PARKED` when parked; otherwise issue `:L1` `:L2` (instant stop), clear `Slewing`, do NOT auto-restore tracking |
| `Park()` | stop tracking, slew both axes to the in-memory park-target encoder pair (loaded from config or captured at handshake — see [§Park lifecycle](#park-lifecycle)), when both report stopped set `AtPark=true`. **Tracking remains off after park** (per ASCOM) |
| `Unpark()` | clear `AtPark`. Does NOT auto-enable tracking |
| `SetPark()` | capture current encoder pair, write back into the running config file (only the `mount.park_ra_ticks` / `mount.park_dec_ticks` keys are touched — see [§Park persistence](#park-persistence)), update the in-memory park target. Refuses if not connected or while slewing. (The binary always resolves a config path, so the historical "no `--config`" refusal no longer fires in practice — see [§Park persistence](#park-persistence).) |
| `SetSideOfPier(side)` | request a meridian flip to the named side. No-op success when `side == current_side`; otherwise issues a through-wrap flip slew to the current target. Returns `NOT_IMPLEMENTED` when `flip_policy.enabled = false`; `INVALID_VALUE` when `side` is `Unknown`; the usual `NOT_CONNECTED` / `INVALID_WHILE_PARKED` / `INVALID_OPERATION` (while slewing) refusals otherwise. See [§Meridian flip](#meridian-flip) |
| `Tracking = true` | refuse with `INVALID_WHILE_PARKED` when parked; otherwise issue `:G<RA>` (Tracking + sidereal) + `:I<RA>` (sidereal step period) + `:J<RA>`. Dec axis untouched |
| `Tracking = false` | issue `:K<RA>` (decelerate to stop). Allowed while parked — Park already left tracking off and a caller re-asserting that should not error |
| `FindHome()` | `NOT_IMPLEMENTED` |
| `MoveAxis(*)` | `NOT_IMPLEMENTED` |
| `PulseGuide(direction, duration)` | rate-shifted tracking pulse on the targeted axis; returns immediately after spawning the watcher task. Refuses when parked / disconnected / slewing / same-axis pulse already in flight. `duration = 0` is a no-op success. See [§PulseGuide lifecycle](#pulseguide-lifecycle) for the wire path. |
| `SetGuideRateRightAscension(deg_per_sec)` | validate `(0, SIDEREAL_DEG_PER_SEC)` exclusive; store as fraction = `deg_per_sec / SIDEREAL_DEG_PER_SEC`. Out-of-range → `INVALID_VALUE`. |
| `SetGuideRateDeclination(deg_per_sec)` | same shape as RA. |
| `Action(name, parameters)` | Driver-specific Actions: `SetUnparkFromApPosition`, `SetPreferredApPark`, `UnparkFromApPosition`. See [§Custom Actions for runtime control](#custom-actions-for-runtime-control). All other action names return `ACTION_NOT_IMPLEMENTED`. |

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

1. **Config-file value** (per axis). The driver always resolves a
   config-file path at startup (see
   [§Device identity (UniqueID)](#device-identity-uniqueid)); that file
   is re-read on every connect and `mount.park_ra_ticks` /
   `mount.park_dec_ticks` are used when present. Per-axis: if only
   `park_ra_ticks` is set, RA comes from the file and Dec falls through
   to step 3.
2. **In-memory config value** (per axis). Only reachable at the library
   layer when `MountDevice` was constructed without a config path
   (`MountDevice::new`, exercised by unit tests): the `MountConfig`
   defaults are used and `SetPark` is unreachable, so these values do
   not change in-process. The binary never takes this branch because it
   always supplies a resolved path.
3. **Live-snapshot fallback** (per axis). The current encoder
   reading from the [`MountManager`] snapshot. Two cases:
   - **Fresh power-up with `unpark_from_ap_position` set to a named
     park (`ap_park_1..ap_park_5`).** The driver runs
     [`seed_after_connect`](#unpark-from-ap-position) before loading
     the park target, so the snapshot already reflects the named
     pose's logical encoder values (e.g. `ap_park_3` → `mech_HA =
     -6h`, `mech_dec = +90°`). `Park` therefore defaults to "return
     to the pose the operator powered up at".
   - **Mid-session reconnect** *or* **fresh power-up with
     `unpark_from_ap_position = "ap_park_0"`.** The seed skips on a
     non-fresh firmware encoder (and is unconditionally a no-op when
     `ap_park_0` is configured), so the snapshot equals the
     handshake-captured `:j1` / `:j2` reading. This is the "park
     where the OTA already is" semantic operators expect from a
     reconnect.
4. **Last resort** — encoder `0`. Only reachable if the snapshot
   somehow produced no position read, which today is unreachable.

Re-reading the config file on every connect means a successful
`SetPark` (or a manual operator edit between connects) takes effect on
the next reconnect without restarting the driver. After load the
in-memory target is fixed for the session unless `SetPark` is called
again (see [§Park persistence](#park-persistence)).

### Park persistence

`SetPark` writes the current encoder pair back into the **same JSON
config file** the driver loaded at startup. No separate state file.
Concretely:

1. **Capability gate.** The driver now always resolves a config-file
   path at startup — the `--config <path>` argument if given, else the
   platform config dir (see
   [§Device identity (UniqueID)](#device-identity-uniqueid)). That path
   is the one `SetPark` persists to, so **`CanSetPark` is effectively
   `true` by default**: park persistence works out-of-the-box, even on
   a no-arg launch. (Earlier builds gated `CanSetPark` on `--config`
   being present and left it `false` for `Config::default()` runs.)
   The library-level capability still keys on whether a config path was
   supplied to `MountDevice` — `MountDevice::new` (no path) reports
   `false` and `SetPark` returns `ASCOM_NOT_IMPLEMENTED` — but the
   binary always supplies a path, so that branch is now only reachable
   in unit tests. The capability flag is computed at startup; clients
   that cache it once per session see a stable answer.
2. **Refusal cases.** `SetPark` also refuses (`NOT_CONNECTED` /
   `INVALID_OPERATION`) when the device is disconnected or while a
   slew / park is in progress — the "current encoder pair" wouldn't be
   stable otherwise.
3. **Fresh encoder read.** The encoder pair (and the per-axis running
   flag the in-progress refusal checks) is read **live from the wire** —
   a synchronous `:j` / `:f` poll on both axes via the connected
   session — not from the background poll snapshot, which lags the wire
   by up to one `polling_interval`. Reading live means `SetPark`
   persists the true current encoder even if the operator moved the
   mount out-of-band immediately before the call, and removes a
   connect/poll timing race that made the BDD persistence scenario flaky
   on slow CI (issue #308): the eager service-start handshake seeds the
   snapshot before the test sets the encoder, and the user's connect is
   a refcount bump rather than a fresh handshake. (The write itself is
   fully `fsync`'d and atomically renamed before the call returns — see
   step 5 — so the persisted file is durable, not fire-and-forget.)
4. **Read-as-`Value`, write only park keys.** The driver reads the
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
5. **Atomic rename via `tempfile::NamedTempFile`.** The temp file is
   created in the **same directory** as the destination (required for
   `persist` to use POSIX `rename` rather than copy-and-delete),
   `sync_all`'d (fsync the file data) so a crash after rename can't
   surface a renamed-but-zero-length file, `persist`'d on success,
   then on unix the parent directory is fsync'd so the rename itself
   is durable. Same pattern as
   [`services/rp/src/persistence/document.rs::write_sidecar_sync`](../../services/rp/src/persistence/document.rs).
   On any error path the temp file auto-deletes via `Drop`, so a
   panic mid-write doesn't leave a `*.tmp` artifact behind.
6. **Blocking I/O on the blocking pool.** The whole read+parse+stage+
   fsync+rename sequence runs inside `tokio::task::spawn_blocking` so
   the async runtime isn't held up. Same pattern as `write_sidecar`.
7. **In-memory update follows disk.** The in-memory park target is
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
violate the capability contract. The binary always resolves a real
config path now, so it always *does* have somewhere to persist; the
`false` answer survives only as the library default for
`MountDevice::new`, exercised by unit tests.

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
  into the unverified mirror of the CW exclusion zone.

#### Safety envelope (CW exclusion zone)

Mechanical safety on the GTi is dominated by one specific hazard:
**the counterweights must not rise more than 0.95 h (≈ 14°) above
horizontal at any point in a slew path.** As the dec axis rotates
around the polar axis, the CW shaft crosses horizontal at
`mech_HA = +0.95`, peaks at `mech_HA = +6`, and crosses horizontal
again at `mech_HA = +11.05`. The arc where the CW is more than
0.95 h above horizontal — and where in turn the OTA on the opposite
end of the dec axis can contact the tripod or pier — is the
contiguous range `(+0.95, +11.05)`. The constraint is **one-sided**:
the negative-mech_HA half puts the CW into the floor beneath the
pier rather than above the mount head, so no mirror zone applies.

The driver enforces this as a single interval on the **chosen-side
encoder mech_HA**, in `MountConfig::cw_exclusion_zone` — an active
`{ min_hours, max_hours }` interval (defaults `0.95` and `11.05` — the
wide zone derived from the 0.95 h rule and hardware-verified at lat
32.7°N), or `null` to disable:

- A slew or sync's *target* mech_HA inside the interval is rejected
  with `INVALID_VALUE` before any motion (the destination check).
- A slew's *path* — the linear mech_HA sweep from current encoder to
  target encoder — is also checked. Crossings happen even when both
  endpoints sit outside the zone (e.g. cur `+0.5 h` → tgt `+11.5 h`
  sweeps through the entire zone interior). Path violations on a
  non-flip slew return `INVALID_OPERATION` ([`check_non_flip_ra_path`]).
  Flip slews have one degree of freedom (canonical short or long way
  around): [`flip_slew_ra_delta`] tries both and returns
  `INVALID_OPERATION` only when both directions cross the zone.
- For natural-side targets, `target_mech_HA = celestial_HA` (folded
  signed into `[−12, +12)`).
- For flipped-side targets, `target_mech_HA = celestial_HA + 12 h`
  (folded).
- Setting `cw_exclusion_zone` to `null` (disabled) turns off both the
  destination and path checks — used by BDD scenarios that pass
  hardcoded celestial coords whose computed mech_HA depends on
  wallclock LST.

Historical note: a narrower `(+6.95, +11.05)` zone was used before
2026-05-17. That captured only the *outer* portion of the exclusion
arc (CW descending past the pier from above), missing the inner
ascent through which the OTA contacted the tripod during a
`SetSideOfPier` from Park 3 in San Diego. The wider zone closes
that gap; the path-aware long-way-also-cross check ensures the
helper can't silently redirect a slew through the newly-covered
inner half just because the short way was blocked.

`Park` writes the target encoder ticks directly without consulting
the exclusion-zone check — also mirroring EQMOD's privileged-park
pattern. The operator's `park_ra_ticks` / `park_dec_ticks` are
assumed to have been validated against the zone at the time of
configuration.

#### Altitude floor

The second, independent gate on slew / sync targets is a **minimum
apparent-altitude floor** (`min_altitude_degrees`, default `0.0` — the
geometric horizon). The driver computes the target's local altitude
from its hour angle, declination, and the site latitude:

```
sin(alt) = sin(lat) · sin(dec) + cos(lat) · cos(dec) · cos(HA)
```

and rejects the operation with `INVALID_VALUE` (naming the computed
altitude and the configured floor) when `alt < min_altitude_degrees`.
A target *at* the floor is accepted. Altitude is a celestial property
of the target — the check uses the celestial `HA = LST − RA`, so it is
the same for both pier sides of a flip-planned slew.

This replaced the earlier rectangular celestial-Dec envelope
(`dec_limits`, removed 2026-07-01; a config file still carrying
`dec_limits` keeps loading — serde ignores unknown fields — but the
field has no effect). The rectangle was the
wrong shape for what operators mean by "don't slew there": the local
horizon is a tilted great circle on the celestial sphere, so a
rectangle is simultaneously too loose (accepts below-horizon targets at
mid Dec, e.g. `(HA = −3 h, dec = −40°)` at LAT 45°N → alt `−4°`) and
too tight when narrowed (rejects legitimate above-horizon targets like
`(HA = −3 h, dec = −30°)` → alt `+4.5°`). The altitude floor follows
the horizon circle exactly.

The floor accepts values in `[-90, +90]`:

- `0.0` (default) — geometric horizon. Refraction-corrected pointing
  (apparent horizon ≈ geometric `−0.5°`) is available by setting
  `-0.5`.
- `5.0` / `10.0` — operator buffer for refraction, horizon light
  pollution, or local obstructions.
- Negative values allow below-horizon pointing (dust-cap operations,
  closed-roof flats). The driver logs `info!` at startup when the floor
  is negative so the relaxed state is discoverable in support
  transcripts. `-90` never rejects anything (the check is
  effectively disabled — the BDD suite ships this in its default test
  config because scenario targets are wallclock-LST-dependent).

Like the CW exclusion zone, the floor gates `SlewToCoordinatesAsync` /
`SlewToTargetAsync`, `SyncToCoordinates` / `SyncToTarget`, and the
per-side validation inside `DestinationSideOfPier`. `Park` is exempt —
park targets are mechanically-defined encoder ticks the operator chose
explicitly via `SetPark` (the same privileged-park pattern as the
exclusion-zone check). The out-of-range celestial gate
(`validate_coordinates`, RA `[0, 24)` / Dec `[-90, +90]`) remains a
separate, earlier check.

#### Tracking-time safety guard

The safety envelope above is enforced at *slew* time. But `Tracking =
true` issues `:G` + `:I` + `:J` on the RA axis and from that point the
firmware advances the encoder autonomously — the driver sends no
further wire commands and the slew-time path checks never re-run. A
multi-hour session that begins at a safe negative `mech_HA` drifts up
across the zone entry (`mech_HA = +0.95` by default) with no operator
action: the same physical failure mode as a zone-crossing slew, reached
via tracking drift instead of a slew command (issue #259).

A per-connection background guard closes that gap. While `Tracking =
true` it watches the live encoder `mech_HA` — read from the snapshot
the background poll loop already refreshes, at `polling_interval`, so
the guard adds no extra wire traffic. When `mech_HA` enters the band
`(min_hours − margin, max_hours + margin)` — the active
`cw_exclusion_zone` widened by `tracking_guard_margin_hours` on each
edge — the guard:

1. issues `:K1` to stop the RA axis,
2. clears the in-memory `Tracking` flag to match the wire, and
3. emits a `warn!` explaining why.

It does **not** pick a pier side or flip. Post-guard state is
`Tracking = false`, `Slewing = false`, the encoder wherever it was when
the guard fired (no automatic park). The operator — or higher-level
automation — decides what to do next: flip via `SetSideOfPier`, slew
elsewhere, or park.

`tracking_guard_margin_hours` (default `0.05` h ≈ 45 s of sidereal
drift) lets cautious operators stop *before* the zone entry rather than
at it; `0.0` stops exactly at the zone's `min_hours`. A non-finite,
negative, or over-cap (> 1.0 h) margin is rejected at config load by the
`TrackingGuardMarginHours` newtype's deserialize; the guard additionally
treats a non-finite or negative value as `0.0` as defense-in-depth. The
guard is **independent of `flip_policy.enabled`**: it is the safety floor
that keeps an unattended autoguided session from contacting hardware
whether or not meridian-flip support is configured. It is disabled only
when `cw_exclusion_zone` is `null` (`Disabled`). Because
`slew_to_coordinates_async` clears `Tracking` for a slew's duration, the
guard is naturally dormant during slews and resumes once the
completion watcher re-engages tracking on the new pose.

Driver-planned auto-flip during tracking (Phase 2.5) — flipping ahead
of the zone instead of just stopping — is a separate, larger change
tracked as Part 2 of issue #259 and remains
[deferred](#deferred-not-in-mvp). When it lands, this guard stays as the
belt-and-suspenders fallback for when an auto-flip can't find a safe
path.

#### Pier-side decision tree

`DestinationSideOfPier(ra, dec)` and `SlewToCoordinatesAsync(ra,
dec)` share the same selector:

1. If `flip_policy.enabled = false`, return the current `SideOfPier`.
2. Compute `target_HA = LST − ra` (signed, folded to `[−12, +12)`).
3. If the *current* side can reach the target without entering the
   CW exclusion zone, stay on the current side (no unnecessary flip):
   - **Pre-flip side** (`pierWest` in the Northern Hemisphere,
     `pierEast` in the Southern): "covers" means `target_HA ∉
     binding_zone`. This is wider than the legacy
     `[ra_min_hours, ra_max_hours]` window — the whole sky except
     the CW exclusion zone.
   - **Post-flip side**: "covers" means `|target_HA| ≤
     flip_range_hours`. This is an *operational* preference (after
     a flip, the driver expects to flip back to natural side soon),
     not a hard mechanical constraint. The post-flip side could
     mechanically be tracked far past `±flip_range_hours`, but the
     decision tree treats anything outside as "should flip back".
4. Otherwise return the *opposite* side.

The driver does not pre-emptively flip; it only flips when the
target can't be reached from the current side (per rule 3) or when
`SetSideOfPier` forces it. The post-flip operational window
(`flip_range_hours`) is small — `0.5 h` by default — so the
practical pattern is: pre-flip side covers most of the sky; an
explicit or implicit flip rotates the mount through the meridian to
the post-flip side for tracking a target past meridian crossing;
the next slew that targets HA outside the flip window auto-flips
back to the pre-flip side.

Park 1 / Park 5 anti-meridian poses are reachable via
`SlewToCoordinatesAsync` because they live at `target_HA = ±12 h`
on the natural / flipped pier respectively, and their corresponding
`mech_HA` (folded `−12` for Park 1 on pierWest, `0` for Park 5 on
pierEast) is outside the CW exclusion zone.

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

**RA axis:** the CW exclusion zone at
`mech_HA ∈ (+0.95, +11.05)` is one-sided and hemisphere-independent;
every other arc of the polar-axis circle is safe. The routing rule is
therefore stated directly in terms of the zone rather than as a sign
proxy:

- Compute the canonical short delta's mech_HA sweep
  (`[current_mech_HA, current_mech_HA + canonical_delta_ha]`).
- If that sweep stays outside `(zone_min, zone_max)` (modulo 24 h —
  the zone copy at `k ∈ {-1, 0, +1}` is checked, enough to cover any
  `|canonical_delta_ha| ≤ 12 h` path), use the canonical direction.
- Otherwise try the long way (`canonical ± cpr_ra`), which lands at
  the same modular destination via the opposite arc.
- If the long way *also* crosses the zone, there is no safe RA path
  between current and target and the slew is refused with
  `INVALID_OPERATION`. (Hardware-revealed 2026-05-17: a
  `SetSideOfPier` from Park 3 took the long way under the narrower
  `(+6.95, +11.05)` zone, which permitted the path; under the wider
  `(+0.95, +11.05)` zone both directions cross and the helper now
  rejects the slew.)

This handles the forward-flip case (pre-flip near meridian → post-flip
wrap, canonical CCW already in the safe negative half), the flip-back
case (post-flip wrap on either side of the `±12` boundary → pre-flip
near meridian, canonical short path stays in the safe arc), and the
edge case the prior sign-blind heuristic mis-fired on:
**hardware validation 2026-05-16, Park 4 N → Park 5 N flip-back.**
Current `mech_HA ≈ −11.97` (post-flip pierEast just east of the
saddle-east wrap), target `mech_HA ≈ +11.99` (pre-flip pierWest just
west of the same wrap). Canonical short delta is a ~4 k-tick CCW
nudge that physically just crosses the `−12 ↔ +12` wrap; the old
`|current| > cpr_ra/4 ⇒ "safe is positive"` rule misread that as
"in the post-flip half, force positive" and issued a `+cpr_ra − 4 k`
≈ +3.625 M-tick CW full revolution that swept mech_HA through
`(+6.95, +11.05)` and slammed the CW shaft into the pier. The
path-aware check preserves the safe canonical step.

Empty zone (`zone_min ≥ zone_max`) disables the routing — the
canonical short delta is always used. BDD tests rely on this to keep
small-distance scenarios from accidentally triggering the long way
when the wall-clock LST puts a synthetic target inside the default
zone.

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

`flip_policy.enabled` defaults to `false` until the through-wrap
flip-back path is hardware-verified end-to-end on a GTi.

**2026-05-16, lat 32.7°N (San Diego).** The AP Park 1–5 sequence ran
end-to-end:

1. `Park 3 → Park 2`: small Dec slew (−90° to celestial equator).
2. `Park 2 → staging (HA = 0, Dec = −57.3°)`: pre-flip pierWest small
   slew; no routing change.
3. `SetSideOfPier(East) → Park 4 N`: through-wrap flip slew — RA
   −180° (saddle west → east) + Dec +294° CW (through wrap, past
   NCP, ending past-pole at `dec_encoder ≈ −122.78°`).
4. `Slew to (LST + 12 h, Dec = +57.3°) → Park 5 N`: flip-back over
   the saddle-east wrap. Wire issued was RA **−3,631 ticks CCW**
   (canonical short path, ~0.024° of polar-axis rotation) + Dec
   −180° CCW via NCP. The earlier sign-blind heuristic issued a
   +3.625 M-tick CW full revolution instead and the operator
   powered the mount off mid-sweep — see §"Through-wrap slew
   routing" above for the path-aware fix.
5. `Park → Park 3 N`: clean unwind.

All five poses confirmed visually. The dec axis's analogous routing
helper (`flip_slew_dec_delta`) was not exercised on its
sign-blind edge case in this session (current dec was on the
`+cpr_dec/2` side of the wrap where the old heuristic happens to be
correct); a follow-up issue tracks porting the path-aware fix to that
function.

## Configuration

JSON, deserialised with `serde` + `humantime-serde` for `Duration`
fields. The transport block is a tagged enum: `usb` or `udp`.

The mount block is **validated at deserialize** (parse-don't-validate):
the range-carrying fields are newtypes whose `serde` `try_from` rejects an
out-of-range value during `load_config`, with the offending field named,
so a bad config fails at startup rather than mid-session. `flip_range_hours`
must be `(0, 0.95]`; `tracking_guard_margin_hours` `[0, 1.0]`; an active
`cw_exclusion_zone` must satisfy `-12 ≤ min_hours < max_hours ≤ 12`;
`min_altitude_degrees` must be finite in `[-90, 90]`. (This
replaced the former runtime `MountConfig::validate` / `FlipPolicy::validate`
— see [ADR-006](../decisions/006-typed-physical-quantities-for-mount-pointing.md).)

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
    "tls": null,
    "auth": null
  },
  "mount": {
    "name": "Star Adventurer GTi",
    "unique_id": "550e8400-e29b-41d4-a716-446655440000",
    "description": "Sky-Watcher Star Adventurer GTi German Equatorial Mount",
    "enabled": true,
    "site_latitude_deg": 37.7749,
    "site_longitude_deg": -122.4194,
    "site_elevation_m": 0.0,
    "settle_after_slew": "2s",
    "tracking_rate": "sidereal",
    "cw_exclusion_zone": { "min_hours": 0.95, "max_hours": 11.05 },
    "tracking_guard_margin_hours": 0.05,
    "min_altitude_degrees": 0.0,
    "park_ra_ticks": null,
    "park_dec_ticks": null,
    "flip_policy": {
      "enabled": false,
      "flip_range_hours": 0.5
    },
    "unpark_from_ap_position": "ap_park_0",
    "preferred_ap_park": "ap_park_3"
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
- `unique_id` is the ASCOM `UniqueID`. It is **generated, not
  hand-authored** — leave it empty (or omit it) and the driver mints a
  spec-compliant UUIDv4 on first run, persisting it to this file and
  never overwriting it. The example value above is illustrative; yours
  will differ. See [§Device identity (UniqueID)](#device-identity-uniqueid).
- `tracking_rate` accepts `"sidereal"` only in MVP. Field is reserved
  for future expansion.
- `cw_exclusion_zone` defines the CW exclusion interval in encoder
  `mech_HA` (signed hours folded `[−12, +12)`) as either an active
  `{ "min_hours": .., "max_hours": .. }` object or `null` (disabled).
  Slews / syncs whose chosen-side `mech_HA` falls inside the interval
  are rejected with `INVALID_VALUE`; flip slews and non-flip slews
  additionally check that their swept path doesn't cross the zone
  (`INVALID_OPERATION` if so — see
  [§Safety envelope](#safety-envelope-cw-exclusion-zone)).
  Default `{ "min_hours": 0.95, "max_hours": 11.05 }` for the GTi — the
  one-sided arc where the CW shaft rises more than 0.95 h above
  horizontal, peaking at `mech_HA = +6 h`. The negative-mech_HA side is
  *not* a CW exclusion zone (CW points into the ground beneath the pier,
  not above the mount head). An active zone must satisfy
  `-12 ≤ min_hours < max_hours ≤ 12`; use `null` to disable (tests, or
  mounts whose geometry differs).
- `tracking_guard_margin_hours` extends CW exclusion-zone enforcement
  to *tracking* time: a background guard stops the mount (`:K1`) once
  the live encoder `mech_HA` drifts into the active zone widened by
  `margin` on each edge, while `Tracking = true`. Defaults `0.05` h
  (≈ 45 s of sidereal drift); `0.0` stops exactly at the zone entry;
  valid range `[0, 1.0]`. Independent of `flip_policy.enabled`. See
  [§Tracking-time safety guard](#tracking-time-safety-guard).
- `min_altitude_degrees` rejects slew / sync targets whose computed
  apparent altitude (from HA + Dec + `site_latitude_deg`) is below the
  floor. Default `0.0` (geometric horizon); must be finite in
  `[-90, 90]`. Negative values permit below-horizon pointing and are
  logged `info!` at startup; `-90` effectively disables the check. See
  [§Altitude floor](#altitude-floor). (Replaced the rectangular
  `dec_limits` envelope 2026-07-01; a stale `dec_limits` key is
  ignored on load.)
- `park_ra_ticks` / `park_dec_ticks` are written by `SetPark` and read
  on every connect; absent (or `null`) at first run, populated once
  `SetPark` is called. Operators may set them by hand to pin a known
  mechanical pose. See [§Park persistence](#park-persistence) for the
  rules around when the driver writes to this file. When absent, the
  park target falls back to the live snapshot reading. With
  `unpark_from_ap_position` set to a named park (`ap_park_1..ap_park_5`)
  and a fresh power-up, that's the named pose's logical encoder values
  (`Park` defaults to "return to the pose you powered up at"); otherwise
  it's the handshake-captured reading. See
  [§Park lifecycle](#park-lifecycle).
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
- `unpark_from_ap_position` is **required** (no default in the schema
  sense, but the ship default is `"ap_park_0"`). Carries the
  operator's declared physical position assumption — one of
  `"ap_park_0"` through `"ap_park_5"`. `ap_park_0` ("current
  position") is the safe-by-default value: no encoder seeding on
  connect; the driver trusts whatever the firmware reports and the
  operator plate-solves and `SyncToCoordinates` to ground-truth.
  Setting it to a named park (`ap_park_1..ap_park_5`) tells the driver
  to seed the firmware encoder via `:E1` / `:E2` on every
  fresh-power-up connect to the codebase's convention for that
  pose — the operator's contract is to physically place the OTA at
  that park before powering on. Runtime-modifiable via the
  `SetUnparkFromApPosition` custom Action (persisted to the same
  config file). See [§Unpark from AP position](#unpark-from-ap-position).
- `preferred_ap_park` (optional) — the AP park `Park()` slews to.
  Defaults to `"ap_park_3"` (the visible-celestial-pole pose, the
  Sky-Watcher stock power-up pose). Runtime-modifiable via the
  `SetPreferredApPark` custom Action. The legacy
  `park_ra_ticks` / `park_dec_ticks` keys remain as raw-encoder
  overrides for ops pinning a specific tick pair; when both are
  set, the explicit tick pair wins.

### Device identity (UniqueID)

ASCOM Alpaca requires every device's `UniqueID` to be **globally
unique** and to **never change**, but the protocol enforces neither —
uniqueness has to come from how the id is generated. The driver
therefore *generates* its `mount.unique_id` rather than shipping a
hardcoded literal (it previously shipped `"skywatcher-sa-gti-001"`,
which collided across every install).

On startup, before loading its configuration, the driver:

1. **Resolves a config-file path.** The `--config <path>` argument if
   given, else the platform default — the per-user config directory on
   Unix: `~/.config/rusty-photon/star-adventurer-gti.json` on Linux
   (XDG), `~/Library/Application Support/rusty-photon/star-adventurer-gti.json`
   on macOS (via `directories::ProjectDirs`) — and the machine-wide
   `%PROGRAMDATA%\rusty-photon\star-adventurer-gti.json` on Windows
   (`ProgramData` env var, `C:\ProgramData` fallback; a Windows service
   account's per-user profile is hidden, see ADR-015). A path is
   *always* resolvable, so identity and park persistence are never
   disabled for lack of a `--config` flag. This is the same path used
   for [§Park persistence](#park-persistence).
2. **Materializes the identity.** Via
   `rusty_photon_config::materialize_identity` against the JSON pointer
   `/mount/unique_id`. If that pointer is absent, `null`, non-string, or
   an empty/whitespace string in the **file layer**, the driver mints a
   fresh UUIDv4, writes it into the file, and persists atomically
   (staged temp file → `fsync` → POSIX `rename` → parent-dir `fsync`,
   the same durability pattern as `SetPark`). The operation is
   **idempotent**: an id that already exists is never overwritten, and
   nothing is written when there was nothing to fill. On a *fresh
   install* (no file yet) the default scaffold written out is the
   serialized `Config::default()`, so the operator gets a complete,
   valid config file — minus the minted id — to edit.
3. **Loads the config** from that path (which now always exists) and
   applies the CLI overrides (`--transport`, `--port`, `--baud`,
   `--server-port`).

The materialize step operates **only on the on-disk file**, never on a
CLI-override-applied effective config, so a transient `--port` is never
baked into the persisted file. It touches only the `/mount/unique_id`
pointer; like `SetPark`, it reads the document as a `serde_json::Value`
and preserves every other field. The two writers never clobber each
other: `materialize_identity` runs once at startup and `SetPark`'s
`write_mount_fields_to_config` runs at runtime, both read-modify-write
the same file as a `Value`, each mutate only their own keys
(`mount.unique_id` vs. `mount.park_ra_ticks` / `mount.park_dec_ticks`),
and each finishes with an atomic rename — so running materialize once at
startup and park-writes later is safe. The materialize-written scaffold
always contains a `mount` object, which is exactly the object
`write_mount_fields_to_config` and the connect-time field reader require.

Operators who need to pin a specific id (e.g. migrating an existing
profile in a client that keyed on the old value) can set `unique_id`
explicitly in the config file; the never-overwrite rule means a
hand-set id is preserved verbatim.

### Unpark from AP position

#### The position-assumption problem

The Sky-Watcher firmware resets its encoder counter to `(0, 0)` on
every power-up. Raw `0` is a coordinate-frame value, not a
physical-position value — which celestial point it corresponds to is
purely a function of where the operator physically placed the OTA
before powering on. The driver therefore *always* relies on an
operator-supplied position assumption to anchor the encoder math.

Every install declares that assumption explicitly via the required
`mount.unpark_from_ap_position` config field. The field carries one
of the named AP-park strings (`ap_park_0` through `ap_park_5`) and
has no default in the schema sense — but the **ship default** is
`ap_park_0`, which encodes the safest possible assumption ("I don't
know where the OTA is; I will plate-solve and sync"). Operators who
run a permanent observatory at a specific physical park override the
default to the matching `ap_park_N` and the driver seeds the firmware
encoder accordingly on fresh-power-up connect.

#### The named poses

| `unpark_from_ap_position` | Semantics | AP description | Mech. pier | N hem (mech_HA, dec_enc) | S hem (mech_HA, dec_enc) |
|---|---|---|---|---|---|
| `ap_park_0` (default) | **Current position.** No seeding — trust the firmware encoder as-is. Operator's responsibility is to plate-solve and `SyncToCoordinates` before any blind-pointing slew. Safe default for unknown / variable physical setups. | — | — | — | — |
| `ap_park_1` | Fresh-power-up seed to this pose. | OTA on west, level, facing polar-side horizon. Celestial Dec = `±(90 − \|lat\|)`. | West | `(0 h, +(90 + \|lat\|)°)` (saddle west, dec past pole) | `(0 h, −(90 + \|lat\|)°)` (saddle west, dec past pole) |
| `ap_park_2` | Fresh-power-up seed to this pose. | OTA level facing east horizon, counterweight straight down. Hemisphere-independent celestial coords `(HA=−6, Dec=0)`. | — (CW down) | `(−6 h, 0°)` | `(−6 h, 0°)` |
| `ap_park_3` | Fresh-power-up seed to this pose. | OTA along polar axis at the visible celestial pole. Sky-Watcher's stock power-up pose. | — (CW along polar) | `(−6 h, +90°)` | `(−6 h, −90°)` |
| `ap_park_4` | Fresh-power-up seed to this pose. | OTA on east, level, facing anti-polar horizon. Celestial Dec = `∓(90 − \|lat\|)` (sign anti-hemisphere). | East | `(−12 h, −(90 + \|lat\|)°)` (saddle east, dec past pole) | `(−12 h, +(90 + \|lat\|)°)` (saddle east, dec past pole) |
| `ap_park_5` | Fresh-power-up seed to this pose. | OTA on east, level, facing polar-side horizon. Celestial Dec = `±(90 − \|lat\|)` (sign matches hemisphere). APCC-only in AP's own software. | East | `(−12 h, +(90 − \|lat\|)°)` (saddle east, dec normal) | `(−12 h, −(90 − \|lat\|)°)` (saddle east, dec normal) |

The named poses (1–5) follow the Astro-Physics
["Park Positions Defined"](https://astro-physics.info/tech_support/mounts/park-positions-defined.pdf)
document. Each AP pose describes a fixed mechanical pier side
(OTA-on-west-of-mount or OTA-on-east-of-mount) that's the same for
both hemispheres — but the driver's natural-vs-flipped pier convention
flips between hemispheres (N natural = pierWest, S natural = pierEast,
see `pre_flip_side` in `mount_device.rs`). So a pose like Park 1
(OTA-on-west-of-mount, mechanically) is on the **natural** side in the
Northern Hemisphere and on the **flipped** side in the Southern
Hemisphere — and its dec-encoder representation differs accordingly
(natural side gives `|dec_enc| ≤ 90°`; flipped side gives `|dec_enc| > 90°`
via the past-pole encoding).

The codebase's `mech_HA` is hemisphere-independent: the saddle east/west
position is determined by the encoder alone (`|mech_HA| ≤ 6 h` →
saddle west; `|mech_HA| > 6 h` → saddle east). The dec encoder *is*
hemisphere-dependent — at fixed `mech_HA`, the dec=0 reference points
at different celestial coords for N vs S, so the rotation needed to
reach the AP-described OTA pointing differs in magnitude.

The encoder values above were corrected via hardware verification at
lat 32.7°N on 2026-05-17. The earlier mapping (commit `2725a2f`) had
Park 1 and Park 5 swapped: it claimed Park 1 N was at
`(mech_HA=−12, dec_enc=+(90−|lat|)°)`, but that encoder pose
physically corresponds to AP Park 5 N (saddle east). The corrected
mapping places each AP pose at the encoder values that physically
match it.

Note on `SideOfPier` reporting: the codebase's `side_of_pier` uses
the ASCOM-spec convention (dec-past-pole indicates `pierEast`),
*not* the AP "OTA east of mount" geometric convention. AP Park 5 N
has saddle east physically but dec encoder normal (`+57°`), so the
codebase reports it as `pierWest` (pre-flip). AP Park 1 N has
saddle west but dec past pole (`+123°`), so the codebase reports
it as `pierEast` (post-flip). The encoder values match the AP
physical pose; only the `pierSide` label differs in convention.

#### Fresh-power-up auto-seed

`seed_after_connect` (renamed from `seed_home_pose_after_connect`)
fires on every connect when the configured
`unpark_from_ap_position` is one of `ap_park_1..ap_park_5` AND the
firmware encoder reading is within `FRESH_POWER_UP_TICK_TOLERANCE`
(currently 10 ticks, ~4″) of `(0, 0)`. When the configured value is
`ap_park_0` the seed is unconditionally skipped — the operator has
asserted that they will ground-truth the position themselves and the
driver does not touch the encoder.

The tolerance exists because the Sky-Watcher firmware does not always
read exactly `(0, 0)` after a power-cycle — empirically the
validation GTi reports `dec = −1` on connect, a single-tick
initialisation artifact (~0.4″) that obviously still represents the
just-powered-up state. Any genuine post-slew encoder is tens of
thousands of ticks away from zero, so a tight tolerance is enough
to absorb the firmware artifact without ever masking a real
mid-session position. Reconnecting mid-session after a slew is
therefore still safe — the seed skips on non-fresh encoders and
the live session state is preserved.

`seed_after_connect` emits two `info!()`-level log lines per connect
(when the configured pose is one of the named parks): a `pre-seed
encoder snapshot at connect` line with the firmware's pre-seed `(ra,
dec)` ticks, and a `seeded firmware encoder for unpark_from_ap_position`
line with the post-seed values. Operators expect the pre-seed pair
to read approximately `(0, 0)` on a fresh power-up; any larger
reading is a signal that either the previous slew's state survived
the connection state machine or the build is running against a mock
transport rather than the real wire.

Documented operator assumption: when `unpark_from_ap_position` is
set to one of `ap_park_1..ap_park_5`, the operator powers up the
mount **at** that configured pose and connects the driver before
any slew or sync.

#### Custom Actions for runtime control

Three driver-specific Actions (ASCOM `Action(name, parameters)`)
expose runtime control over the position assumption and the
preferred-park target. All three are advertised via `SupportedActions`.

| Action name | Parameters | Behaviour |
|---|---|---|
| `SetUnparkFromApPosition` | `park` (`ap_park_0..ap_park_5`) | Validates the park name, writes the value into the running config file (atomic-rename pattern, mirrors `SetPark` persistence — see [§Park persistence](#park-persistence)), updates the in-memory config. The new value takes effect on the *next* fresh-power-up auto-seed; the current session's encoder is not touched. Refuses (`INVALID_OPERATION`) when no config path is available — at the library layer that means `MountDevice::new`; the binary always resolves one, so this refusal no longer fires in practice. |
| `SetPreferredApPark` | `park` (`ap_park_1..ap_park_5`) | Sets the AP-park target that `Park()` will slew to. Persisted to config alongside the same file-write pattern. `ap_park_0` is not a valid value here — "current position" is not a slew target. The legacy `park_ra_ticks` / `park_dec_ticks` config keys remain as raw-encoder overrides for ops who pinned a specific tick pair; when both forms are set, the explicit tick pair wins. |
| `UnparkFromApPosition` | `park` (`ap_park_0..ap_park_5`) | Recovery operation. For `ap_park_0`, semantically equivalent to standard `Unpark()` — clears `AtPark`, no encoder change. For any named park (`ap_park_1..ap_park_5`), runs the [`ResetMountEncoders` sequence](#resetmountencoders-sequence) to safely write the park's encoder values *regardless of the current encoder state*, then clears `AtPark`. Operator is asserting "the OTA is physically at this park"; the driver makes firmware state match. |

Standard ASCOM `Unpark()` is unchanged — it always just clears the
`AtPark` flag with no encoder write. The two operations
(`Unpark()` and `UnparkFromApPosition(ap_park_0)`) are structurally
equivalent; the custom Action exists to express the recovery flow
for the named-park cases.

#### `ResetMountEncoders` sequence

Wraps the bare `:E1` / `:E2` writes in a safe-stop-then-seed envelope
so the operation works correctly regardless of in-flight firmware
state (pending `:G` goto, active `:I` tracking, latched `:M`
brakes):

```
:K1                          stop RA  (existing stop_axis_and_wait)
:K2                          stop Dec
poll :f1, :f2 until idle
:E1 <ra_ticks_for_park>      write seed encoder
:E2 <dec_ticks_for_park>     write seed encoder
clear driver-internal slew / target / tracking-flag state
```

Invoked by:

- `UnparkFromApPosition(ap_park_N)` for `N ≥ 1` (operator-confirmed
  physical position).
- Auto-seed path on fresh-power-up connect (the stop steps are no-ops
  in that state; the encoder writes are the only meaningful work).

Not invoked by the standard `Unpark()` flow — that path is for
"resume from where I parked," and writing the encoder there would
silently destroy session state.

#### Recovery procedures

What to do after various crash / disconnect scenarios:

| Scenario | Procedure |
|---|---|
| Driver crashed, mount still powered, OTA points at solvable sky | Reconnect (no power cycle) → `UnparkFromApPosition(ap_park_0)` (or standard `Unpark()`) → plate-solve current view → `SyncToCoordinates(plate_solved_coords)`. Encoder counter is intact from the session; sync writes the offset to match celestial truth. |
| Driver crashed, mount still powered, OTA points at unsolvable sky (below horizon, clouded out) | Reconnect → `UnparkFromApPosition(ap_park_N)` where `N` is the AP park you've physically positioned the OTA at. The `ResetMountEncoders` sequence stops in-flight motion and writes the park's encoder values; subsequent slews start from a known frame. Alternatively, power-cycle the mount and let the fresh-power-up auto-seed run from the persisted `unpark_from_ap_position` config value. |
| Mount lost power (planned shutdown or otherwise) | Before the next power-on, physically position the OTA at whatever `unpark_from_ap_position` is configured for (or update the config / call `SetUnparkFromApPosition` first). On power-on connect, the fresh-power-up auto-seed writes the encoder to that park's values. |

The only genuinely dangerous pattern is "crash + power-cycle without
re-parking AND with a non-`ap_park_0` `unpark_from_ap_position`
configured" — the auto-seed will fire against fresh-power-up encoder
and the driver will believe the OTA is at the configured park when
it's actually wherever the crash left it. The operator-discipline
mitigation is "if you power-cycle after a crash, physically re-park
first." A heavier mitigation (refusing `Unpark()` after fresh-seed
without an explicit operator confirmation gate) is deferred — opt-in
behind a future config flag if hardware sessions show it's needed.

### CLI arguments

| Argument | Description |
|---|---|
| `-c, --config <PATH>` | Path to JSON config file |
| `--transport <usb\|udp>` | Override `transport.kind` |
| `--port <DEVICE_OR_HOST>` | Override `transport.port` (USB) or `transport.address` (UDP) |
| `--baud <RATE>` | Override `transport.baud_rate` (USB only) |
| `--server-port <PORT>` | Override `server.port` |
| `-l, --log-level <LEVEL>` | `trace` / `debug` / `info` / `warn` / `error` |
| `--service` | Hidden: run as a Windows service (passed by the Windows service control manager; no-op on other platforms) |

### Config actions

The mount exposes its configuration over HTTP as the vendor ASCOM actions
`config.get` / `config.apply` / `config.schema` — the cross-driver protocol in
[`config-actions.md`](config-actions.md) — *alongside* its `SetPark` /
`*ApPark*` vendor actions (the `action()` dispatch tries the `ApParkAction` set
first, then falls through to the config actions). `config_actions.rs` supplies
the driver-specific half (`ConfigurableDriver for StarAdvDriver`).

- **Secret redacted / carried forward:** `/server/auth/password_hash`.
- **Locked (identity) field:** `mount.unique_id`.
- **Hard read-only fields:** the whole `transport` block (`transport.kind`,
  `.port`, `.address`, `.baud_rate`, `.command_timeout`, `.polling_interval` —
  the `usb`/`udp` enum is best edited in the config file), `server.port`, and
  `mount.enabled`.
- **Overrides:** `Overrides = ()`. The CLI overrides (`--transport`/`--port`/
  `--baud`/`--server-port`) all target read-only fields, so there is nothing to
  override-pin; the read-only tier is the practical equivalent.

The mount's parse-don't-validate config types self-validate at deserialize, so a
malformed submission fails with `INVALID_VALUE` before `validate` runs; the only
`status:"invalid"` path is an empty `mount.unique_id`. A `config.apply` that
changes a field persists atomically, returns `status:"applying"`, and fires the
in-process reload: `main.rs` runs under
`ServiceRunner::with_reload().run_with_reload(...)`, whose loop re-reads + re-
applies the CLI overrides and rebuilds the server from the freshly-persisted file.

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
    mod.rs               — module entry point for the per-transport factories
    serial.rs            — SerialTransportFactory: tokio-serial → SerialFrameTransport
    udp.rs               — UdpTransportFactory: tokio UdpSocket → UdpFrameTransport
                           (with bind-IP enforcement for AP-mode reachability)
    mock.rs              — feature("mock") MockTransportFactory +
                           CapturingMockFactory wrapping an in-memory state machine
  codec.rs               — SkywatcherCodec: implements the shared crate's `Codec`
                           trait; `Response = Vec<u8>` (raw frame), typed decode
                           lives in `MountManager::send` so it has the originating
                           Command in scope
  manager.rs             — MountManager: wraps Arc<SharedTransport<SkywatcherCodec>>;
                           owns parameter cache (CPR, TMR_Freq, hsr per axis)
                           and snapshot. Handshake + poll loop +
                           on-last-disconnect / shutdown teardown live
                           in `Hooks` closures.
  coordinates.rs         — encoder-tick ↔ angle, LST, sync offset,
                           side-of-pier derivation
  mount_device.rs        — module entry point: `DriverState`,
                           `MountDevice` struct + constructors,
                           `pre_flip_side_for_latitude` helper, public
                           re-exports of the park-persistence helpers
  mount_device/
    device.rs            — `impl Device for MountDevice` (connect /
                           description / driver info, plus
                           `SupportedActions` + `Action` dispatch)
    actions.rs           — the three driver-specific ASCOM `Action`
                           handlers (`SetUnparkFromApPosition`,
                           `SetPreferredApPark`, `UnparkFromApPosition`)
                           that `device.rs`'s `action` dispatches to
    telescope.rs         — `impl Telescope for MountDevice` (the ASCOM
                           surface: capability flags, coordinate reads,
                           slew / sync / park / side-of-pier / pulse-guide)
    inherent.rs          — inherent methods on `MountDevice` shared
                           between the trait impls: validation,
                           preconditions, motion-control wrappers,
                           post-connect lifecycle hooks
                           (`seed_after_connect`, `reset_mount_encoders`,
                            `load_park_target_after_connect`),
                           the slew planner
                           (`execute_slew_with_explicit_side`),
                           plus the `validate_guide_rate` helper
    slew.rs              — wire-level slew helpers (`:K`/`:G`/`:I`/
                           `:H`/`:M`/`:J` sequence, decelerate-and-wait)
                           and flip-aware delta-routing geometry
                           (CW-exclusion-zone path checks,
                            below-horizon-pole avoidance)
    watchers.rs          — tokio tasks observing slew / park /
                           pulse-guide completion in the background:
                           EQMOD pickup loop, post-slew tracking
                           restore, settle delay, retrying snapshot
                           poller
    park_persistence.rs  — JSON config-file read/write for `SetPark`
                           (read-as-`Value` + atomic-rename pattern)
                           and the boot-time writability probe
    tests.rs             — `#[cfg(test, feature = "mock")]` unit tests
                           for `MountDevice` and the private helpers
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
| `conformu_integration.rs` (gated on `conformu`) | ASCOM Telescope compliance via `ConformUTestBuilder::run()` — runs both `alpacaprotocol` and `conformance` phases. **Currently NOT wired into the nightly `conformu` workflow** (issue #201): three independent conformance-phase failures need driver work first. See [§"Running ConformU manually"](#running-conformu-manually) and [§"Expected ConformU report"](#expected-conformu-report). |

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
workflow rotation (issue #201). `ConformUTestBuilder::run()` (which
the in-tree integration test uses) runs `alpacaprotocol` then
`conformance`. With PulseGuide landed (PR #206), the
`alpacaprotocol` phase now completes — but the `conformance` phase
surfaces three independent failures that need driver work before
re-adding `[package.metadata.conformu]` to the package's
`Cargo.toml`:

1. **`SideOfPierTests` aborts CheckMethods.** ConformU
   (`TelescopeTester.cs::SopPierTest`) slews to mechanical-HA
   ±9 h to verify pier-side reporting on both sides of the
   meridian. The `[-6.95, +6.95]` h safety envelope correctly
   rejects those slews on real hardware, but the
   `InvalidValueException` is caught by ConformU's
   "Exception when testing device" handler at CheckMethods scope
   and the rest of the suite is abandoned — so the CI test exits
   with one ISSUE and no further diagnostics. Widening the
   envelope just for the mock test config (e.g.
   `ra_min_hours = -12`, `ra_max_hours = 12`) lets CheckMethods
   complete and exposes the other two failures below.
2. **`SideOfPier` returns `pierWest` for every in-envelope
   target.** `coordinates::side_of_pier` keys on
   `|dec_ticks| > cpr_dec/4` (the Dec-encoder-past-pole
   "physical flip" convention). Inside the safety envelope the
   Dec encoder never crosses that threshold, so the driver
   reports `West` regardless of which side of the meridian the
   mount is observing. ConformU asserts the ASCOM pointing-state
   convention (`pierWest` for HA ∈ [-6, 0), `pierEast` for
   HA ∈ (0, +6]) and records ISSUEs for every HA > 0 case in
   both `SideofPier` and `DestinationSideofPier`.
3. **PulseGuide rate-shifted tracking produces the wrong
   motion on every axis.** ConformU's PulseGuide tolerance
   check expects `guide_rate × sidereal × duration` of motion
   on the pulsed axis and ~0 on the other. The driver fails
   the check in all four directions across HA ±3 and ±9
   (24 ISSUEs total): Dec North/South moves at ~2× the
   configured rate (71.4″ vs 37.6″ expected for a 5 s pulse at
   the default 0.5× sidereal rate); RA East slows by only ~10 %
   instead of the configured 50 % (Δ RA 0.48 s vs 2.51 s
   expected); and RA West pulses move east (Δ RA +0.48 s where
   ConformU expects −2.51 s). The implementation is the
   rate-shifted tracking burst PR #206 introduced, not a
   Dec-vs-encoder coordinate issue.

To reproduce locally, run the in-tree integration test — same
binary, same config, same ConformU invocation the workflow used:

```bash
bazel test //services/star-adventurer-gti:conformu_integration
```

The test config (`tests/conformu_integration.rs`) sets
`site_latitude_deg = 47.6062` so ConformU's
`SIDE_OF_PIER_INVALID_LATITUDE = 10°` gate does not skip the
side-of-pier model tests.

### Expected ConformU report

These are the conformance-phase findings against the current
driver (PR #206). They are *not* a green run — fixing (2) and (3)
above is on the roadmap before the package is re-added to the
nightly workflow.

After (1) is worked around by widening the mock test envelope:

- **0 errors** — anything here is a real driver regression.
- **~58 issues**, dominated by:
  - PulseGuide tolerance failures at HA ±3 and ±9 in all four
    directions (24 entries) — driver bug (3). Each pulsed-axis
    "Moved {N,S,E,W} but outside test tolerance" row counts
    once, and each accompanying "{East-West,North-South}
    movement was outside test tolerance" row (the axis the
    pulse is *not* supposed to move) counts once.
  - `SideofPier` and `DestinationSideofPier`
    "`pierWest` is returned when the mount is observing at an
    hour angle between 0.0 and +6.0" (8 entries) — driver
    bug (2). ConformU's `SideofPier` test assumes a
    flip-at-meridian GEM (EQMOD / Sky-Watcher Synscan /
    ASCOM-driver-pattern behaviour) and expects `pierEast`
    for any target west of the meridian; the driver returns
    `pierWest` everywhere because the safety envelope keeps
    the Dec encoder within `[-CPR/4, +CPR/4]` and
    `coordinates::side_of_pier` only flips when that bound is
    crossed.
  - `SideofPier` / `DestinationSideofPier`
    "reports physical pier side rather than pointing state"
    and "Same value … on both sides of the meridian"
    (4 entries) — same root cause as bug (2): a non-flipping
    mount lands every in-envelope target in the same
    pointing state.

Historical baselines (`alpacaprotocol`-only or partial
`conformance` runs):

- Pre-#202 baseline (HA-meridian split, `DestinationSideOfPier`
  unimplemented): 9 issues — `DestinationSideOfPier` ×1
  (NotImplemented), `SOPPierTest` ×4 (inherited from
  NotImplemented), `SOPPierTest` ×2 (safety envelope),
  `TrackingRate Write` ×2 (upstream `ascom-alpaca-rs`
  framework bug — Alpaca enum rejection at the axum/serde
  layer before reaching the driver).
- Post-#202 baseline (Dec-encoder split, `DestinationSideOfPier`
  implemented, PulseGuide not yet landed, conformance never
  reached SideOfPierTests because alpacaprotocol failed on
  `IsPulseGuiding`): 7 issues as previously documented.

## Connection Lifecycle

The transport's lifetime is tied to the **service** lifetime, not to ASCOM
`Connected`. The driver runs `rusty-photon-shared-transport` in its
`ServiceLifetime` mode: the port opens eagerly at startup (so a wrong-device
or unreachable mount fails the process *before* it advertises on the
network) and stays open — kept alive across transient drops by the reconnect
supervisor — until the service shuts down. ASCOM `Connected = true` / `false`
only acquires / releases a session on the already-open transport. See
[`docs/plans/archive/eager-hardware-validation.md`](../plans/archive/eager-hardware-validation.md)
for the rationale.

```
Service start (`ServerBuilder::build()` → `SharedTransport::start()`,
               before the HTTP listener binds)
   ↓
open transport (serial: tokio-serial open + raw mode;
                UDP: bind to config.bind_address, set timeout)
   ↓
init handshake:
  :e1               (motor board version)  → identity gate +
                                              mount-type whitelist
                                              (issue #254);
                                              wrong-device handshake
                                              stops here → build() errors
                                              and the process exits
                                              non-zero before binding
                                              the listener.
  :F1, :F2          (initialize axes)
  :a1, :a2          (CPR per axis)         → cache
  :b1               (TMR_Freq)             → cache
  :g1, :g2          (high-speed ratio)     → cache
  :j1, :j2          (initial positions)    → cache + snapshot
   ↓
start background polling task (interval = config.polling_interval)
```

```
Connected = true   → acquire a session (refcount bump; the transport is
                     already open and polling — no wire handshake here),
                     then the post-acquire hooks run in order:
  1. seed_after_connect — if unpark_from_ap_position ∈ ap_park_1..ap_park_5
       AND the firmware encoder is within FRESH_POWER_UP_TICK_TOLERANCE of
       (0, 0):  :E1, :E2  (seed encoder to the AP pose's codebase convention)
  2. load_park_target_after_connect — resolve the in-memory park ticks from
       config / preferred_ap_park

Connected = false  → release the session. On the last client disconnect the
                     on_last_disconnect hook runs :L1, :L2, :K1 (safety
                     stop); the transport stays open, background polling
                     continues, and the parameter cache is retained.
```

```
Service shutdown (HTTP server stops → `SharedTransport::shutdown()`)
   ↓
shutdown hook runs :L1, :L2, :K1 one last time
   ↓
cancel the reconnect supervisor + the polling task
   ↓
close transport
   ↓
clear parameter cache
```

Multiple Alpaca clients share the one open transport (refcount); a client
disconnect no longer closes the port — that happens only at service
shutdown. A transient transport drop is handled by the reconnect supervisor,
which re-runs the handshake against the new connection while live sessions
survive via the connection-cell swap. (Same service-lifetime pattern as
`qhy-focuser`, `ppba-driver`, `pa-falcon-rotator`, and `dsd-fp2`.)

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
| **Phase A7 — PulseGuide** | landed (issue #206) — implements `PulseGuide` as a rate-shifted tracking burst on the targeted axis (no `:P`; that's the ST4-jack rate setter, not a pulse trigger), flips `CanPulseGuide` and `CanSetGuideRates` to `true`. Re-enabled `[package.metadata.conformu]` so the full two-phase ConformU integration ran again — but the `conformance` phase, which `alpacaprotocol`-only manual runs hadn't exercised, surfaced three failures that PR #206's review hadn't caught; see Phase A8. |
| **Phase A8 — Nightly ConformU opt-out (#201)** | landed (issue #201) — removed `[package.metadata.conformu]` again. ConformU's `conformance` phase fails for three independent reasons that need driver work first: (a) `SideOfPierTests` slews to mech-HA ±9 h, which the safety envelope correctly rejects on hardware but which ConformU treats as a fatal CheckMethods-level exception that abandons the rest of the suite; (b) `SideOfPier` always returns `pierWest` for in-envelope targets (Dec-encoder convention) where ConformU asserts the ASCOM pointing-state convention; (c) PulseGuide Dec moves at full sidereal rate instead of `guide_rate_dec_fraction × sidereal`. See [§Running ConformU manually](#running-conformu-manually) and [§Expected ConformU report](#expected-conformu-report) for the failure details and reproduction steps. |
| **Phase 5 — user-defined `SetPark` + persistence** | landed (issue #203) — park target now sourced from `mount.park_ra_ticks` / `mount.park_dec_ticks` in the config (fallback: encoder positions captured at handshake), `SetPark` writes the current encoder pair back into the running config file via atomic rename, `CanSetPark` flips on when `--config` is provided. See [§Park lifecycle](#park-lifecycle) and [§Park persistence](#park-persistence). |
| **Phase 6 — meridian-flip support** | hardware-validated 2026-05-16 (lat 32.7°N) — adds `MountConfig::flip_policy` (`enabled` + `flip_range_hours`), the asymmetric CW exclusion zone safety envelope, CW-exclusion zone-path-aware through-wrap RA routing, visible-pole Dec routing, `SetSideOfPier`, and flip-aware `DestinationSideOfPier`. End-to-end AP Park 1–5 traversal (including the through-wrap saddle-east flip and its flip-back) ran clean; the flip-back from the saddle-east wrap caught a sign-blind heuristic in `flip_slew_ra_delta` that the path-aware check now handles. `flip_policy.enabled` still defaults `false` (operators opt in once they've replayed the validation locally). The tracking-time CW-exclusion-zone safety guard (Part 1 of issue #259) has since landed — a background watcher stops tracking before the encoder `mech_HA` drifts into the zone (see [§Tracking-time safety guard](#tracking-time-safety-guard)). Driver-planned auto-flip-during-tracking (Phase 2.5 / Part 2 of #259) remains deferred — the driver only flips on an explicit `SetSideOfPier` or a slew whose target requires the opposite side. Plan: [`docs/plans/star-adventurer-gti-meridian-flip.md`](../plans/star-adventurer-gti-meridian-flip.md). See [§Meridian flip](#meridian-flip). |
| **Phase 7 — altitude-based safety floor (#223)** | landed 2026-07-01 ("Phase 3" in the plan's local numbering) — replaces the rectangular celestial-Dec envelope (`dec_limits`) with `MountConfig::min_altitude_degrees`: slew / sync targets whose computed apparent altitude (`sin alt = sin lat · sin dec + cos lat · cos dec · cos HA`) is below the floor are rejected with `INVALID_VALUE`. Default `0.0` (geometric horizon); negative floors permit below-horizon pointing and log `info!` at startup. `Park` stays exempt (privileged-park pattern). See [§Altitude floor](#altitude-floor). |

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
     acquires a [`MountManager::pause_background_polling`]
     RAII guard at the top of its spawn. While paused, the
     watcher owns the wire: pickup wire commands (`:K :G :I :H
     :M :J`) fire without contending with `:j` / `:f` polls for
     the shared-transport command lock, and the watcher's
     [`MountManager::poll_axes_now`] drives the snapshot's
     freshness with one wire round-trip per loop iteration. The
     guard is dropped explicitly right after tracking restart,
     before the settle delay — so background polling resumes
     during settle and the snapshot reflects the actively-tracking
     encoder position by the time an Alpaca client reads
     `RightAscension` post-`Slewing`. Releasing the guard early
     vs. on watcher exit makes a measurable difference (UDP mean
     7.3″ → 5.0″ in the experiment runs). Park watcher follows
     the same pattern.

  See `docs/plans/archive/star-adventurer-gti-pickup-accuracy.md` for the
  experiment plan and the diagnostic data that drove these
  choices.
- **Mechanical safety envelope** — driving the mount into the
  counterweight-up region with ConformU's pier-flip tests stalled
  the motor against a hard stop while the encoder counter kept
  advancing (audible motor noise, OTA stationary). `MountConfig`
  carries the asymmetric mech_HA interval where the CW binds against
  the pier (`cw_exclusion_zone`) plus the apparent-altitude floor
  (`min_altitude_degrees`, which replaced the rectangular `dec_limits`
  envelope 2026-07-01) —
  both wrapped as validated newtypes per
  [ADR-006](../decisions/006-typed-physical-quantities-for-mount-pointing.md).
  `SyncToCoordinates` and
  `SlewToCoordinatesAsync` reject targets whose chosen-side
  `mech_HA` falls inside the CW exclusion zone with `INVALID_VALUE`
  before any wire motion; `Park` writes the target encoder ticks
  directly without the CW-exclusion zone check, matching INDI EQMOD's
  privileged-park pattern. Pre-Phase-6 builds used a symmetric
  natural-side window (`ra_min_hours` / `ra_max_hours`); that's
  been replaced by the asymmetric CW-exclusion zone interval because
  the actual GEM hazard is one-sided.
  Defaults: CW exclusion zone `(0.95, 11.05) h` of mech_HA — the
  one-sided arc where the CW shaft rises more than 0.95 h above
  horizontal on the GTi (the symmetric `±6.99 h` natural-side
  bound from the 2026-05-13 test is *subsumed* by the new
  asymmetric model: tracking past `+0.95 h` enters the zone and is
  refused, while reaching `−12 h` for anti-meridian poses like
  Park 1 is no longer blocked). A narrower `(6.95, 11.05)` was
  used before 2026-05-17 but missed the ascending half of the
  CW exclusion arc — see
  [§Safety envelope](#safety-envelope-cw-exclusion-zone)
  for the hardware session that triggered the widening. Altitude
  floor `0°` (geometric horizon) — see [§Altitude floor](#altitude-floor).
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
  `InterfaceVersion`, `Name`, `SupportedActions` lists the three
  driver-specific Actions — see [§Unpark from AP position](#unpark-from-ap-position))
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
- Apparent-altitude floor on slew / sync targets
  (`min_altitude_degrees` — see [§Altitude floor](#altitude-floor))
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
| Operator-confirmation gate on `Unpark()` after fresh-power-up seed | Hardware-discipline mitigation for the "crash + power-cycle without re-park" failure mode. Today's design relies on operator discipline ("if you power-cycle after a crash, physically re-park first"); a confirmation gate is the heavier alternative. Deferred behind a future config flag, to be added if hardware sessions show the discipline assumption breaks. |
| Polar-alignment helpers, TPOINT, cone error | observational pointing model is the host's concern, not the driver's |
| WiFi station mode (mount on a routed network) | AP-mode UDP is verified; station mode just changes the bind-address selection — straightforward to add once a station-mode test setup exists |
| Multi-mount support on a single binary | `rp` assumes one mount per service; multi-mount is a separate concern |
| Driver-planned auto-flip during tracking (Phase 2.5 / Part 2 of issue #259: `flip_policy.auto_flip_during_tracking` + `auto_flip_at_meridian_offset_hours`) | hosts like NINA / SGP / `rp` own flip timing themselves; mid-exposure auto-flip is a footgun for astrophotography and a separate state machine. The Part 1 tracking-time safety guard (stop-only, see [§Tracking-time safety guard](#tracking-time-safety-guard)) has landed; auto-flip would replace the stop with a flip slew and re-engage tracking on the new pier side |

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
  through Park 5) the [§Unpark from AP position](#unpark-from-ap-position) config exposes,
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
