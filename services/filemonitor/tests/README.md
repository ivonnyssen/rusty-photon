# ConformU Integration Tests

This directory contains integration tests that use ConformU for ASCOM Alpaca compliance testing.

## Running ConformU Tests

The ConformU tests are integrated into the Rust test suite but require ConformU to be installed:

```bash
# Install ConformU first (if not already installed)
./test-conformance.sh --install-conformu

# Run ConformU integration tests
cargo test --test conformu_integration -- --ignored

# Or run all tests including ConformU
cargo test -- --ignored
```

## Test Structure

- `conformu_integration.rs`: Main ConformU compliance test
- Uses `ascom_alpaca::test::run_conformu_tests` for programmatic ConformU execution
- Creates temporary test environment with config and status files
- Starts filemonitor service and runs both conformance and protocol tests

## Requirements

- ConformU must be installed and available on PATH or in default location
- Tests are marked with `#[ignore]` to prevent running in CI without ConformU setup
- Requires `test` feature enabled in ascom-alpaca dependency
