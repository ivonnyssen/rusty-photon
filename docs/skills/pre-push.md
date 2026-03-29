# Skill: Pre-Push Quality Gates

## When to Read This

- Before pushing a branch to the remote repository
- Before creating a pull request
- When you need to run CI checks locally to diagnose a failure

## Prerequisites

### Required tools

| Tool | Install | Used by |
|------|---------|---------|
| Rust stable | `rustup default stable` | All checks |
| cargo-nextest | `cargo install cargo-nextest` or `curl -LsSf https://get.nexte.st/latest/linux \| tar zxf - -C ~/.cargo/bin` | Test execution |
| cargo-hack | `cargo install cargo-hack` | Feature powerset checks |
| Docker | [docs.docker.com](https://docs.docker.com/get-docker/) | act-based workflow execution |
| act | `curl -s https://raw.githubusercontent.com/nektos/act/master/install.sh \| sudo bash` | Local CI runner |

### Optional tools

| Tool | Install | Used by |
|------|---------|---------|
| Rust beta | `rustup toolchain install beta` | Beta clippy, beta tests |
| Rust nightly | `rustup toolchain install nightly` | Sanitizers, miri |
| miri component | `rustup +nightly component add miri` | Miri checks |
| cargo-msrv | `cargo install cargo-msrv` | MSRV verification |
| cargo-llvm-cov | `cargo install cargo-llvm-cov` | Coverage |
| ConformU | [ivonnyssen/conformu-install](https://github.com/ivonnyssen/conformu-install) | Conformance tests |
| jq | `sudo apt install jq` / `brew install jq` | ConformU & miri discovery |
| llvm | `sudo apt install llvm` | Address sanitizer symbolization |

---

## Procedure

### Step 1: Run the full CI suite via `act` (recommended)

The easiest way to run all quality gates locally is with `act`, which executes
the actual GitHub Actions workflows in Docker containers.

```bash
# Run all independent checks in parallel
act -W .github/workflows/check.yml -j fmt &
act -W .github/workflows/check.yml -j clippy &
act -W .github/workflows/check.yml -j hack &
act -W .github/workflows/test.yml -j required &
act -W .github/workflows/test.yml -j coverage &
act -W .github/workflows/safety.yml -j sanitizers &
act -W .github/workflows/scheduled.yml -j nightly &
act -W .github/workflows/scheduled.yml -j update &
wait

# Then run jobs with dependencies
act -W .github/workflows/check.yml -j discover-msrv -j msrv
act -W .github/workflows/conformu.yml -j discover -j conformu

# Optional: miri (slow, per-package via discover)
act -W .github/workflows/scheduled.yml -j discover-miri -j miri
```

> **Note:** `act` runs Linux Docker containers, so the `os-check` job
> (macOS/Windows) is skipped locally. Multi-OS `conformu` jobs run the ubuntu
> variant only.

### Step 2 (fallback): Raw `cargo` commands

When Docker or `act` is unavailable, use these cargo commands directly.

With `cargo-hack`:

```bash
cargo fmt --check
cargo hack --feature-powerset clippy --all-targets -- -D warnings
cargo hack --feature-powerset check
cargo nextest run --locked --all-features --all-targets
cargo test --locked --all-features --test bdd
cargo test --locked --all-features --doc
```

Without `cargo-hack`:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --locked --all-features --all-targets
cargo test --locked --all-features --test bdd
cargo test --locked --all-features --doc
```

---

## Detailed Workflow Breakdown

### check.yml

| CI Job | Local Command | Prerequisites | Required? |
|--------|---------------|---------------|-----------|
| **fmt** | `cargo fmt --check` | stable rustfmt | Yes |
| **clippy (stable)** | `cargo clippy --all-targets --all-features -- -D warnings` | stable clippy | Yes |
| **clippy (beta)** | `cargo +beta clippy --all-targets --all-features -- -D warnings` | beta toolchain | Optional |
| **hack** | `cargo hack --feature-powerset check` | cargo-hack | Yes |
| **msrv** | `cargo msrv verify --manifest-path services/<name>/Cargo.toml` | cargo-msrv | Optional |

MSRV services are discovered dynamically. Current services with MSRV:
- filemonitor (1.88.0)
- phd2-guider (1.85.0)
- ppba-driver (1.88.0)
- qhy-focuser (1.88.0)

To check all at once:

```bash
for svc in $(cargo metadata --format-version 1 --no-deps | \
  jq -r '.packages[] | select(.rust_version) | .name'); do
  echo "--- Checking MSRV for $svc ---"
  cargo msrv verify --manifest-path "services/$svc/Cargo.toml"
done
```

### test.yml

| CI Job | Local Command | Prerequisites | Required? |
|--------|---------------|---------------|-----------|
| **required (stable)** | `cargo nextest run --locked --all-features --all-targets` + `cargo test --locked --all-features --test bdd` | stable, cargo-nextest | Yes |
| **required (stable, doc)** | `cargo test --locked --all-features --doc` | stable | Yes |
| **required (beta)** | Same commands with `+beta` | beta toolchain | Optional |
| **os-check** | N/A (cross-platform) | -- | CI-only |
| **coverage** | `cargo llvm-cov --locked --all-features --lcov` | cargo-llvm-cov, llvm-tools-preview | Optional |

### safety.yml

| CI Job | Local Command | Prerequisites | Required? |
|--------|---------------|---------------|-----------|
| **address sanitizer** | See below | nightly, llvm | Optional |
| **leak sanitizer** | See below | nightly | Optional |

Address sanitizer:

```bash
ASAN_OPTIONS="detect_odr_violation=0:detect_leaks=0" \
RUSTFLAGS="-Z sanitizer=address" \
  cargo +nightly test --lib --tests --all-features --target x86_64-unknown-linux-gnu
```

Leak sanitizer:

```bash
LSAN_OPTIONS="suppressions=lsan-suppressions.txt" \
RUSTFLAGS="-Z sanitizer=leak" \
  cargo +nightly test --all-features --target x86_64-unknown-linux-gnu
```

> **Note:** The sanitizers modify `Cargo.toml` in CI to set `[profile.dev] opt-level = 1`.
> Locally you can either do the same (and revert), or accept slightly different
> behavior. The sanitizer results are still meaningful without the opt-level tweak.

### conformu.yml

| CI Job | Local Command | Prerequisites | Required? |
|--------|---------------|---------------|-----------|
| **discover** | `cargo metadata` + jq | jq | -- |
| **conformu** | Per-service command (see below) | ConformU | Optional |

ConformU services are discovered dynamically via `[package.metadata.conformu]`
in each service's `Cargo.toml`. To list them:

```bash
cargo metadata --format-version 1 --no-deps | \
  jq -r '.packages[] | select(.metadata.conformu.command) | "\(.name): \(.metadata.conformu.command)"'
```

Current services and their commands:
- **filemonitor**: `cargo test -p filemonitor --test conformu_integration -- --ignored --nocapture`
- **ppba-driver**: `cargo test -p ppba-driver --features mock --test conformu_integration -- --ignored --nocapture`
- **qhy-focuser**: `cargo test -p qhy-focuser --features mock --test conformu_integration -- --ignored --nocapture`

### scheduled.yml (rolling)

| CI Job | Local Command | Prerequisites | Required? |
|--------|---------------|---------------|-----------|
| **nightly** | `cargo +nightly nextest run --locked --all-features` + `cargo +nightly test --locked --all-features --test bdd` | nightly, cargo-nextest | Optional |
| **discover-miri** | `cargo metadata` + jq | jq | -- |
| **miri** | Per-service command (see below) | nightly + miri component | Optional |
| **update** | `cargo +beta update && cargo +beta nextest run --locked --all-features` + `cargo +beta test --locked --all-features --test bdd` | beta, cargo-nextest | Optional |

Miri services are discovered dynamically via `[package.metadata.miri]` in each
service's `Cargo.toml`. To list them:

```bash
cargo metadata --format-version 1 --no-deps | \
  jq -r '.packages[] | select(.metadata.miri.command) | "\(.name): \(.metadata.miri.command)"'
```

Current services and their commands:
- **filemonitor**: `cargo miri test -p filemonitor`
- **phd2-guider**: `cargo miri test -p phd2-guider`
- **ppba-driver**: `cargo miri test -p ppba-driver`
- **qhy-focuser**: `cargo miri test -p qhy-focuser`

> **Note:** Miri only runs on push to main (not on PRs) and requires
> `MIRIFLAGS="-Zmiri-disable-isolation"`. A clean build (`cargo clean`) is
> recommended before running miri to avoid stale artifact issues.

---

### ConformU Quick Start

```bash
# Install ConformU (first time only)
./scripts/test-conformance.sh --install-conformu

# Run conformance tests
./scripts/test-conformance.sh

# Run with custom options
./scripts/test-conformance.sh --port 12345 --verbose --keep-reports
```

---

## Quick Reference

Minimum pre-push checks (copy-paste):

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --locked --all-features --all-targets
cargo test --locked --all-features --test bdd
cargo test --locked --all-features --doc
```

---

## Conditional Compilation Notes

- The `mock` feature is used by ppba-driver and qhy-focuser for integration
  testing (including ConformU). It is not required for normal builds.
- Feature powerset checks (`cargo hack --feature-powerset`) test all
  combinations of feature flags to verify features are additive -- this is
  important for feature unification in workspaces.

---

## Agent-Specific Notes

**Claude Code** users can run the full quality-gate suite via the `/pre-push`
slash command:

```
/pre-push          # All checks except miri
/pre-push miri     # All checks including miri
```

This command delegates to `act` with task-based parallelism.

---

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

### Configuration files for act
- `.actrc`: act configuration (Docker images, settings)
- `.env`: Environment variables for workflows

### Tips
1. **First run takes longer**: Docker images need to be downloaded
2. **Use specific jobs**: Running entire workflows can be slow
3. **Check formatting first**: `cargo fmt --check` is the fastest check
4. **Memory usage**: Some jobs (like miri) require significant memory

---

## References

- [AGENTS.md](../AGENTS.md) -- Rule 4 (build, test, fmt before committing)
- [Testing skill](testing.md) -- Writing and organizing tests
- `.github/workflows/` -- Workflow YAML files
- [GitHub Actions act](https://github.com/nektos/act) -- Local CI runner
