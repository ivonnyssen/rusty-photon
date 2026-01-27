# PPBA Switch Driver

ASCOM Alpaca Switch driver for the Pegasus Astro Pocket Powerbox Advance Gen2 (PPBA).

## Overview

This service exposes the PPBA device as an ASCOM Alpaca Switch device, allowing control of power outputs, dew heaters, and monitoring of device sensors through the standard ASCOM Switch interface.

## Device Protocol

The PPBA communicates via serial at 9600 baud, 8N1, with newline-terminated commands.

### Commands

| Command | Description | Response |
|---------|-------------|----------|
| `P#` | Ping/status check | `PPBA_OK` |
| `PV` | Firmware version | `n.n.n` |
| `PA` | Full status | `PPBA:voltage:current:temp:humidity:dewpoint:quad:adj:dewA:dewB:autodew:warn:pwradj` |
| `PS` | Power statistics | `PS:averageAmps:ampHours:wattHours:uptime_ms` |
| `P1:b` | Set quad 12V output (0/1) | `P1:b` |
| `P2:n` | Set adjustable output (0/1) | `P2:n` |
| `P3:nnn` | Set DewA PWM (0-255) | `P3:nnn` |
| `P4:nnn` | Set DewB PWM (0-255) | `P4:nnn` |
| `PU:b` | USB2 hub control (0/1) | `PU:b` |
| `PD:b` | Auto-dew enable (0/1) | `PD:b` |

## Switch Mapping

### Controllable Switches (CanWrite = true)

| ID | Name | Type | Min | Max | Step | Command |
|----|------|------|-----|-----|------|---------|
| 0 | Quad 12V Output | Boolean | 0 | 1 | 1 | `P1:b` |
| 1 | Adjustable Output | Boolean | 0 | 1 | 1 | `P2:b` |
| 2 | Dew Heater A | PWM | 0 | 255 | 1 | `P3:nnn` |
| 3 | Dew Heater B | PWM | 0 | 255 | 1 | `P4:nnn` |
| 4 | USB Hub | Boolean | 0 | 1 | 1 | `PU:b` |
| 5 | Auto-Dew | Boolean | 0 | 1 | 1 | `PD:b` |

### Read-Only Switches - Power Statistics (CanWrite = false)

| ID | Name | Type | Min | Max | Step | Source |
|----|------|------|-----|-----|------|--------|
| 6 | Average Current | Amps | 0 | 20 | 0.01 | `PS` command |
| 7 | Amp Hours | Ah | 0 | 9999 | 0.01 | `PS` command |
| 8 | Watt Hours | Wh | 0 | 99999 | 0.1 | `PS` command |
| 9 | Uptime | Hours | 0 | 99999 | 0.01 | `PS` command |

### Read-Only Switches - Sensor Data (CanWrite = false)

| ID | Name | Type | Min | Max | Step | Source |
|----|------|------|-----|-----|------|--------|
| 10 | Input Voltage | Volts | 0 | 15 | 0.1 | `PA` command |
| 11 | Total Current | Amps | 0 | 20 | 0.01 | `PA` command |
| 12 | Temperature | °C | -40 | 60 | 0.1 | `PA` command |
| 13 | Humidity | % | 0 | 100 | 1 | `PA` command |
| 14 | Dewpoint | °C | -40 | 60 | 0.1 | `PA` command |
| 15 | Power Warning | Boolean | 0 | 1 | 1 | `PA` command |

**Total: 16 switches** (MaxSwitch = 16)

## Configuration

Configuration is provided via a JSON file:

```json
{
  "device": {
    "name": "Pegasus PPBA",
    "unique_id": "ppba-switch-001",
    "description": "Pegasus Astro Pocket Powerbox Advance Gen2"
  },
  "serial": {
    "port": "/dev/ttyUSB0",
    "baud_rate": 9600,
    "polling_interval_ms": 5000,
    "timeout_seconds": 2
  },
  "server": {
    "port": 11112,
    "device_number": 0
  }
}
```

### Configuration Options

| Section | Field | Description | Default |
|---------|-------|-------------|---------|
| device | name | Device name for ASCOM | "Pegasus PPBA" |
| device | unique_id | Unique identifier | "ppba-switch-001" |
| device | description | Device description | (see above) |
| serial | port | Serial port path | "/dev/ttyUSB0" |
| serial | baud_rate | Baud rate | 9600 |
| serial | polling_interval_ms | Status poll interval (milliseconds) | 5000 |
| serial | timeout_seconds | Serial timeout | 2 |
| server | port | HTTP server port | 11112 |
| server | device_number | ASCOM device number | 0 |

