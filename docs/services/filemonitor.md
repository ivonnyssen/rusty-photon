# Filemonitor Service Design

## Overview

The filemonitor service implements an ASCOM Alpaca compatible SafetyMonitor device. It monitors a clear text file and determines safety status based on configurable parsing rules.

**Cross-Platform Support:** The service runs natively on Linux, macOS, and Windows with no platform-specific dependencies.

## Implementation Framework

The service uses the `ascom-alpaca` crate [https://crates.io/crates/ascom-alpaca](https://crates.io/crates/ascom-alpaca) which provides:

- `SafetyMonitor` trait with required `async fn is_safe(&self) -> ASCOMResult<bool>` method
- `Device` trait for common ASCOM functionality (device name, unique ID, etc.)
- `Server` struct for ASCOM Alpaca protocol handling
- Auto-discovery mechanism for network clients
- Built on async/await with tokio runtime
- Cross-platform compatibility (Linux, macOS, Windows)

## Configuration

The service uses a JSON configuration file with the following format:

```json
{
  "device": {
    "name": "File Safety Monitor",
    "unique_id": "filemonitor-001",
    "description": "ASCOM Alpaca SafetyMonitor that monitors file content"
  },
  "file": {
    "path": "/path/to/RoofStatusFile.txt",
    "polling_interval_seconds": 60
  },
  "parsing": {
    "rules": [
      {
        "type": "contains",
        "pattern": "OPEN",
        "safe": true
      },
      {
        "type": "contains", 
        "pattern": "CLOSED",
        "safe": false
      },
      {
        "type": "regex",
        "pattern": "Status:\\s*(SAFE|OK)",
        "safe": true
      }
    ],
    "case_sensitive": false
  },
  "server": {
    "port": 11111,
    "device_number": 0
  }
}
```

Configuration sections:

- **device**: ASCOM device metadata (name, unique ID, description)
- **file**: Path to monitor and polling interval in seconds
- **parsing**: Multiple rule types (contains, regex) with safe/unsafe outcomes
- **server**: ASCOM Alpaca server configuration (port, device number)

The parsing rules are evaluated in order, with the first match determining safety status. If no rules match, the device defaults to unsafe (`false`) for safety reasons.

## Operation

The service parses the monitored file according to the configured rules, yielding either:

- `true` - Safe condition
- `false` - Unsafe condition or any errors/unresolvable conflicts

The `is_safe()` method contains the core file monitoring and parsing logic, called by ASCOM clients. When the device is not connected, it returns `false` to indicate an unsafe condition, ensuring ASCOM conformance compliance.

### Connection Management

The `set_connected()` method controls the monitoring behavior:

- **When set to `true`**:
  - Immediately reloads the monitored file to get current content
  - Initiates background polling according to the configured `polling_interval_seconds`
  - Caches file content for use by `is_safe()` calls
  - Returns error if file cannot be read

- **When set to `false`**:
  - Stops the background polling task
  - `is_safe()` returns `false` when called (indicating unsafe condition)

The polling runs in a background task that periodically reads the file and updates the cached content. This ensures `is_safe()` calls are fast and don't block on file I/O.

## Architecture

```mermaid
graph TD;
    A[ASCOM Client] --> B[ASCOM Alpaca Server];
    B --> C[FileMonitorDevice];
    C --> D[File Parser];
    D --> E[Monitored File];
    
    C --> F[Connection State];
    C --> G[Cached Content];
    C --> H[Background Polling Task];
    H --> E;
    H --> G;
    
    I[is_safe called] --> J{Connected?};
    J -->|No| K[Return false unsafe];
    J -->|Yes| L{Cached Content?};
    L -->|Yes| M[Evaluate Safety Rules];
    L -->|No| N[Return false unsafe];
    M --> O[Return Safe/Unsafe];
```

## Implementation Components

1. **FileMonitorDevice**: Struct implementing `Device` and `SafetyMonitor` traits
2. **Configuration**: JSON-based config for file path, parsing rules, and device metadata
3. **File Parser**: Logic to read and parse monitored file according to rules
4. **ASCOM Server**: Uses `ascom-alpaca::Server` to expose device over network

## Example

An example monitored file `RoofStatusFile.txt` might contain:

```
???2025-12-15 01:20:13AM Roof Status: CLOSED
```

This would cause the safety monitor to evaluate to `false` (unsafe), as the roof is closed and the telescope might be in danger of colliding with the roof.

## Cross-Platform Support

The filemonitor service is designed to run natively on multiple platforms:

### Supported Platforms
- **Linux** (x86_64, ARM64) - Primary development platform
- **macOS** (Intel, Apple Silicon) - Full compatibility
- **Windows** (x86_64, ARM64) - Full compatibility

### Platform-Specific Considerations

#### File Paths
The service uses Rust's `PathBuf` for cross-platform path handling:
- **Linux/macOS**: `/home/user/observatory/RoofStatusFile.txt`
- **Windows**: `C:\Observatory\RoofStatusFile.txt` or `\\server\share\RoofStatusFile.txt`

#### Network Binding
- All platforms bind to `0.0.0.0` (IPv4) by default
- IPv6 dual-stack support available on all platforms
- Windows Firewall may require configuration for network access

#### File Monitoring
- Uses standard file I/O operations (`std::fs::read_to_string`)
- Polling-based approach works consistently across all platforms
- No platform-specific file watching dependencies

### Installation

#### Linux (Debian/Ubuntu) — `.deb` package
```bash
sudo dpkg -i filemonitor_0.1.0-1_amd64.deb
```

This installs the binary to `/usr/bin/filemonitor`, a default config to `/etc/filemonitor/config.json`, enables and starts a systemd service, and creates a `filemonitor` system user. Edit `/etc/filemonitor/config.json` to point to your monitored file. The config is preserved across package upgrades.

#### Linux / macOS — from source
```bash
cargo build --release -p filemonitor
./target/release/filemonitor -c config.json
```

#### Windows — from source
```cmd
cargo build --release -p filemonitor
.\target\release\filemonitor.exe -c config.json
```

### Service Integration

#### Linux (systemd)
A systemd unit file is provided at `pkg/filemonitor.service`. When installed via the `.deb` package, the service is automatically enabled and started. Manual setup:

```bash
sudo cp pkg/filemonitor.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now filemonitor
```

The service runs as a dedicated `filemonitor` system user, created automatically during package installation.

#### macOS / Windows
Service integration for macOS (launchd) and Windows (Windows Service) is not yet implemented.

### Monorepo Structure

The filemonitor service is located in `services/filemonitor/` within the rusty-photon monorepo.

#### Building
```bash
# From repository root
cargo build --release -p filemonitor
```

#### Running
```bash
# From repository root
./target/release/filemonitor -c config.json
```
