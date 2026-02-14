# Running GitHub CI Workflows Locally

> **See also:** [docs/pre-push-checklist.md](docs/pre-push-checklist.md) for a
> complete mapping of every CI check to its local equivalent, and the
> `/pre-push` Claude Code command that runs the full suite via `act`.

This repository is set up to run GitHub Actions workflows locally using [act](https://github.com/nektos/act).

## Prerequisites

- Docker (installed and running)
- [act](https://github.com/nektos/act) installed
- Rust toolchain (for fallback cargo commands when Docker is unavailable)

## Quick Start

Run individual workflow jobs with `act`:

```bash
# List all available jobs
act --list

# Run specific jobs
act -W .github/workflows/check.yml -j fmt
act -W .github/workflows/check.yml -j clippy
act -W .github/workflows/check.yml -j hack
act -W .github/workflows/test.yml -j required
act -W .github/workflows/test.yml -j coverage
act -W .github/workflows/safety.yml -j sanitizers

# Run jobs with dependencies (act resolves the chain)
act -W .github/workflows/check.yml -j discover-msrv -j msrv
act -W .github/workflows/conformu.yml -j discover -j conformu

# Run entire workflows
act -W .github/workflows/check.yml
act -W .github/workflows/test.yml
act -W .github/workflows/scheduled.yml
```

## ASCOM Alpaca Conformance Testing

The project includes comprehensive ASCOM Alpaca conformance testing using [ConformU](https://github.com/ASCOMInitiative/ConformU).

### Quick Conformance Testing

```bash
# Install ConformU (first time only)
./scripts/test-conformance.sh --install-conformu

# Run conformance tests
./scripts/test-conformance.sh

# Run with custom options
./scripts/test-conformance.sh --port 12345 --verbose --keep-reports
```

### CI Integration

Conformance tests run automatically in CI when service code changes:
- Full ASCOM interface conformance testing
- Alpaca protocol compliance testing
- Automated report generation and artifact storage

## Available Workflows

### check.yml
- **fmt**: Code formatting check (`cargo fmt --check`)
- **clippy**: Linting with clippy (stable + beta)
- **hack**: Feature flag combinations (`cargo hack --feature-powerset check`)
- **discover-msrv** + **msrv**: Minimum supported Rust version verification per service

### test.yml
- **required**: Main test suite (stable + beta Rust)
- **os-check**: Cross-platform testing (macOS, Windows) — CI-only, skipped locally
- **coverage**: Test coverage collection with llvm-cov

### safety.yml
- **sanitizers**: Address and leak sanitizer checks (nightly)

### conformu.yml
- **discover** + **conformu**: ASCOM Alpaca conformance tests per service (all platforms)

### scheduled.yml (rolling)
- **nightly**: Test suite on nightly toolchain
- **miri**: Miri interpreter checks (push to main only)
- **update**: Tests with updated dependencies (beta)

## Direct `act` Usage

```bash
# Run a specific job
act -W .github/workflows/check.yml -j fmt

# Run with verbose output
act -W .github/workflows/check.yml -j clippy --verbose

# Run specific workflow
act -W .github/workflows/check.yml

# List all jobs
act --list

# Run with different event
act pull_request -W .github/workflows/test.yml -j required
```

## Configuration Files

- `.actrc`: act configuration (Docker images, settings)
- `.env`: Environment variables for workflows

## Tips

1. **First run takes longer**: Docker images need to be downloaded
2. **Use specific jobs**: Running entire workflows can be slow
3. **Check formatting first**: `act -W .github/workflows/check.yml -j fmt` is the fastest check
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

Use the `/pre-push` Claude Code command for the full quality-gate suite:

```
/pre-push          # All checks except miri
/pre-push miri     # All checks including miri
```

Or run `act` directly for targeted checks:

```bash
# Quick pre-commit checks
act -W .github/workflows/check.yml -j fmt
act -W .github/workflows/check.yml -j clippy

# Before pushing (full validation)
act -W .github/workflows/test.yml -j required
act -W .github/workflows/safety.yml -j sanitizers
```
