# Deep Sky Dad FP2 Service

## Overview

The `dsd-fp2` service is an ASCOM Alpaca **CoverCalibrator** driver for the
Deep Sky Dad **Flat Panel 2 (FP2)** — a motorised flat-field panel that
combines an electroluminescent light source with a servo-driven cover. The
device speaks a bracketed ASCII protocol over a USB-CDC serial port
(`/dev/ttyACM*` on Linux, vendor/product `2e8a:000a`).

The driver exposes the FP2 as a single ASCOM Alpaca CoverCalibrator device so
that the existing `calibrator-flats` orchestrator (which already consumes
`open_cover` / `close_cover` / `calibrator_on` / `calibrator_off` via `rp`'s
MCP tools) can drive it without any orchestrator changes.

## Architecture

The service is built on the workspace's `rusty-photon-shared-transport`
crate (PR #269), which factors out the refcounted-lifecycle scaffolding
that previously lived per-service. The crate provides:
`SharedTransport<C>` (the lifecycle/refcount core), `Codec` (typed
command/response translation), `FrameTransport` (one open conduit),
`TransportFactory` (open-me-a-transport), `Session<C>` (per-device
handle), and `Hooks` (per-service handshake / teardown / while-open
plug). See `docs/plans/shared-transport-extraction.md` for the rationale.

```
+----------------------------------------------------------------+
|  dsd-fp2 binary                                                |
|                                                                |
|  main.rs ──► lib.rs (ServerBuilder, BoundServer)               |
|                  │                                             |
|                  ▼                                             |
|        DsdFp2Device (device.rs)                                |
|             │  session: Session<Fp2Codec>                      |
|             │  set_connected → transport.acquire() / close()   |
|             ▼                                                  |
|        FlatPanelManager (manager.rs)                           |
|             ├─ cached_state (motor/cover/light/brightness/…)   |
|             └─ transport: Arc<SharedTransport<Fp2Codec>>       |
|                  │  Hooks: handshake = [GFRM] verify + seed    |
|                  │         while_open = 500 ms poll loop       |
|                  │         teardown = noop                     |
|                  ▼                                             |
|        Fp2Codec (codec.rs)  ◀── encodes Command → bytes        |
|                                  decodes (VALUE) → RawResponse |
|                  │                                             |
|                  ▼                                             |
|        Fp2SerialTransportFactory (transport.rs)                |
|             ▶ tokio_serial::SerialStream                       |
|             ▶ SerialFrameTransport (read until b')')           |
|                                                                |
|   tests:    MockTransportFactory (mock.rs, feature "mock")     |
|             ▶ MockFrameTransport with stateful sim             |
|                                                                |
|        /dev/ttyACM0  @ 115200 8N1   (FP2 hardware)             |
+----------------------------------------------------------------+
```

Module responsibilities:

- **Protocol layer** (`protocol.rs`): `Command` enum, `RawResponse` body
  parsers (`parse_ok` / `parse_int` / `parse_bool` / `parse_temperature`
  / `parse_firmware`).
- **Codec** (`codec.rs`): `Fp2Codec` implements
  `rusty_photon_shared_transport::Codec` — `encode(Command) -> Vec<u8>`,
  `decode(&[u8]) -> RawResponse`. FP2 has no unsolicited frames, so
  default `matches` (true) and `max_skip` (0) are correct.
- **Transport factory** (`transport.rs`):
  `Fp2SerialTransportFactory` opens a `tokio_serial::SerialStream` and
  wraps it in `SerialFrameTransport` with `terminator = b')'` and a
  256-byte frame-size cap.
- **Manager** (`manager.rs`): `FlatPanelManager` owns
  `Arc<SharedTransport<Fp2Codec>>` and the `Arc<RwLock<CachedState>>`
  written by the while-open poll task and the handshake hook. No
  lifecycle plumbing of its own.
- **ASCOM device** (`device.rs`): `DsdFp2Device` holds the
  manager and a per-device `Session<Fp2Codec>`. `set_connected(true)`
  calls `manager.transport().acquire().await`; `set_connected(false)`
  awaits `session.close().await` (primary teardown path,
  per the shared-transport contract). Cover and calibrator state derive
  from the cached snapshot.
- **Server builder** (`lib.rs`): Wires the device into an
  `ascom_alpaca::Server` with TLS and auth (via `rp-tls`, `rp-auth`)
  and returns a `BoundServer`.
- **Mock mode** (`mock.rs`, feature `mock`):
  `MockTransportFactory` returns in-process `MockFrameTransport`s
  driven by a shared simulator state. Used by BDD scenarios, ConformU,
  and unit tests.

What is **not** in this crate (because the shared-transport crate owns it):

- Refcounting of clients (`SharedTransport.count`).
- Request arbitration (`Connection.transport` Mutex).
- Spawning and cancelling the while-open task.
- Rollback on handshake failure (Phase A's `RollbackGuard`).
- `Session::close()`'s synchronous teardown contract and the
  `Session::drop` detached-cleanup fallback.

## Hardware Constraints

- **Connection**: USB-CDC virtual serial port. Stable path is
  `/dev/serial/by-id/usb-Deep_Sky_Dad_Deep_Sky_Dad_FP2_<serial>-if00`.
- **Cover travel**: a servo sweeping nominally 0° (open) to 270° (closed).
  Targets outside the bounds are rejected by the firmware.
- **Brightness range**: 12-bit, `0..=4096` (inclusive at both ends per the
  INDI reference driver).
- **No halt-cover support**: the FP2 protocol does not expose a
  cover-motion abort opcode; once started, a move runs to completion.
  The driver therefore returns `MethodNotImplementedException`
  (Alpaca error code 0x400) from `HaltCover` — which is what the
  [ASCOM ICoverCalibratorV2 spec][cc-spec] explicitly mandates "if cover
  movement cannot be interrupted". (See [Known limitations](#known-limitations)
  for the ConformU divergence on this point.)

  [cc-spec]: https://ascom-standards.org/newdocs/covercalibrator.html
- **Heater**: optional dew-heater channel with a thermistor. Present if
  `[GHTT]` returns a value greater than `-40` °C. The driver reads this
  for diagnostics but does not expose it as an ASCOM Switch in the MVP
  (see [Future Work](#future-work)).
- **Control modes**: the device has a local-button mode that auto-returns
  to "remote control" on the first received serial command or after a 10-minute
  idle. No driver state is needed to handle this — the panel restores
  remote mode the moment we ping it on connect.

## Protocol Reference

The FP2 uses **bracketed ASCII** at **115200 8N1**.

- **Command framing**: `[CMD...]` — opening `[`, ASCII payload, closing `]`.
  No terminator is added.
- **Response framing**: `(VALUE)` — opening `(`, ASCII payload, closing `)`.
  Read until `)`. Timeout: 3 s per request.
- **Frame ordering**: every `Session::request` and every poll-task
  request goes through the per-`Connection<Fp2Codec>` request lock in
  `rusty-photon-shared-transport` (the `Connection.transport` Mutex).
  Encode → `send_frame` → `recv_frame` → decode runs end-to-end while the
  lock is held, so foreground writes and the while-open poll cannot
  interleave bytes on the wire. The driver does not pre-drain the read
  buffer — `SerialFrameTransport` consumes one terminator-delimited
  frame per `recv_frame` call, and any leading bytes before the next
  `(` are tolerated by `RawResponse::from_frame`.

### Command Set

| Command       | Direction | Purpose                                      | Response                                  |
|---------------|-----------|----------------------------------------------|-------------------------------------------|
| `[GFRM]`      | Get       | Firmware identification                      | `(Board=DeepSkyDad.FP2, Version=X.Y.Z.W)` |
| `[GOPS]`      | Get       | FP2 cover state (binary)                     | `(0)` closed, `(1)` open, other in-between |
| `[GPOS]`      | Get       | Cover angle (FP1-style; also valid on FP2)   | `(0)` open … `(270)` closed                |
| `[GMOV]`      | Get       | Motor state                                  | `(0)` stopped, `(1)` running              |
| `[STRG<deg>]` | Set       | Set target angle (0..270)                    | `(OK)`                                    |
| `[SMOV]`      | Action    | Execute move to current target               | `(OK)`                                    |
| `[GLON]`      | Get       | Light enable state                           | `(0)` off, `(1)` on                       |
| `[SLON<0\|1>]`| Set       | Light on/off                                 | `(OK)`                                    |
| `[GLBR]`      | Get       | Brightness                                   | `(N)`  with `N` in `0..=4096`             |
| `[SLBR<NNNN>]`| Set       | Brightness (zero-padded 4 digits)            | `(OK)`                                    |
| `[GHTT]`      | Get       | Heater temperature (°C, float)               | `( 26.104242)` — leading-space tolerated  |
| `[GHTM]`      | Get       | Heater mode                                  | `(0)` off, `(1)` on, `(2)` on-when-active |
| `[SHTM<N>]`   | Set       | Heater mode (0\|1\|2)                        | `(OK)`                                    |

Commands not in the above table are unused by this driver. The FP1
variant of the protocol uses some additional commands (e.g. `[GPOS]`-only
state) that are exercised here only as a fallback / liveness ping.

### Ping / Liveness

The handshake on connect issues `[GFRM]`. A non-empty response identifying
"DeepSkyDad.FP2" succeeds; anything else fails the connection with a
clear error. The firmware string is cached for `driver_info()`.

## ASCOM CoverCalibrator Mapping

ASCOM's `CoverCalibrator` interface unifies a motorised cover and a
calibration light source. The mapping is:

### CoverState Derivation

`CoverState` is derived from cached `[GMOV]` + `[GOPS]`:

| `[GMOV]` | `[GOPS]` | `CoverState`         |
|----------|----------|-----------------------|
| `1`      | _any_    | `Moving`              |
| `0`      | `0`      | `Closed`              |
| `0`      | `1`      | `Open`                |
| `0`      | other    | `Unknown`             |
| —        | —        | `Unknown` (disconnected) |
| parse err| —        | `Error`               |

### CalibratorState Derivation

`CalibratorState` is derived from cached `[GLON]`:

| `[GLON]` | `CalibratorState` |
|----------|-------------------|
| `0`      | `Off`             |
| `1`      | `Ready`           |
| other    | `Unknown`         |
| —        | `Unknown` (disconnected) |
| parse err| `Error`           |

The FP2's EL panel reaches commanded brightness in well under a poll
interval, so we never report `NotReady`. The default
`calibrator_changing()` impl (returns `state == NotReady`) yields `false`
without any driver-specific code.

### Method Mapping

| ASCOM method                  | FP2 wire sequence                       | Notes                                                                                               |
|-------------------------------|------------------------------------------|-----------------------------------------------------------------------------------------------------|
| `cover_state()`               | _(cached)_                              | Derived from cached `[GMOV]` + `[GOPS]`                                                              |
| `calibrator_state()`          | _(cached)_                              | Derived from cached `[GLON]`                                                                         |
| `brightness()`                | _(cached)_                              | Cached `[GLBR]`                                                                                      |
| `max_brightness()`            | _(constant)_                            | `4096`                                                                                               |
| `open_cover()`                | `[STRG0]` → `(OK)` ; `[SMOV]` → `(OK)`  | Asynchronous; `cover_state` reports `Moving` until polled `[GMOV]→(0)`.                              |
| `close_cover()`               | `[STRG270]` → `(OK)` ; `[SMOV]` → `(OK)` | Same as above.                                                                                      |
| `halt_cover()`                | — (returns `MethodNotImplementedException`) | The FP2 firmware has no halt-motion opcode; per the ASCOM spec, `HaltCover` MUST throw `MethodNotImplementedException` when cover movement cannot be interrupted. |
| `calibrator_on(brightness)`   | `[SLBR<NNNN>]` → `(OK)` ; `[SLON1]` → `(OK)` | Clamp `brightness` to `0..=4096`; pad to 4 digits. Sending brightness even when already on keeps the call idempotent. |
| `calibrator_off()`            | `[SLON0]` → `(OK)`                      | Does not change brightness; subsequent `calibrator_on(brightness)` reuses the prior commanded value if any. |
| `interface_version()`         | — (default)                             | Returns `2` (ICoverCalibratorV2).                                                                    |
| `cover_moving()`              | — (default)                             | Returns `cover_state == Moving`.                                                                     |
| `calibrator_changing()`       | — (default)                             | Returns `false` (we never report `NotReady`).                                                        |

### Validation

- `calibrator_on(brightness)`: a brightness over `MaxBrightness` (4096) is
  rejected with `ASCOMError::INVALID_VALUE`. Zero is accepted and forwarded
  unchanged (the device treats `[SLBR0000]` + `[SLON1]` as "on at zero",
  which is what the spec calls for).
- All writes (open/close, calibrator on/off, brightness changes) require
  `connected == true`; otherwise the driver returns `ASCOMError::NOT_CONNECTED`.

## Connection Lifecycle

The lifecycle is the standard shared-transport flow; this section is
calibration for what specifically happens at each phase, not a
re-description of the shared-transport contract.

1. **`Connected = true`** on the ASCOM device calls
   `manager.transport().acquire().await`, which through
   `SharedTransport` increments the refcount and — on the 0→1 transition —
   calls the `TransportFactory::open()` (opens the serial port at 115200
   8N1), constructs a `Connection<Fp2Codec>`, runs the handshake hook,
   then publishes the slot and spawns the while-open task.
2. The handshake hook sends `[GFRM]`, verifies the board is `DeepSkyDad.FP2`,
   and seeds the cached state with a single poll round. Failure (open
   error, non-FP2 firmware, malformed response, IO timeout) propagates
   back through `acquire()`'s `Result` and `SharedTransport`'s
   `RollbackGuard` rolls the refcount back to zero with no slot
   published.
3. The while-open task ticks every `polling_interval` (default 500 ms),
   refreshing `[GMOV]`, `[GOPS]`, `[GLON]`, `[GLBR]`, `[GHTT]` into the
   cached state. The task uses `tokio::select!` against
   `ctx.cancelled()` so cancellation at teardown returns within one
   tick.
4. **`Connected = false`** calls `session.close().await`, which on the
   1→0 transition cancels the while-open task, awaits it (5 s timeout
   then abort), runs the teardown hook, and drops the slot's
   `Arc<Connection<C>>` (which closes the OS-level serial port).

The CoverCalibrator is the only device this server registers, so the
refcount tops out at `1` in normal use. The pattern is preserved so the
service can later expose a Switch device (heater control) without redesign.

## Configuration

```jsonc
{
  "serial": {
    "port": "/dev/ttyACM0",
    "baud_rate": 115200,
    "polling_interval": "500ms",
    "timeout": "3s"
  },
  "server": {
    "port": 11119,
    "discovery_port": 32227,
    "auth": null,
    "tls": null
  },
  "cover_calibrator": {
    "name": "Deep Sky Dad FP2",
    "unique_id": "dsd-fp2-001",
    "description": "Deep Sky Dad Flat Panel 2 (motorised flat field panel)",
    "enabled": true,
    "max_brightness": 4096
  }
}
```

`serial.port` accepts either `/dev/ttyACM0`-style paths or the more
durable `/dev/serial/by-id/...-if00` form. Prefer the `by-id` form in
production configs so that re-enumeration of other USB devices does not
shuffle the FP2 onto a different `ttyACM*`.

### CLI Arguments

| Argument | Description |
|----------|-------------|
| `-c, --config`     | Path to configuration file |
| `--port`           | Serial port path (overrides config) |
| `--server-port`    | Server port (overrides config) |
| `-l, --log-level`  | Log level: trace, debug, info, warn, error |

## Error Handling

All driver errors flow through `DsdFp2Error` (defined in `error.rs`) and
are converted to `ASCOMError` for protocol boundaries:

| Internal error               | ASCOM error code                       |
|------------------------------|-----------------------------------------|
| `NotConnected`               | `NOT_CONNECTED`                        |
| `InvalidValue(String)`       | `INVALID_VALUE`                        |
| `Timeout(_)`                 | `UNSPECIFIED` with `Operation timed out` message |
| `Communication(_)`           | `UNSPECIFIED`                          |
| `MalformedResponse(_)`       | `UNSPECIFIED`                          |
| `Io(_)`                      | `UNSPECIFIED`                          |
| `HandshakeFailed(_)`         | `NOT_CONNECTED` (connect path)        |

`debug!` is used for the per-operation breadcrumbs; only the
once-per-lifecycle messages (server bound, port opened, port closed) use
`info!` (CLAUDE.md rule 9).

## Module Structure

| Module                | Description                                                                                  |
|-----------------------|----------------------------------------------------------------------------------------------|
| `config.rs`           | `Config`, `SerialConfig`, `ServerConfig`, `CoverCalibratorConfig` + defaults + JSON load     |
| `error.rs`            | `DsdFp2Error` enum (`thiserror`) + `to_ascom_error()` + `From<TransportError>` / `From<SessionError<…>>` / `From<DsdFp2Error> for ASCOMError` |
| `protocol.rs`         | `Command` enum + `RawResponse` body parsers                                                  |
| `codec.rs`            | `Fp2Codec` — `rusty_photon_shared_transport::Codec` impl                                    |
| `transport.rs`        | `Fp2SerialTransportFactory` — opens `tokio_serial::SerialStream`, wraps in `SerialFrameTransport` |
| `manager.rs`          | `FlatPanelManager` over `SharedTransport<Fp2Codec>` + `CachedState` + Hooks (handshake / while_open / teardown) |
| `device.rs`           | `DsdFp2Device` implementing ASCOM `Device + CoverCalibrator`; holds a per-device `Session<Fp2Codec>` |
| `mock.rs`             | `MockTransportFactory` + `MockFrameTransport` (feature `mock`); shared `MockState` simulator |
| `lib.rs`              | Public exports, `ServerBuilder`, `BoundServer`                                              |
| `main.rs`             | CLI (clap) + tracing init + shutdown signal                                                 |

## MVP Scope

### In Scope

- Connect / disconnect over USB-CDC serial
- Cover open / close + state polling (`CoverState` ∈ {`Open`, `Closed`, `Moving`, `Unknown`, `Error`})
- Calibrator on / off + brightness set (`CalibratorState` ∈ {`Off`, `Ready`, `Unknown`, `Error`})
- Brightness read (cached) and `max_brightness = 4096`
- Mock factory for ConformU and BDD (consumed by both the spawned
  binary and the in-process `manager.rs` mock tests)
- Three BDD feature files: connection_lifecycle, cover_control, calibrator_control
- Unit tests for protocol parsing and state derivation

### Deferred

- **Heater control as an ASCOM Switch device.** The protocol supports
  `[GHTM]` / `[SHTM]`; a separate Switch device with three settings
  (`Off`, `On`, `OnWhenFlapOpenOrLed`) could be added in a follow-up.
- **Halt cover.** The FP2 firmware exposes no abort; users wanting this
  should bring it up with Deep Sky Dad.
- **Brightness ramp profiles.** EL panels have non-linear perceived
  brightness; a calibration LUT could be applied between the ASCOM
  0..MaxBrightness scale and the device's raw 0..4096. Out of scope until
  there is evidence the linear mapping is not good enough.
- **i18n.** The driver uses plain English log messages; the workspace
  i18n facility (`rusty-photon-i18n`) is only used by services that need
  localised CLI help (e.g. `ppba-driver`).

## Testing Strategy

Follows `docs/skills/testing.md`.

### BDD Tests (Cucumber)

Three feature files cover the MVP behaviour:

- `connection_lifecycle.feature` — connect, disconnect, status after
  disconnect.
- `cover_control.feature` — open, close, state transitions, errors when
  disconnected.
- `calibrator_control.feature` — turn on at brightness, turn off,
  reject out-of-range brightness, state after disconnect.

Scenarios spawn the dsd-fp2 binary (built with `--features mock` so
`MockTransportFactory` is wired in place of the real serial transport)
via [`bdd_infra::ServiceHandle`] and drive it through the typed ASCOM
Alpaca `CoverCalibrator` client — same pattern as `ppba-driver` and
`qhy-focuser`. The `World` writes a temp config (`/dev/mock` port, OS-
assigned server port, 100 ms polling interval) and the `bdd_infra::
bdd_main!` macro handles the entry-point + Bazel-runfiles chdir.

Scenarios that need the simulator in a non-default state (e.g. cover
open before exercising `close_cover`) prime it through the client
itself rather than poking `MockState` directly — the spawned binary
gives tests no in-process handle. Scenarios that previously required
bespoke simulator state (e.g. the non-FP2 firmware handshake
rejection) are covered by the in-process unit test
`manager::mock_tests::handshake_rejects_non_fp2_firmware`.

### Unit Tests

- `protocol.rs`: encode/decode every command and response variant;
  malformed input handling (missing `)`, junk before `(`, empty body).
- `device.rs`: `derive_cover_state` / `derive_calibrator_state`
  state-derivation tables (every cell in the CoverState/CalibratorState
  tables above).
- `manager.rs`: brightness validation; the shared-transport crate's
  own integration suite covers refcount + handshake-rollback edge cases.
- `error.rs`: `to_ascom_error()` round-trips per the table in
  [Error Handling](#error-handling); `From<TransportError>` /
  `From<SessionError<DsdFp2Error>>` flattening (centralised here so
  `.map_err(DsdFp2Error::from)?` works at every call site);
  `From<DsdFp2Error> for ASCOMError` so `?` chains land in
  `ASCOMResult<_>` without explicit wrappers.

### ConformU

The service exposes a `conformu = ["mock", "ascom-alpaca/test"]` feature.
`tests/conformu_integration.rs` launches the binary against a mock port
and points ConformU at it. The same approach `qhy-focuser` uses.

## Known Limitations

### ConformU 4.3 flags spec-compliant `HaltCover` as an issue

The driver returns `MethodNotImplementedException` from `HaltCover`,
exactly as the [ASCOM ICoverCalibratorV2 spec][cc-spec] mandates "if
cover movement cannot be interrupted". The FP2 firmware exposes no
halt-motion opcode, so this is the spec-correct response.

ConformU 4.3's `CoverCalibratorTester.TestHaltCover` (file
`ConformU/Conform/CoverCalibratorTester.cs`) flags this as an issue
anyway, because in its async-cover branch every exception type — including
the spec-mandated `MethodNotImplementedException` — is treated as
`Required.MustBeImplemented`. A ConformU run against this driver
reports:

> HaltCover  ISSUE  CoverStatus indicates that the device has cover capability and a NotImplementedException error was returned, this method must function per the ASCOM specification.

The driver is intentionally not changed to silence this — the upstream
divergence is filed at
<https://github.com/ASCOMInitiative/ConformU/issues/30>. For that
reason this service does **not** declare a `[package.metadata.conformu]`
entry, so the rolling ConformU workflow does not include it. Re-enable
once the upstream issue lands and the test passes cleanly.

[cc-spec]: https://ascom-standards.org/newdocs/covercalibrator.html

## Future Work

- Switch device for heater control (`[SHTM]`).
- Per-filter brightness profiles surfaced to `calibrator-flats` (the
  orchestrator already supports a single `brightness` knob; per-filter
  needs orchestrator changes, not driver changes).
- Optional integration with `sentinel` to surface the heater temperature
  in the dashboard.

## References

- Vendor manual: [DSD-FP2-MANUAL-V3](https://shop.deepskydad.com/software-and-documentation/)
- INDI reference driver: `indilib/indi/drivers/auxiliary/deepskydad_fp.cpp`
- ASCOM CoverCalibrator interface: `ascom-alpaca` crate (pinned commit
  `638429b` on branch `integration` in workspace `Cargo.toml`)
- Sibling services with the same architecture:
  [`qhy-focuser`](qhy-focuser.md), [`ppba-driver`](ppba-driver.md)
- Consumer of this driver:
  [`calibrator-flats`](calibrator-flats.md)
