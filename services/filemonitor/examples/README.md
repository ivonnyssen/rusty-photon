# Filemonitor Configuration Examples

This directory contains platform-specific configuration examples for the filemonitor service.

## Platform-Specific Configurations

### Linux (`config-linux.json`)
- Uses Unix-style paths: `/home/user/observatory/RoofStatusFile.txt`
- Standard configuration for Linux systems

### macOS (`config-macos.json`)
- Uses macOS-style paths: `/Users/username/Observatory/RoofStatusFile.txt`
- Compatible with both Intel and Apple Silicon Macs

### Windows (`config-windows.json`)
- Uses Windows-style paths: `C:\Observatory\RoofStatusFile.txt`
- Supports UNC paths: `\\server\share\RoofStatusFile.txt`

## Usage

1. Copy the appropriate configuration file for your platform
2. Modify the `file.path` to point to your actual status file
3. Adjust other settings as needed
4. Run the service:

### Linux/macOS
```bash
cargo run --release -- -c config-linux.json
# or
cargo run --release -- -c config-macos.json
```

### Windows
```cmd
cargo run --release -- -c config-windows.json
```

## Cross-Platform Notes

- All configurations use the same JSON structure
- Only file paths need to be adjusted for each platform
- Network settings (port, device_number) are identical across platforms
- Parsing rules work the same way on all platforms
