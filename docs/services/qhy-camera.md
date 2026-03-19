# QHY Camera Service

## Overview

The `qhy-camera` service is an ASCOM Alpaca driver for QHYCCD cameras and filter wheels. It communicates with camera hardware through the QHYCCD C SDK via `qhyccd-rs` Rust bindings, exposing ASCOM Camera and FilterWheel devices over HTTP.

The service supports single-frame exposures with async abort, CCD info queries, binning, ROI, gain/offset control, cooler management, readout modes, and filter wheel positioning.

## Architecture

The service follows the same architecture as `qhy-focuser`:

- **SDK abstraction**: Trait-based I/O (`io.rs`) for testability — `SdkProvider`, `CameraHandle`, `FilterWheelHandle`
- **Camera device**: Implements `Device` + `Camera` traits (`camera_device.rs`)
- **Filter wheel device**: Implements `Device` + `FilterWheel` traits (`filter_wheel_device.rs`)
- **Mock mode**: Feature-gated mock SDK implementation for testing without hardware (`mock.rs`)
- **Server builder**: Configures and starts the ASCOM Alpaca server (`lib.rs`)

Unlike `qhy-focuser` which uses serial communication, this service uses the QHYCCD C SDK (via `qhyccd-rs`) for USB camera access. There is no serial port, no background polling, and no serial manager.

## Hardware Constraints

- **Connection**: USB (via libusb / QHYCCD C SDK)
- **SDK dependency**: Requires QHYCCD SDK installed at system level (headers + shared library)
- **Platform**: Linux, macOS, and Windows (SDK available for all three via `ivonnyssen/qhyccd-sdk-install`)
- **Stream mode**: Single-frame mode only (live/video mode is deferred)
- **Binning**: Symmetric only (bin_x == bin_y), mode depends on camera model (1x1, 2x2, 3x3, 4x4, 6x6, 8x8)
- **Filter wheel**: USB filter wheel accessed via same SDK, position changes are asynchronous

## ASCOM Camera Mapping

| ASCOM Property/Method | Type | Implementation |
|---|---|---|
| BayerOffsetX/Y | `u8` | From SDK CamColor Bayer pattern mapping |
| BinX/Y | `u8` | Cached, set via SDK `set_bin_mode` (symmetric) |
| CameraState | `CameraState` | From internal state machine (Idle/Exposing) |
| CameraXSize/YSize | `u32` | From cached CCD info (`image_width/height`) |
| CanAbortExposure | `bool` | `true` (always) |
| CanAsymmetricBin | `bool` | `false` (symmetric only) |
| CanFastReadout | `bool` | `true` if Speed control available with valid range |
| CanGetCoolerPower | `bool` | `true` if Cooler control available |
| CanPulseGuide | `bool` | `false` (deferred) |
| CanSetCCDTemperature | `bool` | `true` if Cooler control available |
| CanStopExposure | `bool` | `false` |
| CCDTemperature | `f64` | From SDK `CurTemp` parameter |
| CoolerOn | `bool` | Derived from SDK `CurPWM` > 0 |
| CoolerPower | `f64` | From SDK `CurPWM` (scaled 0-100%) |
| ElectronsPerADU | `f64` | NOT_IMPLEMENTED |
| ExposureMax/Min/Resolution | `Duration` | From SDK exposure min/max/step (microseconds) |
| FastReadout | `bool` | Speed at max = fast, min = normal |
| FullWellCapacity | `f64` | NOT_IMPLEMENTED |
| Gain/GainMin/GainMax | `i32` | From SDK Gain parameter with cached range |
| HasShutter | `bool` | From SDK CamMechanicalShutter control |
| ImageArray | `ImageArray` | From last captured image (ndarray transform) |
| ImageReady | `bool` | `true` when Idle and image exists |
| LastExposureDuration | `Duration` | Cached from last exposure |
| LastExposureStartTime | `SystemTime` | Cached from last exposure |
| MaxADU | `u32` | `2^OutputDataActualBits` |
| MaxBinX/Y | `u8` | Max of valid binning modes |
| NumX/Y | `u32` | From intended ROI (width/height in binned pixels) |
| Offset/OffsetMin/OffsetMax | `i32` | From SDK Offset parameter with cached range |
| PercentCompleted | `u8` | From SDK remaining exposure time |
| PixelSizeX/Y | `f64` | From cached CCD info |
| ReadoutMode | `usize` | From SDK readout mode index |
| ReadoutModes | `Vec<String>` | From SDK readout mode names |
| SensorName | `String` | Parsed from camera ID (model prefix) |
| SensorType | `SensorType` | Monochrome or RGGB from SDK CamIsColor/CamColor |
| SetCCDTemperature | `f64` | Cached target, set via SDK Cooler parameter |
| StartX/Y | `u32` | From intended ROI |
| StartExposure | `Duration`, `bool` | Spawns async exposure task (dark frames not supported) |
| AbortExposure | — | Signals exposure task via oneshot channel |
| StopExposure | — | NOT_IMPLEMENTED |

## ASCOM FilterWheel Mapping

| ASCOM Property/Method | Type | Implementation |
|---|---|---|
| FocusOffsets | `Vec<i32>` | Returns zeros (no offset data from hardware) |
| Names | `Vec<String>` | Default names "Filter0", "Filter1", ... or from config |
| Position | `Option<usize>` | `None` = moving (actual != target), `Some(n)` = at position |
| SetPosition | `usize` | Validates range, sends to SDK, caches target |

## Camera State Machine

```
                    start_exposure()
        Idle ─────────────────────────> Exposing
         ^                                  │
         │                                  │
         │  abort_exposure() ───────────────┤
         │  (via oneshot channel)           │
         │                                  │
         │         exposure completes       │
         └──────────────────────────────────┘
               (image stored, state → Idle)
```