## Usage

### Starting the Service

```bash
# With configuration file
cargo run -p ppba-switch -- -c config.json

# With command-line overrides
cargo run -p ppba-switch -- --port /dev/ttyUSB1 --server-port 11113

# With debug logging
cargo run -p ppba-switch -- -c config.json -l debug
```

### CLI Options

| Option | Description |
|--------|-------------|
| `-c, --config <FILE>` | Path to configuration file |
| `--port <PORT>` | Serial port (overrides config) |
| `--server-port <PORT>` | Server port (overrides config) |
| `-l, --log-level <LEVEL>` | Log level (trace, debug, info, warn, error) |

## ASCOM Alpaca API

### Endpoints

The service exposes standard ASCOM Alpaca Switch endpoints:

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/switch/0/maxswitch` | GET | Returns 16 |
| `/api/v1/switch/0/canwrite?Id=N` | GET | Check if switch is writable |
| `/api/v1/switch/0/getswitch?Id=N` | GET | Get boolean state |
| `/api/v1/switch/0/setswitch` | PUT | Set boolean state |
| `/api/v1/switch/0/getswitchvalue?Id=N` | GET | Get numeric value |
| `/api/v1/switch/0/setswitchvalue` | PUT | Set numeric value |
| `/api/v1/switch/0/getswitchname?Id=N` | GET | Get switch name |
| `/api/v1/switch/0/getswitchdescription?Id=N` | GET | Get switch description |
| `/api/v1/switch/0/minswitchvalue?Id=N` | GET | Get minimum value |
| `/api/v1/switch/0/maxswitchvalue?Id=N` | GET | Get maximum value |
| `/api/v1/switch/0/switchstep?Id=N` | GET | Get step size |

### Example curl Commands

```bash
# Get max switch count
curl http://localhost:11112/api/v1/switch/0/maxswitch

# Get input voltage (switch 10)
curl "http://localhost:11112/api/v1/switch/0/getswitchvalue?Id=10"

# Turn on quad 12V (switch 0)
curl -X PUT http://localhost:11112/api/v1/switch/0/setswitch \
  -d "Id=0&State=true"

# Set dew heater A to 50% (switch 2, PWM 128)
curl -X PUT http://localhost:11112/api/v1/switch/0/setswitchvalue \
  -d "Id=2&Value=128"
```

## Architecture

### Module Structure

```
ppba-switch/
├── src/
│   ├── lib.rs           # Crate root, start_server()
│   ├── main.rs          # CLI entry point
│   ├── config.rs        # Configuration types
│   ├── error.rs         # Error types
│   ├── device.rs        # ASCOM Device + Switch implementation
│   ├── protocol.rs      # PPBA command/response handling
│   ├── io.rs            # I/O trait abstractions
│   ├── serial.rs        # tokio-serial implementations
│   └── switches.rs      # Switch definitions
├── tests/
│   ├── test_protocol.rs # Protocol parsing tests
│   └── test_switches.rs # Switch definition tests
└── examples/
    ├── config-linux.json
    ├── config-macos.json
    └── config-windows.json
```

### Key Design Decisions

1. **I/O Trait Abstraction**: Serial I/O is abstracted behind traits (`SerialReader`, `SerialWriter`, `SerialPortFactory`) to enable testing without hardware.

2. **Background Polling**: Device status is polled periodically and cached. This provides fast reads while keeping the cached state reasonably current.

3. **PWM Values**: Dew heaters use raw 0-255 PWM values matching the device protocol directly. ASCOM clients can use `SetSwitchValue()` with the PWM value.

4. **Synchronous Operations**: The MVP uses synchronous switch operations. Async switch methods are not implemented.

5. **USB Hub Tracking**: USB hub state is tracked separately since it's not included in the `PA` status response.

## Testing

```bash
# Run all tests
cargo test -p ppba-switch

# Run with verbose output
cargo test -p ppba-switch -- --nocapture

# Run specific test module
cargo test -p ppba-switch test_protocol
```

## Dependencies

- `ascom-alpaca` - ASCOM Alpaca server and device traits
- `tokio-serial` - Async serial port communication
- `tokio` - Async runtime
- `serde` / `serde_json` - Configuration parsing
- `clap` - Command-line argument parsing
- `tracing` - Logging
- `thiserror` - Error handling
