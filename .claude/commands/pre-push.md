---
allowed-tools:
  - Bash(act:*)
  - Bash(git branch:*)
  - Bash(cargo fmt:*)
  - Bash(cargo build:*)
  - Bash(cargo test:*)
  - Bash(cargo clippy:*)
  - Task
---

Run the full pre-push quality-gate suite by delegating to `act` (local GitHub
Actions runner). Use a **Task-based agent team** to run jobs in parallel without
overflowing context. Do NOT stop on the first failure — run all jobs and report
a summary at the end.

## Context

Current branch: `!git branch --show-current`
Arguments: $ARGUMENTS

## Architecture

Each `act` job runs inside its own **Task subagent** (subagent_type: `Bash`).
Subagents are **read-only investigators** — they run the job, diagnose any
failure, and return a structured report. They NEVER edit or fix files.

The **main agent** (you) coordinates the team, collects reports, applies fixes,
and re-runs failed jobs. This avoids both context overflow and edit conflicts.

## Steps

### 1. Verify branch

Confirm the current branch is NOT `main`. If it is, **stop immediately** and
tell the user to create a feature branch first (CLAUDE.md Rule 5).

### 2. Launch CI jobs via agent team

Spawn one Task subagent per job (or job chain). Launch all independent agents
simultaneously using the Task tool.

**Parallel group** — launch all of these simultaneously as separate Task agents:

| Agent label         | Command                                                          |
|---------------------|------------------------------------------------------------------|
| fmt                 | `act -W .github/workflows/check.yml -j fmt`                     |
| clippy              | `act -W .github/workflows/check.yml -j clippy`                  |
| hack                | `act -W .github/workflows/check.yml -j hack`                    |
| msrv                | `act -W .github/workflows/check.yml -j discover-msrv -j msrv`   |
| required            | `act -W .github/workflows/test.yml -j required`                 |
| coverage            | `act -W .github/workflows/test.yml -j coverage`                 |
| sanitizers          | `act -W .github/workflows/safety.yml -j sanitizers`             |
| nightly             | `act -W .github/workflows/scheduled.yml -j nightly`             |
| update              | `act -W .github/workflows/scheduled.yml -j update`              |
| conformu            | `act -W .github/workflows/conformu.yml -j discover -j conformu` |

**Optional** — only when `$ARGUMENTS` contains `miri`:

| Agent label         | Command                                                  |
|---------------------|----------------------------------------------------------|
| miri                | `act -W .github/workflows/scheduled.yml -j miri`        |

**Notes:**
- Skip `test.yml` `os-check` job entirely (act runs Linux Docker containers).
- For `conformu.yml`, act will run the ubuntu variant only.

### 3. Subagent prompt template

Use this prompt for every Task subagent (fill in `{COMMAND}` and `{LABEL}`):

```
Run the following CI job and report the result:

  {COMMAND}

Job label: {LABEL}

INSTRUCTIONS:
1. Run the command above. Use a timeout of 600000ms.
2. If the job PASSES (exit code 0), respond with exactly:
   RESULT: PASS
   JOB: {LABEL}
3. If the job FAILS, investigate:
   a. Read the output carefully. Identify the specific error.
   b. Categorize the failure as one of:
      - CODE: compiler error, test failure, clippy lint, formatting issue
      - WORKFLOW: act compatibility issue, missing env var, GitHub-only action
      - FLAKY: timeout, race condition, intermittent failure
   c. For CODE and WORKFLOW failures: read the relevant source or workflow
      files to understand the root cause. Identify exactly which file(s) and
      line(s) need to change and what the fix should be.
   d. Respond with a structured report:
      RESULT: FAIL
      JOB: {LABEL}
      CATEGORY: <CODE|WORKFLOW|FLAKY>
      ERROR_SUMMARY: <one-line description of what went wrong>
      FILES_AFFECTED: <comma-separated list of file paths>
      DIAGNOSIS: <detailed explanation of root cause>
      SUGGESTED_FIX: <specific, actionable fix — include exact code changes
        as a diff or before/after snippet if possible>

IMPORTANT: Do NOT edit or fix any files. You are a read-only investigator.
Only run the act command and read files to diagnose. Return your report.
```

### 4. Collect reports and print summary

Wait for all subagents to complete. Parse their reports and print a summary
table:

```
| Job          | Result | Category | Error Summary          |
|--------------|--------|----------|------------------------|
| fmt          | PASS   | —        | —                      |
| clippy       | FAIL   | CODE     | unused import in foo.rs|
| ...          | ...    | ...      | ...                    |

Total: X passed, Y failed, Z skipped
```

### 5. Evaluate failures and fix

If **all jobs passed**, congratulate the user and stop here.

If **any jobs failed**, apply fixes **yourself** (the main agent):

1. **Review all failure reports together** — Look for overlapping fixes.
   Multiple jobs may fail for the same root cause. Group related failures
   and fix the root cause once rather than applying conflicting patches.

2. **Apply fixes** — Make the minimal changes needed. Follow CLAUDE.md rules:
   - Do NOT over-engineer — fix only what is broken
   - Run `cargo fmt` after any Rust source changes
   - Run `cargo build --all --quiet --color never` to verify compilation
   - Run `cargo test --all --quiet --color never` to verify tests pass locally

3. **Re-run failed jobs only** — Spawn new Task subagents for just the
   previously failed jobs (using the same prompt template). Do NOT re-run
   jobs that already passed.

4. **Repeat** — If re-runs still fail, review the new reports and try again.
   After **3 unsuccessful fix attempts** for the same job, stop and report
   the issue to the user with your diagnosis and what you've tried.

5. **Final summary** — Once all jobs pass (or max retries reached), print an
   updated summary table showing the final status of every job.
