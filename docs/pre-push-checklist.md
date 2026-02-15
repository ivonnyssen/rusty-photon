# Pre-Push Checklist

Local equivalents for every CI quality gate. Run these before pushing to avoid
surprises on GitHub Actions.

## Recommended: Run via `act`

The easiest way to run the full CI suite locally is with `act`, which executes
the actual GitHub Actions workflows in Docker containers:

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

Or use the Claude Code command which does this automatically:

```
/pre-push          # All checks except miri
/pre-push miri     # All checks including miri
```

> **Note:** `act` runs Linux Docker containers, so the `os-check` job
> (macOS/Windows) is skipped locally. Multi-OS `conformu` jobs run the ubuntu
> variant only.

---

## Fallback: Raw `cargo` Commands

When Docker or `act` is unavailable, use these cargo commands directly.

### Quick Reference

Copy-paste block for the common case (stable toolchain, cargo-hack installed):

```bash
cargo fmt --check
cargo hack --feature-powerset clippy --all-targets -- -D warnings
cargo hack --feature-powerset check
cargo test --locked --all-features --all-targets
cargo test --locked --all-features --doc
```

If you do **not** have `cargo-hack`:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --locked --all-features --all-targets
cargo test --locked --all-features --doc
```

---

## Detailed Breakdown by Workflow

### check.yml

| CI Job | CI Command | Local Equivalent | Prerequisites | Required? |
|--------|-----------|------------------|---------------|-----------|
| **fmt** | `cargo fmt --check` | `cargo fmt --check` | stable rustfmt | Yes |
| **clippy (stable)** | clippy-action (stable) | `cargo clippy --all-targets --all-features -- -D warnings` | stable clippy, cfitsio | Yes |
| **clippy (beta)** | clippy-action (beta) | `cargo +beta clippy --all-targets --all-features -- -D warnings` | beta toolchain, cfitsio | Optional |
| **hack** | `cargo hack --feature-powerset check` | `cargo hack --feature-powerset check` | cargo-hack, cfitsio | Yes |
| **msrv** | `cargo msrv verify` per service | `cargo msrv verify --manifest-path services/<name>/Cargo.toml` | cargo-msrv, cfitsio | Optional |

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

| CI Job | CI Command | Local Equivalent | Prerequisites | Required? |
|--------|-----------|------------------|---------------|-----------|
| **required (stable)** | `cargo test --locked --all-features --all-targets` | Same | stable, cfitsio | Yes |
| **required (stable, doc)** | `cargo test --locked --all-features --doc` | Same | stable, cfitsio | Yes |
| **required (beta)** | Same commands with beta | `cargo +beta test --locked --all-features --all-targets` | beta toolchain | Optional |
| **os-check** | Tests on macOS/Windows | N/A (cross-platform) | — | CI-only |
| **coverage** | `cargo llvm-cov --locked --all-features --lcov` | Same | cargo-llvm-cov, llvm-tools-preview | Optional |

### safety.yml

| CI Job | CI Command | Local Equivalent | Prerequisites | Required? |
|--------|-----------|------------------|---------------|-----------|
| **address sanitizer** | `cargo test --lib --tests --all-features --target x86_64-unknown-linux-gnu` with `RUSTFLAGS="-Z sanitizer=address"` | Same (see below) | nightly, llvm, cfitsio | Optional |
| **leak sanitizer** | `cargo test --all-features --target x86_64-unknown-linux-gnu` with `RUSTFLAGS="-Z sanitizer=leak"` | Same (see below) | nightly, cfitsio | Optional |

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

| CI Job | CI Command | Local Equivalent | Prerequisites | Required? |
|--------|-----------|------------------|---------------|-----------|
| **discover** | `cargo metadata` + jq | Same | jq | — |
| **conformu** | Per-service command from metadata | Same (see below) | ConformU, cfitsio | Optional |

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

| CI Job | CI Command | Local Equivalent | Prerequisites | Required? |
|--------|-----------|------------------|---------------|-----------|
| **nightly** | `cargo test --locked --all-features --all-targets` (nightly) | `cargo +nightly test --locked --all-features --all-targets` | nightly, cfitsio | Optional |
| **discover-miri** | `cargo metadata` + jq | Same | jq | — |
| **miri** | `cargo miri test -p <service>` (per service) | `cargo +nightly miri test -p <service>` | nightly + miri component | Optional |
| **update** | `cargo update && cargo test` (beta) | `cargo +beta update && cargo +beta test --locked --all-features --all-targets` | beta | Optional |

Miri services are discovered dynamically via `[package.metadata.miri]` in each
service's `Cargo.toml`. To list them:

```bash
cargo metadata --format-version 1 --no-deps | \
  jq -c '[.packages[] | select(.metadata.miri) | .name]'
```

Current services: filemonitor, phd2-guider, ppba-driver, qhy-focuser

> **Note:** Miri only runs on push to main (not on PRs) and requires
> `MIRIFLAGS="-Zmiri-disable-isolation"`. A clean build (`cargo clean`) is
> recommended before running miri to avoid stale artifact issues.

---

## Prerequisites

### Required tools

| Tool | Install | Used by |
|------|---------|---------|
| Rust stable | `rustup default stable` | All checks |
| cfitsio | `sudo apt install libcfitsio-dev` (Ubuntu) / `brew install cfitsio` (macOS) | All workspace builds |
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

## Conditional Compilation Notes

- The `filemonitor` crate requires `cfitsio` to compile. If cfitsio is not
  installed, use `-p <package>` to build/test specific packages that don't
  need it.
- The `mock` feature is used by ppba-driver and qhy-focuser for integration
  testing (including ConformU). It is not required for normal builds.
- Feature powerset checks (`cargo hack --feature-powerset`) test all
  combinations of feature flags to verify features are additive — this is
  important for feature unification in workspaces.
