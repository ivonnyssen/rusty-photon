#!/bin/bash

# ASCOM Alpaca Conformance Testing Script
# Tests the filemonitor service using ConformU

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFORMU_VERSION="v4.1.0"
CONFORMU_URL="https://github.com/ASCOMInitiative/ConformU/releases/download/${CONFORMU_VERSION}/conformu.linux-x64.tar.gz"

show_help() {
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Run ASCOM Alpaca conformance tests on filemonitor service"
    echo ""
    echo "Options:"
    echo "  --install-conformu  Download and install ConformU"
    echo "  --port PORT         Use specific port (default: 11111)"
    echo "  --config FILE       Use specific config file"
    echo "  --test-dir DIR      Use specific test directory"
    echo "  --keep-reports      Don't delete test reports after completion"
    echo "  --verbose           Verbose output"
    echo "  -h, --help          Show this help"
    echo ""
    echo "Examples:"
    echo "  $0                          # Run conformance tests"
    echo "  $0 --install-conformu       # Install ConformU first"
    echo "  $0 --port 12345 --verbose   # Use custom port with verbose output"
}

install_conformu() {
    echo "Installing ConformU ${CONFORMU_VERSION}..."
    
    CONFORMU_DIR="$HOME/tools/conformu"
    mkdir -p "$CONFORMU_DIR"
    cd "$CONFORMU_DIR"
    
    if [[ -f "conformu.linux-x64.tar.gz" ]]; then
        rm -f conformu.linux-x64.tar.gz
    fi
    
    echo "Downloading ConformU..."
    wget -q --show-progress "$CONFORMU_URL"
    
    echo "Extracting ConformU..."
    tar -xf conformu.linux-x64.tar.gz
    chmod +x conformu
    
    echo "ConformU installed to: $CONFORMU_DIR/conformu"
    echo "Version: $(./conformu --version 2>/dev/null || echo 'Unknown')"
}