**Exposure lifecycle:**
1. `start_exposure()` validates ROI, sets SDK parameters, transitions to Exposing state
2. A background `tokio::spawn` task runs the blocking SDK calls:
   - `start_single_frame_exposure()` (blocking)
   - Check for abort signal
   - `get_image_size()` (blocking)
   - Check for abort signal
   - `get_single_frame(buffer_size)` (blocking)
   - Transform and store image
3. On abort: sends signal via `oneshot` channel, exposure task calls `abort_exposure_and_readout()` and completes data exchange for SDK synchronization
4. On completion: image stored in `last_image`, state returns to Idle

## Image Processing Pipeline

Raw bytes from `get_single_frame()` → `transform_image_static()`:
1. Validate data length matches width × height × bytes_per_pixel
2. For 8-bit: interpret as `Vec<u8>`, reshape to `Array3<u8>(height, width, 1)`
3. For 16-bit: convert byte pairs to `u16` (native endian), reshape to `Array3<u16>(height, width, 1)`
4. Swap axes 0↔1 to get `(width, height, 1)` — ASCOM expects column-major image layout
5. Convert to `ImageArray`

## Configuration

```json
{
  "server": {
    "port": 11116
  },
  "cameras": [
    {
      "unique_id": "QHY600M-abc123",
      "name": "QHY600M Main Camera",
      "description": "QHYCCD QHY600M cooled CMOS camera",
      "device_number": 0,
      "enabled": true
    }
  ],
  "filter_wheels": [
    {
      "unique_id": "CFW=QHY600M-abc123",
      "name": "QHYCCD Filter Wheel",
      "description": "QHYCCD CFW3 filter wheel",
      "device_number": 0,
      "enabled": true,
      "filter_names": ["L", "R", "G", "B", "Ha", "OIII", "SII"]
    }
  ]
}
```

### CLI Arguments

| Argument | Description |
|----------|-------------|
| `-c, --config` | Path to configuration file |
| `--server-port` | Server port (overrides config) |
| `-l, --log-level` | Log level: trace, debug, info, warn, error |

## Module Structure

| Module | Description |
|--------|-------------|
| `config.rs` | Configuration types and loading |
| `error.rs` | Error types with ASCOM error mapping |
| `io.rs` | SDK trait abstractions (SdkProvider, CameraHandle, FilterWheelHandle) |
| `camera_device.rs` | ASCOM Device + Camera trait implementation |
| `filter_wheel_device.rs` | ASCOM Device + FilterWheel trait implementation |
| `mock.rs` | Mock SDK implementation (feature-gated) |
| `lib.rs` | Module declarations, ServerBuilder |
| `main.rs` | CLI entry point |

## Testing

- **BDD tests** (cucumber-rs): Connection lifecycle, camera properties, exposure control, filter wheel control — all using mock SDK infrastructure
- **Unit tests**: Image transformation, config defaults, error types
- **Server tests**: Server startup with mock feature (`test_lib.rs`, feature-gated)
- **ConformU**: ASCOM Alpaca compliance testing (using `simulation` feature, requires ConformU)

```bash
# Run all tests
cargo test -p qhy-camera --quiet

# Run BDD tests specifically
cargo test -p qhy-camera --test bdd

# Run with mock feature (for server tests)
cargo test -p qhy-camera --features mock

# Run ConformU compliance tests
cargo test -p qhy-camera --features simulation --test conformu_integration -- --ignored

# Run in mock mode
cargo run -p qhy-camera --features mock
```

## Connection Lifecycle

### Camera
1. ASCOM client calls `set_connected(true)`
2. SDK opens camera device
3. Verify SingleFrameMode is available
4. Set stream mode to SingleFrameMode
5. Set default readout mode (0)
6. Initialize camera (`init()`)
7. Set transfer bit depth to 16-bit
8. Cache CCD info (chip dimensions, pixel sizes)
9. Cache effective area as initial ROI
10. Detect and cache valid binning modes
11. Cache readout speed min/max/step (if available)
12. Cache exposure min/max/step
13. Cache gain min/max (if available)
14. Cache offset min/max (if available)

### Filter Wheel
1. ASCOM client calls `set_connected(true)`
2. SDK opens filter wheel device
3. Query and cache number of filters
4. Query and cache current position

## MVP Scope

**In scope:**
- Camera: connect, disconnect, single-frame exposure with async abort, image retrieval, gain/offset, binning, ROI, CCD info, exposure limits, cooler control, sensor type, Bayer info, readout modes, fast readout
- FilterWheel: connect, disconnect, position get/set with moving state (`None`), filter count, filter names, focus offsets
- JSON configuration file
- Feature-gated mock for testing
- BDD tests + unit tests
- `simulation` feature for ConformU integration tests

**Deferred:**
- Live/video mode, dark frames, pulse guiding
- Multi-camera management (SDK enumeration)
- Camera-attached filter wheel access
- Real `qhyccd-rs` SDK adapter (the `sdk` feature implementation is a separate PR)

## References

- **qhyccd-rs**: [crates.io/crates/qhyccd-rs](https://crates.io/crates/qhyccd-rs) — Rust bindings for QHYCCD SDK
- **qhyccd-alpaca**: [github.com/ivonnyssen/qhyccd-alpaca](https://github.com/ivonnyssen/qhyccd-alpaca) — Upstream standalone driver (source for this port)
- **QHYCCD SDK**: [qhyccd.com/html/prepub/log_en.html](https://www.qhyccd.com/html/prepub/log_en.html) — Official SDK
