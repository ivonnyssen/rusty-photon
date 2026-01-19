#!/bin/bash

# Helper script to run GitHub CI workflows locally using act

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

show_help() {
    echo "Usage: $0 [OPTIONS] [JOB_NAME]"
    echo ""
    echo "Run GitHub CI workflows locally using act"
    echo ""
    echo "Options:"
    echo "  -l, --list          List all available jobs"
    echo "  -w, --workflow FILE Run specific workflow file"
    echo "  -j, --job JOB       Run specific job"
    echo "  -e, --event EVENT   Trigger event (default: push)"
    echo "  -v, --verbose       Verbose output"
    echo "  -s, --simple        Run simple version (for clippy, etc.)"
    echo "  -h, --help          Show this help"
    echo ""
    echo "Common jobs:"
    echo "  fmt                 Check code formatting"
    echo "  clippy              Run clippy lints (use --simple for basic clippy)"
    echo "  test                Run test suite"
    echo "  workspace-check     Run workspace-level checks"
    echo "  conformance         Run ASCOM Alpaca conformance tests"
    echo ""
    echo "Workflows:"
    echo "  filemonitor.yml     Binary crate workflow"
    echo "  workspace.yml       Workspace-level checks"
    echo ""
    echo "Examples:"
    echo "  $0 --list                    # List all jobs"
    echo "  $0 fmt                       # Run formatting check"
    echo "  $0 --simple clippy           # Run basic clippy"
    echo "  $0 test                      # Run tests"
    echo "  $0 --workflow filemonitor.yml # Run filemonitor workflow"
}

run_simple_clippy() {
    echo "Running simple clippy check..."
    echo "----------------------------------------"
    
    # Check if rust toolchain is available
    if ! command -v cargo &> /dev/null; then
        echo "Installing Rust toolchain..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        source ~/.cargo/env
    fi
    
    # Install clippy if not available
    if ! rustup component list --installed | grep -q clippy; then
        echo "Installing clippy..."
        rustup component add clippy
    fi
    
    # Run clippy
    cargo clippy --all-targets --all-features -- -D warnings
}

run_conformance_test() {
    echo "Running ASCOM Alpaca conformance tests..."
    echo "----------------------------------------"
    
    # Check if ConformU is available
    CONFORMU_PATH="$HOME/tools/conformu/conformu"
    if [[ ! -x "$CONFORMU_PATH" ]]; then
        echo "ConformU not found. Installing..."
        mkdir -p "$HOME/tools/conformu"
        cd "$HOME/tools/conformu"
        wget -q https://github.com/ASCOMInitiative/ConformU/releases/download/v4.1.0/conformu.linux-x64.tar.gz
        tar -xf conformu.linux-x64.tar.gz
        chmod +x conformu
        cd "$SCRIPT_DIR"
    fi
    
    # Build filemonitor
    echo "Building filemonitor..."
    cargo build --release -p filemonitor
    
    # Create test environment
    TEST_DIR="/tmp/conformu-test-$$"
    mkdir -p "$TEST_DIR"
    
    # Create test configuration
    cat > "$TEST_DIR/config.json" << 'EOFCONFIG'
{
  "device": {
    "name": "File Safety Monitor Test",
    "unique_id": "filemonitor-test-001",
    "description": "ASCOM Alpaca SafetyMonitor for conformance testing"
  },
  "file": {
    "path": "/tmp/conformu-test/RoofStatusFile.txt",
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
      }
    ],
    "default_safe": false,
    "case_sensitive": false
  },
  "server": {
    "port": 11111,
    "device_number": 0
  }
}
EOFCONFIG
    
    # Create test status file
    echo "2025-12-22 08:00:00 Roof Status: CLOSED" > "$TEST_DIR/RoofStatusFile.txt"
    
    # Start filemonitor service
    echo "Starting filemonitor service..."
    cd "$TEST_DIR"
    timeout 300 "$SCRIPT_DIR/../target/release/filemonitor" -c config.json > "$TEST_DIR/filemonitor.log" 2>&1 &
    FILEMONITOR_PID=$!
    
    # Wait for service to start
    echo "Waiting for service to start..."
    for i in {1..30}; do
        if curl -s http://localhost:11111/management/v1/description >/dev/null 2>&1; then
            echo "Service started successfully"
            break
        fi
        if [[ $i -eq 30 ]]; then
            echo "❌ Service failed to start"
            cat "$TEST_DIR/filemonitor.log"
            kill $FILEMONITOR_PID 2>/dev/null || true
            return 1
        fi
        sleep 2
    done
    
    # Run conformance tests
    echo "Running ConformU conformance test..."
    "$CONFORMU_PATH" conformance http://localhost:11111/api/v1/safetymonitor/0 \
        --logfilename "$TEST_DIR/conformance.log" \
        --resultsfile "$TEST_DIR/conformance-report.json" || CONFORMANCE_RESULT=$?
    
    echo "Running ConformU Alpaca protocol test..."
    "$CONFORMU_PATH" alpacaprotocol http://localhost:11111/api/v1/safetymonitor/0 \
        --logfilename "$TEST_DIR/alpaca-protocol.log" \
        --resultsfile "$TEST_DIR/alpaca-protocol-report.json" || PROTOCOL_RESULT=$?
    
    # Cleanup
    echo "Cleaning up..."
    kill $FILEMONITOR_PID 2>/dev/null || true
    wait $FILEMONITOR_PID 2>/dev/null || true
    
    # Check results
    echo "Checking conformance results..."
    if [[ -f "$TEST_DIR/conformance-report.json" ]]; then
        if command -v jq &> /dev/null; then
            ISSUES=$(jq -r '.TestResults[] | select(.Outcome == "Issue" or .Outcome == "Error") | .TestName' "$TEST_DIR/conformance-report.json" 2>/dev/null || echo "")
            if [[ -n "$ISSUES" ]]; then
                echo "❌ Conformance issues found:"
                echo "$ISSUES"
                echo ""
                echo "Full reports available in: $TEST_DIR"
                return 1
            else
                echo "✅ All conformance tests passed"
            fi
        else
            echo "⚠️  jq not installed, cannot parse results. Check reports manually."
        fi
    else
        echo "❌ Conformance report not generated"
        echo "Full logs available in: $TEST_DIR"
        return 1
    fi
    
    echo "Reports saved in: $TEST_DIR"
    return 0
}

