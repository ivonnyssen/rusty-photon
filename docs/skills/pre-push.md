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
| cargo-llvm-cov | `cargo install cargo-llvm-cov` | Local ad-hoc cargo coverage (CI coverage is `bazel coverage`) |
| cargo-rail | `cargo install cargo-rail` | Change detection |
| ConformU | [ivonnyssen/conformu-install](https://github.com/ivonnyssen/conformu-install) | Conformance tests |
| jq | `sudo apt install jq` / `brew install jq` | ConformU & miri discovery |
| llvm | `sudo apt install llvm` | Address sanitizer symbolization |

---

## Procedure

> **Since the Bazel cutover, Bazel is the per-PR gate.** The required checks are
> `bazel / {ubuntu,macos,windows}-latest` (build + test), `bazel coverage`,
> `bazel/cargo target parity`, and the Cargo `stable / fmt` + `stable / clippy`
> lint jobs (Bazel does not run rustfmt/clippy). The Cargo build / test /
> coverage / hack / msrv jobs moved to a **nightly safety net** and no longer
> gate PRs. So the authoritative pre-push is:
>
> ```bash
> bazel build //... && bazel test //...                       # bazel / <os> (build + fast tests)
> bazel test --test_tag_filters=bdd //...                     # BDD suites (needs OmniSim + OMNISIM_PATH)
> bazel coverage --config=coverage //...                      # bazel coverage (needs OmniSim)
> ./scripts/check-bazel-cargo-parity.sh                       # bazel/cargo target parity
> cargo fmt --check                                           # `stable / fmt`
> cargo clippy --all-targets --all-features -- -D warnings    # `stable / clippy`
> ```
>
> `cargo rail run --profile commit -q` remains the fastest local inner loop while
> iterating (it is no longer a CI job — see "Change Detection" below). The `act` /
> raw-cargo steps below reproduce the **nightly** Cargo safety net when you need it.

### Step 1: Run the full CI suite via `act`

`act` executes the actual GitHub Actions workflows in Docker containers. Use it
to reproduce the nightly Cargo safety net (and the PR `fmt`/`clippy` lint jobs)
locally.

```bash
# Run all independent checks in parallel
act -W .github/workflows/check.yml -j fmt &
act -W .github/workflows/check.yml -j clippy &
act -W .github/workflows/check.yml -j hack &
act -W .github/workflows/check.yml -j msrv &
act -W .github/workflows/test.yml -j required &
act -W .github/workflows/test.yml -j coverage &
act -W .github/workflows/safety.yml -j sanitizers &
wait

# Optional: rolling jobs (these only run on main/scheduled, not PRs)
act -W .github/workflows/scheduled.yml -j nightly &
act -W .github/workflows/scheduled.yml -j beta &
act -W .github/workflows/scheduled.yml -j update &
wait
act -W .github/workflows/scheduled.yml -j discover-miri -j miri  # slow
# conformu.yml only triggers on schedule/workflow_dispatch, so act needs
# the workflow_dispatch event explicitly:
act workflow_dispatch -W .github/workflows/conformu.yml -j plan -j conformu  # nightly + on-demand
```

> **Note:** `act` runs Linux Docker containers, so the macOS/Windows jobs
> (`test.yml` `macos` / `windows`) are skipped locally. Multi-OS `conformu` jobs
> run the ubuntu variant only.

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

## Change Detection: cargo-rail (local) + Bazel (CI)

**CI no longer uses cargo-rail.** The per-PR Bazel gate gets change detection for
free from Bazel's content-addressed action graph (only changed targets rebuild;
everything else is a remote-cache hit), and the nightly Cargo safety net always
runs the full `--workspace`.

