---
allowed-tools:
  - Bash(run pre-push checks)
  - Read
  - Glob
  - Grep
---

Run the full pre-push quality-gate suite. Execute every step below **in order**.
Do NOT stop on the first failure — run all steps and report a summary at the end.

Use `docs/pre-push-checklist.md` as the authoritative reference for what each
CI workflow expects.

## Context

Current branch: `!git branch --show-current`
Git status: `!git status --short`

## Steps

### 1. Verify branch

Confirm the current branch is NOT `main`. If it is, **stop immediately** and
tell the user to create a feature branch first (CLAUDE.md Rule 5).

### 2. Format check

```
cargo fmt --check
```

### 3. Clippy (all feature combinations)

First check if `cargo-hack` is installed:

```
cargo hack --version
```

- **If cargo-hack is available:**
  ```
  cargo hack --feature-powerset clippy --all-targets -- -D warnings
  ```
- **If cargo-hack is NOT available:**
  ```
  cargo clippy --all-targets --all-features -- -D warnings
  ```
  Warn the user that feature-powerset clippy was skipped.

### 4. Feature powerset compilation check

Only if cargo-hack is installed:

```
cargo hack --feature-powerset check
```

If cargo-hack is not installed, skip and note it in the summary.

### 5. Tests

Run both target tests and doc tests:

```
cargo test --locked --all-features --all-targets
cargo test --locked --all-features --doc
```

### 6. ConformU conformance tests

Discover services dynamically — do NOT hardcode service names:

```
cargo metadata --format-version 1 --no-deps | jq -r '.packages[] | select(.metadata.conformu.command) | "\(.name)|\(.metadata.conformu.command)"'
```

For each discovered service, run its command. If `jq` is not installed or no
services are found, skip and note it in the summary.

### 7. Sanitizers (optional — nightly only)

Check if nightly is available:

```
rustup run nightly rustc --version
```

If nightly is available, run:

**Address sanitizer:**
```
ASAN_OPTIONS="detect_odr_violation=0:detect_leaks=0" RUSTFLAGS="-Z sanitizer=address" cargo +nightly test --lib --tests --all-features --target x86_64-unknown-linux-gnu
```

**Leak sanitizer:**
```
LSAN_OPTIONS="suppressions=lsan-suppressions.txt" RUSTFLAGS="-Z sanitizer=leak" cargo +nightly test --all-features --target x86_64-unknown-linux-gnu
```

If nightly is not available, skip and note it in the summary.

### 8. Summary

Print a summary table showing each step and its result (pass/fail/skipped).
Include the total count of passes, failures, and skips.