run_simple_fmt() {
    echo "Running simple format check..."
    echo "----------------------------------------"
    
    # Check if rust toolchain is available
    if ! command -v cargo &> /dev/null; then
        echo "Installing Rust toolchain..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        source ~/.cargo/env
    fi
    
    # Install rustfmt if not available
    if ! rustup component list --installed | grep -q rustfmt; then
        echo "Installing rustfmt..."
        rustup component add rustfmt
    fi
    
    # Run format check
    cargo fmt --check
}

# Default values
EVENT="push"
VERBOSE=""
WORKFLOW=""
JOB=""
SIMPLE=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -l|--list)
            act --list
            exit 0
            ;;
        -w|--workflow)
            WORKFLOW="$2"
            shift 2
            ;;
        -j|--job)
            JOB="$2"
            shift 2
            ;;
        -e|--event)
            EVENT="$2"
            shift 2
            ;;
        -v|--verbose)
            VERBOSE="--verbose"
            shift
            ;;
        -s|--simple)
            SIMPLE=true
            shift
            ;;
        -h|--help)
            show_help
            exit 0
            ;;
        -*)
            echo "Unknown option: $1"
            show_help
            exit 1
            ;;
        *)
            if [[ -z "$JOB" ]]; then
                JOB="$1"
            else
                echo "Multiple job names specified"
                exit 1
            fi
            shift
            ;;
    esac
done

# Handle simple mode for specific jobs
if [[ "$SIMPLE" == true ]]; then
    case "$JOB" in
        clippy)
            run_simple_clippy
            exit $?
            ;;
        fmt)
            run_simple_fmt
            exit $?
            ;;
        *)
            echo "Simple mode not available for job: $JOB"
            echo "Available simple jobs: clippy, fmt"
            exit 1
            ;;
    esac
fi

# Handle conformance testing
if [[ "$JOB" == "conformance" ]]; then
    run_conformance_test
    exit $?
fi

# Build act command
ACT_CMD="act $VERBOSE"

if [[ -n "$WORKFLOW" ]]; then
    ACT_CMD="$ACT_CMD --workflows .github/workflows/$WORKFLOW"
fi

if [[ -n "$JOB" ]]; then
    ACT_CMD="$ACT_CMD --job $JOB"
fi

ACT_CMD="$ACT_CMD $EVENT"

echo "Running: $ACT_CMD"
echo "----------------------------------------"

# Run act
eval $ACT_CMD