[cargo-rail](https://github.com/loadingalias/cargo-rail) remains a fast **local**
inner loop. `cargo rail run --profile commit -q` checks `cargo check` +
`cargo nextest` against only the packages your branch touches vs. the merge base.
To preview which packages would be affected:

```bash
cargo rail plan --merge-base -f text
```

---

## Detailed Workflow Breakdown

### check.yml

`fmt` and stable `clippy` run on every PR + push to main (required PR gates,
because Bazel does not run rustfmt/clippy). The beta-clippy, `hack`, and `msrv`
jobs moved off the per-PR path at the Bazel cutover: they run on push to main,
the nightly schedule, and `workflow_dispatch` — skipped on PRs via
`if: github.event_name != 'pull_request'`. ("Off-PR" below = that set.)

| CI Job | Local Command | Prerequisites | Runs |
|--------|---------------|---------------|------|
| **fmt** | `cargo fmt --check` | stable rustfmt | **PR gate** |
| **clippy (stable)** | `cargo clippy --all-targets --all-features -- -D warnings` | stable clippy | **PR gate** |
| **clippy (beta)** | `cargo +beta clippy --all-targets --all-features -- -D warnings` | beta toolchain | Off-PR |
| **hack** | `cargo hack --feature-powerset check` | cargo-hack | Off-PR |
| **msrv** | `cargo msrv verify` | cargo-msrv | Off-PR |

The workspace uses a single MSRV (currently 1.94.1) declared in the root
`Cargo.toml` via `[workspace.package]`. All members inherit it with
`rust-version.workspace = true` **except the four dual-homed FFI crates**
(`qhyccd-rs` 1.85.0, `libqhyccd-sys` 1.68.0, `libzwo-sys` 1.70.0, `zwo-rs` 1.87.0), which
declare explicit lower MSRVs because they publish to crates.io for outside
consumers. Those lower floors cannot be verified in-workspace (the root
`profile.dev` needs Rust ≥ 1.71 and the shared lockfile pins newest deps), so the
in-workspace **msrv** job (`check.yml`) **skips** those four (the wrapper plus its
`sys-crate`, discovered from `[package.metadata.publish-readiness]`) and verifies
only the workspace-MSRV members. The four are instead checked out-of-tree by the
nightly **publish-readiness** workflow — see below and
[docs/plans/publish-readiness-checks.md](../plans/publish-readiness-checks.md).

### test.yml

`test.yml` moved to a nightly schedule (+ push to main + `workflow_dispatch`) at
the Bazel cutover. Bazel (`bazel.yml` + `bazel-coverage.yml`) is the per-PR
build/test/coverage gate, so this is a full-workspace Cargo safety net — there is
no longer a `plan`/cargo-rail narrowing job, and every job runs `--workspace`.

| CI Job | Local Command | Prerequisites | Runs |
|--------|---------------|---------------|------|
| **required (stable)** | `cargo nextest run --locked --workspace --all-features --all-targets` + `cargo test --locked --workspace --all-features --test bdd` | stable, cargo-nextest | Off-PR |
| **required (stable, doc)** | `cargo test --locked --workspace --all-features --doc` | stable | Off-PR |
| **macos / windows** | same, per host OS (Windows runs BDD in one job) | -- | Off-PR |

This workflow no longer collects coverage — `bazel coverage` (bazel-coverage.yml)
is the sole coverage source.

### safety.yml

Nightly + push-to-main + `workflow_dispatch` (never on PRs). No `plan`/cargo-rail
job — both sanitizers run at the workspace level.

| CI Job | Local Command | Prerequisites | Runs |
|--------|---------------|---------------|------|
| **address sanitizer** | See below | nightly, llvm | Off-PR |
| **leak sanitizer** | See below | nightly | Off-PR |

Address sanitizer:

```bash
ASAN_OPTIONS="detect_odr_violation=0:detect_leaks=0" \
RUSTFLAGS="-Z sanitizer=address" \
  cargo +nightly test --workspace --lib --tests --all-features --target x86_64-unknown-linux-gnu
```

Leak sanitizer:

```bash
RUSTFLAGS="-Z sanitizer=leak" \
  cargo +nightly test --workspace --all-features --all-targets --target x86_64-unknown-linux-gnu
```

> **Note:** The sanitizers modify `Cargo.toml` in CI to set `[profile.dev] opt-level = 1`.
> Locally you can either do the same (and revert), or accept slightly different
> behavior. The sanitizer results are still meaningful without the opt-level tweak.

### conformu.yml (rolling)

ConformU runs on a nightly cron (05:30 UTC) and `workflow_dispatch` --
**not on PRs or push**. Conformance regressions are real but rare, and
the matrix is the most expensive workflow we have, so paying for it on
every PR is overkill. The faster `check`/`test` workflows already gate
the unit-level changes that would most often break conformance; the
nightly catches drift, and `workflow_dispatch` covers the "I just
touched the Alpaca interface, run it now" case. A `notify-on-failure`
job opens or updates a `conformu-nightly` labeled tracking issue when
a scheduled run fails.

| CI Job | Local Command | Prerequisites | Required? |
|--------|---------------|---------------|-----------|
| **plan** | `cargo metadata` + jq (see below) | jq | -- |
| **conformu** | Per-service command (see below) | ConformU | Optional |

The `plan` job no longer uses `cargo rail` filtering: nightly + on-demand
runs always exercise every conformu-tagged service. ConformU services are
discovered dynamically via `[package.metadata.conformu]` in each service's
`Cargo.toml`. To list them:

```bash
cargo metadata --format-version 1 --no-deps | \
  jq -r '.packages[] | select(.metadata.conformu.command) | "\(.name): \(.metadata.conformu.command)"'
```

Current services and their commands:
- **filemonitor**: `cargo test -p filemonitor --features conformu --test conformu_integration -- --ignored --nocapture`
- **ppba-driver**: `cargo test -p ppba-driver --features conformu --test conformu_integration -- --ignored --nocapture`
- **qhy-focuser**: `cargo test -p qhy-focuser --features conformu --test conformu_integration -- --ignored --nocapture`
- **sky-survey-camera**: `cargo test -p sky-survey-camera --features conformu --test conformu_integration -- --ignored --nocapture`

### pi-nightly.yml (rolling, self-hosted ARM64)

Runs the workspace build + tests on a Raspberry Pi 5 self-hosted runner
(Linux/ARM64) once per night. The only CI surface that exercises ARM64;
catches arch-specific regressions (atomics, alignment, vendored C deps
like `cfitsio`, cross-arch feature unification) that the x86 GitHub-hosted
runners cannot. **Triggers are deliberately limited to `schedule` and
`workflow_dispatch`** — never `pull_request` or `push` — because the repo
is public and a self-hosted runner accepting PR triggers would let forks
execute arbitrary code on the Pi. See
[docs/skills/raspberry-pi-runner.md](raspberry-pi-runner.md) for the full
security model, setup steps, and decommissioning procedure.

| CI Job | Local Command | Prerequisites | Required? |
|--------|---------------|---------------|-----------|
| **arm64-stable** | Same as scheduled `nightly` but on ARM64 stable: `cargo build --locked --workspace --all-features --all-targets` + `cargo nextest run --locked --all-features --all-targets` + `cargo test --locked --all-features --test bdd` + `cargo test --locked --all-features --doc` | self-hosted ARM64 runner, stable, cargo-nextest | Optional |
| **notify-on-failure** | N/A (CI-only — opens/updates a `pi-nightly` labelled issue when `arm64-stable` fails on a scheduled run) | -- | CI-only |

### publish-readiness.yml (rolling)

Pre-publish verification for the four **dual-homed FFI crates** (`qhyccd-rs` +
`libqhyccd-sys`, `zwo-rs` + `libzwo-sys`) — the published-in-isolation guarantees
the in-workspace `check`/`test` jobs cannot give. Nightly cron (02:30 UTC) +
`workflow_dispatch` + paths-filtered PR/push on the workflow and its script;
**non-blocking** for ordinary PRs (a minimal-versions break usually comes from an
upstream release, not the PR under review). Families are discovered dynamically via
`[package.metadata.publish-readiness]`. A green run is a **release prerequisite**
(see each crate's release runbook). Full design:
[docs/plans/publish-readiness-checks.md](../plans/publish-readiness-checks.md).

| CI Job | Local Command | Prerequisites | Required? |
|--------|---------------|---------------|-----------|
| **plan** | `cargo metadata` + jq (discovers FFI families) | jq | -- |
| **msrv-minimal-versions** | `scripts/verify-publishable-crate.sh <crate> verify` | nightly, cargo-hack, jq, rustup (auto-installs MSRV toolchains); libclang for zwo | Optional |
| **semver-checks** | `cargo semver-checks --package <crate>` | cargo-semver-checks | Optional |
| **docs** | `cargo +nightly docs-rs --package <crate>` | nightly, cargo-docs-rs | Optional |
| **find** (advisory) | `scripts/verify-publishable-crate.sh <crate> find` | cargo-msrv | CI-only (continue-on-error) |
| **notify-on-failure** | N/A (opens/updates a `publish-readiness` issue on scheduled red) | -- | CI-only |

The script copies each crate family OUT of the workspace and builds it on its
declared (lower) MSRV with a `-Z direct-minimal-versions` lockfile generated under
the MSRV-aware resolver (`CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback`) — the
two ingredients that let a low MSRV hold against minimal dependency versions. The
`*_SKIP_NATIVE_LINK` env makes it a check-only, SDK-free build (zwo still needs
libclang for bindgen, not the SDK binary).

### scheduled.yml (rolling)

These jobs only run on push to main, on schedule, or manually -- **not on PRs**.
No change detection is used; everything runs against the full workspace.

| CI Job | Local Command | Prerequisites | Required? |
|--------|---------------|---------------|-----------|
| **nightly** | `cargo +nightly nextest run --locked --all-features` + `cargo +nightly test --locked --all-features --test bdd` | nightly, cargo-nextest | Optional |
| **beta** | Same commands with `+beta` | beta toolchain | Optional |
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
- **rp-auth**: `cargo miri test -p rp-auth`

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

Pre-push checks (copy-paste) — these mirror the full required gate (`bazel / <os>`,
`bazel coverage`, `bazel/cargo target parity`, `stable / fmt`, `stable / clippy`);
`fmt`/`clippy` are the Cargo-only lint jobs Bazel doesn't cover:

```bash
bazel build //... && bazel test //...                     # bazel / <os> (build + fast tests)
bazel test --test_tag_filters=bdd //...                   # BDD suites (needs OmniSim + OMNISIM_PATH)
bazel coverage --config=coverage //...                    # bazel coverage (heavier; needs OmniSim)
./scripts/check-bazel-cargo-parity.sh                     # bazel/cargo target parity
cargo fmt --check                                         # stable / fmt
cargo clippy --all-targets --all-features -- -D warnings  # stable / clippy
```

## Bazel (primary gate)

Bazel is the per-PR build / test / coverage gate (`.github/workflows/bazel.yml`,
`bazel-coverage.yml`, `parity.yml`) per `docs/plans/bazel-migration.md`. The
Cargo build/test jobs moved to a nightly safety net; `Cargo.toml` / `Cargo.lock`
remain the single source of truth for dependency versions.

Pre-push commands (these ARE the gate — run them before pushing):

```bash
bazel build //...
bazel test //...           # filters out tagged `requires-cargo` and `bdd`
```

If you added a crates.io dependency, refresh the Bazel index:

```bash
# 2nd (un-forced) `bazel mod tidy` resets the lock's recorded CARGO_BAZEL_REPIN
# fingerprint to null, so the committed lock doesn't churn on later plain `bazel` runs.
CARGO_BAZEL_REPIN=1 bazel mod tidy && bazel mod tidy
git add MODULE.bazel.lock
```

BDD cucumber tests now build and run under Bazel (Phase 3 complete) but
are still tagged `bdd` and excluded from the default test filter because
the full suite takes ~150 s. Run them explicitly:

```bash
bazel test --test_tag_filters=bdd //...
# Or a single service:
bazel test //services/filemonitor:bdd
```

Coverage runs as a separate required workflow
(`.github/workflows/bazel-coverage.yml`) on every PR. Locally it needs the
pinned nightly toolchain, which `--config=coverage` selects (see `.bazelrc`):

```bash
bazel coverage --config=coverage //...
# Combined lcov: $(bazel info output_path)/_coverage/_coverage_report.dat
```

It runs on the pinned nightly toolchain with `coverage_nightly` set, so the
`#[cfg(test)] mod tests` blocks stay out of the numbers — as do the
feature-gated `mock` transport/client modules, which carry a module-level
`#![cfg_attr(coverage_nightly, coverage(off))]` because they never ship in a
production binary and counting them would inflate the coverage figure with
code that never runs at the telescope. It uploads under the canonical
`<pkg>` Codecov flags that drive the per-service badges, and is the **sole**
coverage source (the Cargo jobs no longer collect coverage). It **includes the BDD suite**
(`--config=coverage` drops only the `requires-cargo` tag), so locally it needs
OmniSim installed and `OMNISIM_PATH` set, the same as a
`bazel test --test_tag_filters=bdd` run. Whether the BDD-spawned service
binaries' coverage is collected is validated in CI — see
[docs/plans/bazel-migration.md](../plans/bazel-migration.md).

Known limitations during migration:
- A few tests in `bdd-infra`, `phd2-guider`, and `filemonitor:test_cli`
  shell out to `cargo` or assume `target/debug` paths; they are tagged
  `requires-cargo` and skipped under Bazel.
- Conformu integration tests and Miri continue to run only under Cargo.

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
- [cargo-rail](https://github.com/loadingalias/cargo-rail) -- Change detection for CI
