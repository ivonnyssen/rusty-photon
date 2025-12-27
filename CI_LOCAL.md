# Running GitHub CI Workflows Locally

This repository is set up to run GitHub Actions workflows locally using [act](https://github.com/nektos/act).

## Prerequisites

- Docker (installed and running) - for full workflow simulation
- Rust toolchain - for simple mode checks
- act (installed via the setup script)

## Quick Start

Use the provided helper script to run workflows:

```bash
# List all available jobs
./run-ci.sh --list

# Run simple checks (faster, no Docker required)
./run-ci.sh --simple fmt      # Check code formatting
./run-ci.sh --simple clippy   # Run clippy lints

# Run full workflow jobs (requires Docker)
./run-ci.sh fmt          # Full formatting workflow
./run-ci.sh required     # Run main test suite
./run-ci.sh coverage     # Run tests with coverage
./run-ci.sh conformance  # Run ASCOM Alpaca conformance tests

# Run entire workflows
./run-ci.sh --workflow check.yml
./run-ci.sh --workflow test.yml
./run-ci.sh --workflow conformance.yml
```

## ASCOM Alpaca Conformance Testing

The project includes comprehensive ASCOM Alpaca conformance testing using [ConformU](https://github.com/ASCOMInitiative/ConformU).

### Quick Conformance Testing

```bash
# Install ConformU (first time only)
./test-conformance.sh --install-conformu

# Run conformance tests
./test-conformance.sh

# Run with custom options
./test-conformance.sh --port 12345 --verbose --keep-reports
```

### CI Integration

Conformance tests run automatically in CI when filemonitor code changes:
- Full ASCOM interface conformance testing
- Alpaca protocol compliance testing
- Automated report generation and artifact storage

### For AI Assistants

To run conformance tests programmatically:

```bash
# Install ConformU
mkdir -p ~/tools/conformu
cd ~/tools/conformu
wget https://github.com/ASCOMInitiative/ConformU/releases/download/v4.1.0/conformu.linux-x64.tar.gz
tar -xf conformu.linux-x64.tar.gz
chmod +x conformu

# Test the filemonitor service
./conformu conformance http://localhost:11111/api/v1/safetymonitor/0
./conformu alpacaprotocol http://localhost:11111/api/v1/safetymonitor/0
```

## Simple vs Full Mode

### Simple Mode (Recommended for quick checks)
- **Faster**: No Docker overhead
- **Lighter**: Uses local Rust toolchain
- **Limited**: Only supports fmt and clippy
- **Usage**: `./run-ci.sh --simple <job>`

### Full Mode (Complete workflow simulation)
- **Complete**: Full GitHub Actions environment
- **Slower**: Docker container setup required
- **Comprehensive**: All workflow features
- **Usage**: `./run-ci.sh <job>`

## Available Workflows

### check.yml
- **fmt**: Code formatting check (`cargo fmt --check`)
- **clippy**: Linting with clippy
- **doc**: Documentation generation
- **hack**: Feature flag combinations
- **msrv**: Minimum supported Rust version check

### test.yml
- **required**: Main test suite (stable + beta Rust)
- **minimal**: Tests with minimal dependency versions
- **os-check**: Cross-platform testing (macOS, Windows)
- **coverage**: Test coverage collection

### safety.yml
- **sanitizers**: Memory safety checks
- **miri**: Miri interpreter checks
- **loom**: Concurrency testing

### nostd.yml
- **nostd**: No-std compatibility checks

## Manual act Usage

You can also use act directly:

```bash
# Run a specific job
act --job fmt

# Run with verbose output
act --job clippy --verbose

# Run specific workflow
act --workflows .github/workflows/check.yml

# List all jobs
act --list

# Run with different event
act pull_request --job required
```

## Configuration Files

- `.actrc`: act configuration (Docker images, settings)
- `.env`: Environment variables for workflows
- `run-ci.sh`: Helper script for common operations

## Tips

1. **First run takes longer**: Docker images need to be downloaded
2. **Use specific jobs**: Running entire workflows can be slow
3. **Check formatting first**: `./run-ci.sh fmt` is the fastest check
4. **Memory usage**: Some jobs (like miri) require significant memory
5. **Network required**: Some tests may need internet access

## Troubleshooting

### Docker permission issues
```bash
sudo usermod -aG docker $USER
# Then log out and back in
```

### act not found
```bash
# Reinstall act
curl -s https://raw.githubusercontent.com/nektos/act/master/install.sh | sudo bash
sudo mv ./bin/act /usr/local/bin/
```

### Workflow fails locally but passes on GitHub
- Check environment variables in `.env`
- Ensure Docker has enough resources
- Some GitHub-specific features may not work locally

## Integration with Development Workflow

Add to your development routine:

```bash
# Quick pre-commit checks (simple mode)
./run-ci.sh --simple fmt      # Quick format check
./run-ci.sh --simple clippy   # Quick lint check

# Before pushing (full validation)
./run-ci.sh required # Full test suite

# Alternative: use cargo directly for fastest checks
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo build --all --quiet --color never
cargo test --all --quiet --color never
```

This matches the project's development workflow rules from AGENTS.md:
- Always run `cargo build --all --quiet --color never`
- Always run `cargo test --all --quiet --color never`
- Fix all errors and warnings before committing
