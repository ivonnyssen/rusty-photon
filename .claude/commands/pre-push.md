---
allowed-tools:
  - Bash(act:*)
  - Bash(git branch:*)
  - Bash(cargo fmt:*)
  - Bash(cargo build:*)
  - Bash(cargo test:*)
  - Bash(cargo clippy:*)
---

Run the full pre-push quality-gate suite by delegating to `act` (local GitHub
Actions runner). Do NOT stop on the first failure — run all jobs and report a
summary at the end.

## Context

Current branch: `!git branch --show-current`
Arguments: $ARGUMENTS

## Steps

### 1. Verify branch

Confirm the current branch is NOT `main`. If it is, **stop immediately** and
tell the user to create a feature branch first (CLAUDE.md Rule 5).

### 2. Run CI workflow jobs via `act`

Launch **all independent jobs in parallel** using background Bash commands,
then run sequential (dependent) jobs after the parallel group completes.

**Parallel group** (all independent — launch simultaneously):

```
act -W .github/workflows/check.yml -j fmt
act -W .github/workflows/check.yml -j clippy
act -W .github/workflows/check.yml -j hack
act -W .github/workflows/test.yml -j required
act -W .github/workflows/test.yml -j coverage
act -W .github/workflows/safety.yml -j sanitizers
act -W .github/workflows/scheduled.yml -j nightly
act -W .github/workflows/scheduled.yml -j update
```

Wait for all parallel jobs to finish before proceeding.

**Sequential group** (depend on discovery jobs — run after parallel group):

```
act -W .github/workflows/check.yml -j discover-msrv -j msrv
act -W .github/workflows/conformu.yml -j discover -j conformu
```

Run the two sequential chains in parallel with each other (each chain runs its
jobs in order internally via act's dependency resolution).

**Optional — only when `$ARGUMENTS` contains `miri`:**

```
act -W .github/workflows/scheduled.yml -j miri
```

**Notes:**
- Skip `test.yml` `os-check` job entirely (act runs Linux Docker containers).
- For `conformu.yml`, act will run the ubuntu variant only.

### 3. Report summary

Print a summary table showing each job and its result (pass/fail/skipped).
Include the total count of passes, failures, and skips.

### 4. Evaluate failures and fix

If **all jobs passed**, congratulate the user and stop here.

If **any jobs failed**, do the following for each failure:

1. **Diagnose** — Read the `act` output for the failed job carefully. Identify
   the specific error: compiler error, test failure, lint warning, formatting
   issue, timeout, missing dependency, etc.

2. **Investigate** — Read the relevant source files, workflow files, and test
   output to understand the root cause. Use Grep and Glob to find related code.
   Consider whether the failure is:
   - A **code issue** (compiler error, test logic, clippy lint) → fix the source
   - A **workflow/act compatibility issue** (missing env var, GitHub-only action,
     token requirement) → fix the workflow file with an `act` fallback using
     `if: env.ACT == 'true'` / `if: env.ACT != 'true'` conditionals
   - A **timing/flakiness issue** (timeout, race condition under sanitizers) →
     increase timeouts or add retry logic as appropriate

3. **Fix** — Apply the minimal fix. Follow CLAUDE.md rules:
   - Do NOT over-engineer — fix only what is broken
   - Run `cargo fmt` after any Rust source changes
   - Run `cargo build --all --quiet --color never` to verify compilation
   - Run `cargo test --all --quiet --color never` to verify tests pass locally

4. **Re-run failed jobs only** — Re-run just the `act` jobs that previously
   failed. Launch them in parallel if there are multiple failures. Do NOT re-run
   jobs that already passed.

5. **Repeat** — If re-runs still fail, go back to step 1 for those jobs.
   After **3 unsuccessful fix attempts** for the same job, stop and report the
   issue to the user with your diagnosis and what you've tried.

6. **Final summary** — Once all jobs pass (or max retries reached), print an
   updated summary table showing the final status of every job.
