---
allowed-tools:
  - Bash(gh:*)
  - Bash(git:*)
  - Bash(cargo:*)
  - Bash(bazel:*)
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
4. Between events, pace checks to what is actually pending (Copilot
   rounds ~5–10 min; windows-latest legs 40–90 min) rather than busy
   polling. For unattended babysitting, wrap this command in `/loop`.
5. When the exit criteria hold, report merge readiness — checks, review
   rounds, thread status — and stop. Never merge the PR yourself.