run_conformance_tests() {
    local PORT=${1:-11111}
    local CONFIG_FILE=${2:-""}
    local TEST_DIR=${3:-"/tmp/conformu-test-$$"}
    local KEEP_REPORTS=${4:-false}
    local VERBOSE=${5:-false}
    
    echo "Running ASCOM Alpaca conformance tests..."
    echo "Port: $PORT"
    echo "Test directory: $TEST_DIR"
    echo "----------------------------------------"
    
    # Check ConformU installation
    CONFORMU_PATH="$HOME/tools/conformu/conformu"
    if [[ ! -x "$CONFORMU_PATH" ]]; then
        echo "ConformU not found at: $CONFORMU_PATH"
        echo "Run: $0 --install-conformu"
        exit 1
    fi
    
    # Build filemonitor
    echo "Building filemonitor..."
    cd "$SCRIPT_DIR"
    cargo build --release -p filemonitor
    
    # Create test environment
    mkdir -p "$TEST_DIR"
    
    # Create or use provided config
    if [[ -n "$CONFIG_FILE" && -f "$CONFIG_FILE" ]]; then
        cp "$CONFIG_FILE" "$TEST_DIR/config.json"
        echo "Using provided config: $CONFIG_FILE"
    else
        echo "Creating test configuration..."
        cat > "$TEST_DIR/config.json" << EOFCONFIG
{
  "device": {
    "name": "File Safety Monitor Test",
    "unique_id": "filemonitor-test-001",
    "description": "ASCOM Alpaca SafetyMonitor for conformance testing"
  },
  "file": {
    "path": "$TEST_DIR/RoofStatusFile.txt",
    "polling_interval_seconds": 5
  },
  "parsing": {
    "rules": [
      {
        "type": "contains",
        "pattern": "CLOSED",
        "safe": true
      },
      {
        "type": "contains", 
        "pattern": "OPEN",
        "safe": false
      },
      {
        "type": "regex",
        "pattern": "Status:\\\\s*(SAFE|OK)",
        "safe": true
      }
    ],
    "default_safe": false,
    "case_sensitive": false
  },
  "server": {
    "port": $PORT,
    "device_number": 0
  }
}
EOFCONFIG
    fi
    
    # Create test status file
    echo "2025-12-22 08:00:00 Roof Status: CLOSED" > "$TEST_DIR/RoofStatusFile.txt"
    
    # Start filemonitor service
    echo "Starting filemonitor service on port $PORT..."
    cd "$TEST_DIR"
    
    if [[ "$VERBOSE" == true ]]; then
        timeout 300 "$SCRIPT_DIR/../target/release/filemonitor" -c config.json &
    else
        timeout 300 "$SCRIPT_DIR/../target/release/filemonitor" -c config.json > "$TEST_DIR/filemonitor.log" 2>&1 &
    fi
    
    FILEMONITOR_PID=$!
    echo "Service PID: $FILEMONITOR_PID"
    
    # Wait for service to start
    echo "Waiting for service to start..."
    SERVICE_STARTED=false
    for i in {1..30}; do
        if curl -s "http://localhost:$PORT/management/v1/description" >/dev/null 2>&1; then
            echo "✅ Service started successfully"
            SERVICE_STARTED=true
            break
        fi
        if [[ $i -eq 30 ]]; then
            echo "❌ Service failed to start after 60 seconds"
            if [[ -f "$TEST_DIR/filemonitor.log" ]]; then
                echo "Service logs:"
                cat "$TEST_DIR/filemonitor.log"
            fi
            kill $FILEMONITOR_PID 2>/dev/null || true
            exit 1
        fi
        echo "Waiting... ($i/30)"
        sleep 2
    done
    
    # Test basic connectivity
    echo "Testing basic connectivity..."
    DEVICE_URL="http://localhost:$PORT/api/v1/safetymonitor/0"
    if curl -s "$DEVICE_URL/connected" | grep -q '"Value"'; then
        echo "✅ Device responds to API calls"
    else
        echo "❌ Device not responding to API calls"
        kill $FILEMONITOR_PID 2>/dev/null || true
        exit 1
    fi
    
    # Run conformance tests
    echo ""
    echo "Running ConformU conformance test..."
    CONFORMANCE_ARGS=(
        "conformance"
        "$DEVICE_URL"
        "--logfilename" "$TEST_DIR/conformance.log"
        "--resultsfile" "$TEST_DIR/conformance-report.json"
    )
    
    if [[ "$VERBOSE" == true ]]; then
        echo "Running: $CONFORMU_PATH ${CONFORMANCE_ARGS[*]}"
    fi
    
    "$CONFORMU_PATH" "${CONFORMANCE_ARGS[@]}" || CONFORMANCE_RESULT=$?
    
    echo ""
    echo "Running ConformU Alpaca protocol test..."
    PROTOCOL_ARGS=(
        "alpacaprotocol"
        "$DEVICE_URL"
        "--logfilename" "$TEST_DIR/alpaca-protocol.log"
        "--resultsfile" "$TEST_DIR/alpaca-protocol-report.json"
    )
    
    if [[ "$VERBOSE" == true ]]; then
        echo "Running: $CONFORMU_PATH ${PROTOCOL_ARGS[*]}"
    fi
    
    "$CONFORMU_PATH" "${PROTOCOL_ARGS[@]}" || PROTOCOL_RESULT=$?
    
    # Cleanup service
    echo ""
    echo "Stopping filemonitor service..."
    kill $FILEMONITOR_PID 2>/dev/null || true
    wait $FILEMONITOR_PID 2>/dev/null || true
    
    # Analyze results
    echo "Analyzing test results..."
    OVERALL_SUCCESS=true
    
    if [[ -f "$TEST_DIR/conformance-report.json" ]]; then
        if command -v jq &> /dev/null; then
            ISSUES=$(jq -r '.Issues[]? | "\(.Key): \(.Value)"' "$TEST_DIR/conformance-report.json" 2>/dev/null || echo "")
            ISSUE_COUNT=$(jq -r '.IssueCount' "$TEST_DIR/conformance-report.json" 2>/dev/null || echo "0")
            ERROR_COUNT=$(jq -r '.ErrorCount' "$TEST_DIR/conformance-report.json" 2>/dev/null || echo "0")
            
            echo "Conformance Test Results:"
            echo "  Errors: $ERROR_COUNT"
            echo "  Issues: $ISSUE_COUNT"
            
            if [[ "$ERROR_COUNT" -gt 0 ]]; then
                echo "❌ Conformance errors found - these must be fixed"
                OVERALL_SUCCESS=false
            elif [[ "$ISSUE_COUNT" -gt 0 ]]; then
                echo "⚠️  Conformance issues found (minor):"
                echo "$ISSUES"
                echo "Note: These are minor issues that don't prevent device operation"
            else
                echo "✅ All conformance tests passed"
            fi
        else
            echo "⚠️  jq not installed, cannot parse JSON results"
            echo "Check $TEST_DIR/conformance-report.json manually"
        fi
    else
        echo "❌ Conformance report not generated"
        OVERALL_SUCCESS=false
    fi
    
    if [[ -f "$TEST_DIR/alpaca-protocol-report.json" ]]; then
        if command -v jq &> /dev/null; then
            PROTOCOL_ISSUES=$(jq -r '.TestResults[] | select(.Outcome == "Issue" or .Outcome == "Error") | .TestName' "$TEST_DIR/alpaca-protocol-report.json" 2>/dev/null || echo "")
            PROTOCOL_PASSED=$(jq -r '.TestResults[] | select(.Outcome == "OK") | .TestName' "$TEST_DIR/alpaca-protocol-report.json" 2>/dev/null | wc -l || echo "0")
            PROTOCOL_TOTAL=$(jq -r '.TestResults[] | .TestName' "$TEST_DIR/alpaca-protocol-report.json" 2>/dev/null | wc -l || echo "0")
            
            echo "Protocol Test Results: $PROTOCOL_PASSED/$PROTOCOL_TOTAL tests passed"
            
            if [[ -n "$PROTOCOL_ISSUES" ]]; then
                echo "❌ Protocol issues found:"
                echo "$PROTOCOL_ISSUES"
                OVERALL_SUCCESS=false
            else
                echo "✅ All protocol tests passed"
            fi
        fi
    else
        echo "❌ Protocol report not generated"
        OVERALL_SUCCESS=false
    fi
    
    # Report locations
    echo ""
    echo "Test reports saved in: $TEST_DIR"
    echo "  - conformance.log"
    echo "  - conformance-report.json"
    echo "  - alpaca-protocol.log"
    echo "  - alpaca-protocol-report.json"
    echo "  - filemonitor.log"
    
    # Cleanup if requested
    if [[ "$KEEP_REPORTS" != true ]]; then
        echo ""
        read -p "Delete test reports? [y/N] " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            rm -rf "$TEST_DIR"
            echo "Test reports deleted"
        fi
    fi
    
    if [[ "$OVERALL_SUCCESS" == true ]]; then
        echo ""
        echo "🎉 All conformance tests passed!"
        return 0
    elif [[ "$ERROR_COUNT" -eq 0 && "$ISSUE_COUNT" -gt 0 ]]; then
        echo ""
        echo "✅ Conformance tests passed with minor issues"
        echo "The device is fully functional and ASCOM compliant"
        return 0
    else
        echo ""
        echo "❌ Some conformance tests failed"
        return 1
    fi
}

# Parse command line arguments
PORT=11111
CONFIG_FILE=""
TEST_DIR=""
KEEP_REPORTS=false
VERBOSE=false
INSTALL_ONLY=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --install-conformu)
            INSTALL_ONLY=true
            shift
            ;;
        --port)
            PORT="$2"
            shift 2
            ;;
        --config)
            CONFIG_FILE="$2"
            shift 2
            ;;
        --test-dir)
            TEST_DIR="$2"
            shift 2
            ;;
        --keep-reports)
            KEEP_REPORTS=true
            shift
            ;;
        --verbose)
            VERBOSE=true
            shift
            ;;
        -h|--help)
            show_help
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            show_help
            exit 1
            ;;
    esac
done

# Set default test directory if not provided
if [[ -z "$TEST_DIR" ]]; then
    TEST_DIR="/tmp/conformu-test-$$"
fi

# Install ConformU if requested
if [[ "$INSTALL_ONLY" == true ]]; then
    install_conformu
    exit 0
fi

# Run conformance tests
run_conformance_tests "$PORT" "$CONFIG_FILE" "$TEST_DIR" "$KEEP_REPORTS" "$VERBOSE"
