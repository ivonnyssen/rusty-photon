#!/usr/bin/env bash
set -euo pipefail

# ConformU test script for PPBA driver
# This script starts the PPBA driver and runs ConformU compliance tests

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
TEST_DIR=$(mktemp -d -t conformu-ppba-XXXXXX)
DEVICE_TYPE="${1:-switch}"  # switch or observingconditions

cleanup() {
    local exit_code=$?
    echo "Cleaning up..."
    if [ -n "${SERVER_PID:-}" ]; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    rm -rf "$TEST_DIR"
    exit $exit_code
}

trap cleanup EXIT INT TERM

echo "=== ConformU Test for PPBA Driver (${DEVICE_TYPE}) ==="
echo "Test directory: $TEST_DIR"

# Create ConformU settings file
cat > "$TEST_DIR/conformu-settings.json" << 'EOF'
{
    "SettingsCompatibilityVersion": 1,
    "GoHomeOnDeviceSelected": true,
    "ConnectionTimeout": 2,
    "RunAs32Bit": false,
    "RiskAcknowledged": false,
    "DisplayMethodCalls": false,
    "UpdateCheck": false,
    "ApplicationPort": 0,
    "ConnectDisconnectTimeout": 5,
    "Debug": false,
    "TraceDiscovery": false,
    "TraceAlpacaCalls": false,
    "TestProperties": true,
    "TestMethods": true,
    "TestPerformance": false,
    "AlpacaDevice": {},
    "AlpacaConfiguration": {},
    "ComDevice": {},
    "ComConfiguration": {},
    "DeviceName": "No device selected",
    "DeviceTechnology": "NotSelected",
    "ReportGoodTimings": true,
    "ReportBadTimings": true,
    "TelescopeTests": {},
    "TelescopeExtendedRateOffsetTests": true,
    "TelescopeFirstUseTests": true,
    "TestSideOfPierRead": false,
    "TestSideOfPierWrite": false,
    "CameraFirstUseTests": true,
    "CameraTestImageArrayVariant": true,
    "SwitchEnableSet": false,
    "SwitchReadDelay": 50,
    "SwitchWriteDelay": 100,
    "SwitchExtendedNumberTestRange": 100,
    "SwitchAsyncTimeout": 10,
    "SwitchTestOffsets": true,
    "ObservingConditionsNumReadings": 5,
    "ObservingConditionsReadInterval": 50
}
EOF

# Create device configuration
if [ "$DEVICE_TYPE" = "switch" ]; then
    cat > "$TEST_DIR/config.json" << EOF
{
    "serial": {
        "port": "/dev/mock",
        "baud_rate": 9600,
        "polling_interval_ms": 60000,
        "timeout_seconds": 2
    },
    "server": {
        "port": 0
    },
    "switch": {
        "enabled": true,
        "name": "ConformU Test PPBA",
        "unique_id": "conformu-ppba-001",
        "description": "Test PPBA Switch for ConformU compliance",
        "device_number": 0
    },
    "observingconditions": {
        "enabled": false,
        "name": "ConformU Test PPBA Weather",
        "unique_id": "conformu-ppba-weather-001",
        "description": "Test PPBA ObservingConditions",
        "device_number": 0
    }
}
EOF
    DEVICE_PATH="switch/0"
else
    cat > "$TEST_DIR/config.json" << EOF
{
    "serial": {
        "port": "/dev/mock",
        "baud_rate": 9600,
        "polling_interval_ms": 60000,
        "timeout_seconds": 2
    },
    "server": {
        "port": 0
    },
    "switch": {
        "enabled": false,
        "name": "ConformU Test PPBA Switch",
        "unique_id": "conformu-ppba-switch-001",
        "description": "Test PPBA Switch",
        "device_number": 0
    },
    "observingconditions": {
        "enabled": true,
        "name": "ConformU Test PPBA Weather",
        "unique_id": "conformu-ppba-weather-001",
        "description": "Test PPBA ObservingConditions for ConformU compliance",
        "device_number": 0
    }
}
EOF
    DEVICE_PATH="observingconditions/0"
fi

echo "Building ppba-driver..."
cd "$PROJECT_ROOT"
cargo build -p ppba-driver --features mock --quiet

echo "Starting ppba-driver server..."
RUST_LOG=info cargo run -p ppba-driver --features mock --quiet -- -c "$TEST_DIR/config.json" > "$TEST_DIR/server.log" 2>&1 &
SERVER_PID=$!

echo "Waiting for server to bind (PID: $SERVER_PID)..."

# Wait for bound address in logs (max 30 seconds)
PORT=""
for i in {1..30}; do
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        echo "ERROR: Server process died unexpectedly"
        cat "$TEST_DIR/server.log"
        exit 1
    fi

    # Look for the bound address in logs (strip ANSI codes first)
    PORT=$(grep -m 1 "Bound Alpaca server" "$TEST_DIR/server.log" 2>/dev/null | sed 's/\x1b\[[0-9;]*m//g' | grep -oP 'bound_addr=0\.0\.0\.0:\K\d+' || true)

    if [ -n "$PORT" ]; then
        echo "Server bound to port: $PORT"
        break
    fi

    sleep 1
done

if [ -z "$PORT" ]; then
    echo "ERROR: Failed to detect bound port within 30 seconds"
    echo "Server log:"
    cat "$TEST_DIR/server.log"
    exit 1
fi

# Additional health check
echo "Verifying server is responding..."
for i in {1..10}; do
    if curl -sf "http://localhost:$PORT/management/v1/description" > /dev/null 2>&1; then
        echo "Server is ready!"
        break
    fi
    if [ $i -eq 10 ]; then
        echo "ERROR: Server not responding to health check"
        exit 1
    fi
    sleep 1
done

# Run ConformU tests
echo ""
echo "=========================================="
echo "Running ConformU Compliance Tests"
echo "Device: http://localhost:$PORT/api/v1/$DEVICE_PATH"
echo "=========================================="
echo ""

conformu conformance "http://localhost:$PORT/api/v1/$DEVICE_PATH" \
    --settingsfile "$TEST_DIR/conformu-settings.json" \
    --logfilename "$TEST_DIR/conformu.log" \
    --resultsfile "$TEST_DIR/conformu-results.json"

CONFORMU_EXIT=$?

echo ""
echo "=========================================="
if [ $CONFORMU_EXIT -eq 0 ]; then
    echo "✅ ConformU compliance tests PASSED"
    echo "All ASCOM Alpaca compliance requirements met"
else
    echo "❌ ConformU compliance tests FAILED"
    echo "See logs for details: $TEST_DIR/conformu.log"
fi
echo "=========================================="

exit $CONFORMU_EXIT
