#!/usr/bin/env bash
set -euo pipefail

# Master script to run all ConformU tests for PPBA driver

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFORMU_SCRIPT="$SCRIPT_DIR/conformu_test.sh"

echo "========================================"
echo "PPBA Driver - ConformU Compliance Tests"
echo "========================================"
echo ""

FAILED=0

# Test Switch device
echo ">>> Testing Switch device..."
if "$CONFORMU_SCRIPT" switch; then
    echo "✅ Switch tests PASSED"
else
    echo "❌ Switch tests FAILED"
    FAILED=1
fi

echo ""
echo "----------------------------------------"
echo ""

# Test ObservingConditions device
echo ">>> Testing ObservingConditions device..."
if "$CONFORMU_SCRIPT" observingconditions; then
    echo "✅ ObservingConditions tests PASSED"
else
    echo "❌ ObservingConditions tests FAILED"
    FAILED=1
fi

echo ""
echo "========================================"
if [ $FAILED -eq 0 ]; then
    echo "✅ ALL ConformU tests PASSED"
else
    echo "❌ SOME ConformU tests FAILED"
fi
echo "========================================"

exit $FAILED
