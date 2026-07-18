---
allowed-tools:
  - Bash(gh:*)
  - Bash(git status:*)
  - Bash(git branch:*)
  - Bash(git switch:*)
  - Bash(git fetch:*)
  - Bash(git merge:*)
  - Bash(git add:*)
  - Bash(git commit:*)
  - Bash(git push:*)
  - Bash(git diff:*)
  - Bash(git log:*)
  - Bash(cargo fmt:*)
  - Bash(cargo build:*)
  - Bash(cargo test:*)
  - Bash(cargo clippy:*)
  - Bash(bazel build:*)
  - Bash(bazel test:*)
---

Babysit a pull request to merge readiness: iterate with CI and Copilot
review until CI is fully green, the latest Copilot round produced no new
comments, and every review thread has a reply.

## Context

Current branch: `!git branch --show-current`
PR for this branch: `!gh pr list --head "$(git branch --show-current)" --json number,title,url --jq '.[] | "#\(.number) \(.title) \(.url)"'`
Arguments: $ARGUMENTS

## Steps

1. Resolve the PR: `$ARGUMENTS` if it names one, otherwise the PR for the
   current branch (above). If neither exists, stop and say so.
2. Read `docs/skills/babysitting-prs.md` and run its loop — it defines
   the exit criteria, the reply-per-thread rule, the exact `gh api`
   calls (Copilot re-request does not work via `gh pr edit`), and the
   triage guidance.
3. Fixing anything means the full quality gate before pushing
   (AGENTS.md rule 4) and the commit-author convention (rule 6).
4. Between events, run the background watcher the skill doc mandates
   (§Pacing) — exit on new Copilot round / failed check / no checks
   pending — rather than sleeping on assumed durations. For unattended
   babysitting, wrap this command in `/loop`.
5. When the exit criteria hold, report merge readiness — checks, review
   rounds, thread status — and stop. Never merge the PR yourself.
