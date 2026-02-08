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

| ID | Name | Type | Min | Max | Step | Command | Notes |
|----|------|------|-----|-----|------|---------|-------|
| 0 | Quad 12V Output | Boolean | 0 | 1 | 1 | `P1:b` | |
| 1 | Adjustable Output | Boolean | 0 | 1 | 1 | `P2:b` | |
| 2 | Dew Heater A | PWM | 0 | 255 | 1 | `P3:nnn` | Read-only when auto-dew enabled |
| 3 | Dew Heater B | PWM | 0 | 255 | 1 | `P4:nnn` | Read-only when auto-dew enabled |
| 4 | USB Hub | Boolean | 0 | 1 | 1 | `PU:b` | |
| 5 | Auto-Dew | Boolean | 0 | 1 | 1 | `PD:b` | |

**Note:** See [Auto-Dew Behavior](#auto-dew-behavior) for important information about the interaction between auto-dew and manual dew heater control.

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
│   ├── test_protocol.rs      # Protocol parsing tests
│   ├── test_switches.rs      # Switch definition tests
│   ├── test_config.rs        # Configuration tests
│   ├── test_device_mock.rs   # Device tests with mock serial
│   └── conformu_integration.rs # ASCOM ConformU compliance tests
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

## Auto-Dew Behavior

The PPBA has a built-in auto-dew feature (switch 5) that automatically calculates and applies optimal PWM values to the dew heaters based on ambient temperature and humidity readings.

### Dynamic Write Protection

This driver implements **dynamic write protection** for dew heater switches (2 & 3) based on the auto-dew state:

**When auto-dew is ENABLED (switch 5 = ON):**
- `CanWrite(2)` and `CanWrite(3)` return `false` (read-only)
- Attempting to write to switches 2 or 3 returns an `INVALID_OPERATION` error
- Error message: "Cannot write to switch X while auto-dew is enabled. Disable auto-dew (switch 5) first."

**When auto-dew is DISABLED (switch 5 = OFF):**
- `CanWrite(2)` and `CanWrite(3)` return `true` (writable)
- Switches 2 & 3 can be written normally

**When disconnected:**
- `CanWrite()` for any switch returns a `NOT_CONNECTED` error (per ASCOM specification)

### State Caching and Refresh Behavior

The driver caches device state to minimize serial communication overhead:

- **Background polling**: Device state is refreshed every `polling_interval_ms` (default: 5000ms)
- **CanWrite() queries**: Use cached state (may be up to `polling_interval_ms` stale if auto-dew changed externally), except for dew heaters (switches 2 & 3) which refresh the cache if not yet populated to ensure accurate writability reporting
- **SetSwitchValue() for dew heaters**: Refreshes state immediately before validation (always validates against current device state)
- **After successful writes**: State is refreshed immediately to reflect the change
- **External changes**: Auto-dew changes made by other clients or via serial are detected within the polling interval

For tighter synchronization with external changes, reduce `polling_interval_ms` in the configuration. However, note that very short intervals (< 1000ms) increase serial communication overhead.

### Manual Dew Heater Control

To manually control dew heaters:

```bash
# 1. First, disable auto-dew (switch 5)
curl -X PUT http://localhost:11112/api/v1/switch/0/setswitch \
  -d "Id=5&State=false"

# 2. Now manual dew heater control will work
curl -X PUT http://localhost:11112/api/v1/switch/0/setswitchvalue \
  -d "Id=2&Value=128"
```

If you attempt to set a dew heater while auto-dew is enabled, you'll receive an error:

```bash
# This will fail with INVALID_OPERATION error:
curl -X PUT http://localhost:11112/api/v1/switch/0/setswitchvalue \
  -d "Id=2&Value=128"
# Error: "Cannot write to switch 2 while auto-dew is enabled. Disable auto-dew (switch 5) first."
```

### Client Recommendations

For robust client applications:

1. **Always connect first**: `CanWrite()` requires an active connection
2. **Check CanWrite() before writing**: Query `CanWrite(id)` to determine if a switch is currently writable
3. **Handle write errors gracefully**: Catch `INVALID_OPERATION` errors when writing to dew heaters
4. **Update UI on auto-dew changes**: If your UI allows controlling both auto-dew and manual heaters, update the dew heater controls' enabled/disabled state when auto-dew changes

Example client flow:

```python
# Connect to device
device.Connected = True

# Check if dew heater A is writable
if device.CanWrite(2):
    # Write is allowed
    device.SetSwitchValue(2, 128)
else:
    # Dew heater is read-only (auto-dew is probably ON)
    print("Cannot write to dew heater while auto-dew is enabled")
```

### ConformU Testing

When running ASCOM ConformU compliance tests against real hardware, auto-dew must be disabled first for the dew heater tests (switches 2 and 3) to pass.

## Testing

```bash
# Run all tests (includes mock-based tests)
cargo test -p ppba-switch --all-features

# Run with verbose output
cargo test -p ppba-switch --all-features -- --nocapture

# Run specific test module
cargo test -p ppba-switch --all-features test_protocol

# Run ConformU compliance test (requires ConformU installed)
cargo test -p ppba-driver --features mock --test conformu_integration -- --ignored --nocapture
```

Note: The `--all-features` flag enables the `mock` feature which is required for device tests that use mock serial ports.

### ConformU Compliance Testing

The driver includes ASCOM ConformU compliance tests that verify conformance to the ASCOM Switch and ObservingConditions interface specifications. These tests run in CI via the `conformu.yml` workflow.

**Performance optimization**: ConformU uses configurable delays between Switch read/write operations. The test uses a complete ConformU settings file with reduced delays:
- `SwitchReadDelay`: 50ms (default: 500ms)
- `SwitchWriteDelay`: 100ms (default: 3000ms)

This reduces test time from ~8 minutes to ~35 seconds per platform.

**Important**: ConformU requires a complete settings file with all required properties. Partial settings files (with only the Switch delays) are ignored and overwritten with defaults.

#### Running ConformU Against Real Hardware

To run ConformU compliance tests against the actual PPBA hardware on `/dev/ttyUSB0`:

**Step 1: Ensure auto-dew is disabled on the hardware**

Auto-dew must be OFF before running ConformU, otherwise the dew heater write tests will fail (CanWrite will return false for switches 2 and 3).

**Step 2: Start the ppba-switch service**

```bash
# Start the service with the real hardware configuration
cargo run -p ppba-switch -- -c services/ppba-switch/config.json
```

The service will connect to the PPBA on `/dev/ttyUSB0` and start the Alpaca server on port 11112.

**Step 3: Run ConformU**

In a separate terminal, run ConformU with default hardware timing (recommended for real hardware):

```bash
# Run ConformU against the Switch device with default timing
conformu conformance http://localhost:11112/api/v1/switch/0
```

**Note:** We use ConformU's default timing settings for real hardware tests (SwitchReadDelay: 500ms, SwitchWriteDelay: 3000ms). These conservative delays ensure reliable operation with actual hardware. The automated CI tests use reduced delays with mock hardware for faster execution.

**Expected results:**
- All tests should pass with 0 errors and 0 issues
- Test duration: ~10 minutes with default timing
- ConformU will test all 16 switches including read/write operations on controllable switches

**Troubleshooting:**
- If dew heater write tests fail with "Expected P3:XXX, got: P3:0" or "Expected P4:XXX, got: P4:0", auto-dew is enabled. Disable it before running ConformU.
- If the service fails to start, ensure no other process is using port 11112 or `/dev/ttyUSB0`
- If connection fails, verify the PPBA is powered on and connected via USB

## Dependencies

- `ascom-alpaca` - ASCOM Alpaca server and device traits
- `tokio-serial` - Async serial port communication
- `tokio` - Async runtime
- `serde` / `serde_json` - Configuration parsing
- `clap` - Command-line argument parsing
- `tracing` - Logging
- `thiserror` - Error handling
