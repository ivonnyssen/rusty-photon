# QHY Q-Focuser Service

## Overview

The `qhy-focuser` service is an ASCOM Alpaca Focuser driver for the QHY Q-Focuser (EAF - Electronic Auto Focuser). It communicates with the focuser hardware over a USB-CDC serial port using a JSON-based command/response protocol.

The Q-Focuser is a standalone USB serial device - it does **not** use the QHYCCD camera SDK. It has its own USB connection and protocol independent of any QHY camera.

## Architecture

The service follows the same architecture as `ppba-driver`:

- **Serial abstraction**: Trait-based I/O (`io.rs`, `serial.rs`) for testability
- **Serial manager**: Reference-counted shared serial connection with background polling (`serial_manager.rs`)
- **Protocol layer**: JSON command serialization and response parsing (`protocol.rs`)
- **ASCOM device**: Implements `Device` + `Focuser` traits (`focuser_device.rs`)
- **Mock mode**: Feature-gated mock serial implementation for testing without hardware (`mock.rs`)
- **Server builder**: Configures and starts the ASCOM Alpaca server (`lib.rs`)

## Protocol Reference

The Q-Focuser uses JSON objects over serial. Commands include a `cmd_id` field; responses echo the ID in an `idx` field.

| cmd_id | Command | Parameters | Response Fields | Notes |
|--------|---------|------------|-----------------|-------|
| 1 | GetVersion | — | firmware_version, board_version | |
| 2 | RelativeMove | dir, step | — | dir: 1=inward, -1=outward |
| 3 | Abort | — | — | Stops current movement |
| 4 | ReadTemperature | — | o_t, c_t (÷1000→°C), c_r (÷10→V) | |
| 5 | GetPosition | — | pos | Returns current position |
| 6 | AbsoluteMove | tar | — | Moves to absolute position |
| 7 | SetReverse | rev (0/1) | — | Reverses motor direction |
| 11 | SyncPosition | init_val | — | Sets position counter without moving |
| 13 | SetSpeed | speed (0-8) | — | 0 = fastest |
| 16 | SetHoldCurrent | ihold, irun | — | Motor current (0-31 each) |
| 19 | SetPdnMode | pdn_d | — | Motor power-down mode |

Responses are JSON objects terminated by `}` (no newline). Commands are sent as raw JSON without any terminator.

## ASCOM Focuser Mapping

| ASCOM Property/Method | Implementation |
|------------------------|----------------|
| Absolute | `true` (always) |
| IsMoving | Cached state, detected via position polling |
| MaxIncrement | From config `max_step` |
| MaxStep | From config `max_step` |
| Position | Cached from polling (i64 → i32 cast) |
| StepSize | NOT_IMPLEMENTED |
| TempComp | `false` (not supported) |
| TempCompAvailable | `false` |
| Temperature | Outer temperature from polling |
| Halt | Sends Abort command (cmd 3) |
| Move | Validates 0..max_step, sends AbsoluteMove (cmd 6) |

## Configuration

```json
{
  "serial": {
    "port": "/dev/ttyACM0",
    "baud_rate": 9600,
    "polling_interval_ms": 1000,
    "timeout_seconds": 2
  },
  "server": {
    "port": 11113
  },
  "focuser": {
    "name": "QHY Q-Focuser",
    "unique_id": "qhy-focuser-001",
    "description": "QHY Q-Focuser (EAF) Stepper Motor Controller",
    "device_number": 0,
    "enabled": true,
    "max_step": 64000,
    "speed": 0,
    "reverse": false
  }
}
```

### CLI Arguments

| Argument | Description |
|----------|-------------|
| `-c, --config` | Path to configuration file |
| `--port` | Serial port path (overrides config) |
| `--server-port` | Server port (overrides config) |
| `-l, --log-level` | Log level: trace, debug, info, warn, error |

## Module Structure

| Module | Description |
|--------|-------------|
| `config.rs` | Configuration types and loading |
| `error.rs` | Error types with ASCOM error mapping |
| `focuser_device.rs` | ASCOM Device + Focuser trait implementation |
| `io.rs` | Serial I/O trait abstractions |
| `lib.rs` | Module declarations, ServerBuilder |
| `main.rs` | CLI entry point |
| `mock.rs` | Mock serial implementation (feature-gated) |
| `protocol.rs` | JSON command/response protocol |
| `serial.rs` | tokio-serial implementation |
| `serial_manager.rs` | Shared serial connection with polling |

## Testing

- **Unit tests**: Protocol serialization, config, error types
- **Integration tests**: Device behavior with mock serial, server startup
- **ConformU**: ASCOM Alpaca compliance testing (requires ConformU installation)

```bash
# Run all tests
cargo test -p qhy-focuser --quiet

# Run with mock feature (for device tests)
cargo test -p qhy-focuser --features mock

# Run ConformU compliance tests
cargo test -p qhy-focuser --features mock --test conformu_integration -- --ignored

# Run in mock mode
cargo run -p qhy-focuser --features mock
```

## Connection Lifecycle

1. ASCOM client calls `set_connected(true)`
2. Serial manager opens port (first connection only, ref-counted)
3. Handshake: GetVersion → SetSpeed → GetPosition → ReadTemperature
4. Background polling starts: position + temperature at configured interval
5. Move detection: polling compares position to target, clears `is_moving` when reached
6. On disconnect: ref-count decremented, port closed when last device disconnects
