# Skill: Babysitting Pull Requests

## When to Read This

- After opening a pull request that must reach merge readiness
- When asked to "babysit" a PR
- When addressing Copilot (or human) review comments on an open PR

## What "merge-ready" means

A babysat PR is done when **all** of these hold at the same time, on the
latest push:

1. **CI fully green** — every required check plus any path-triggered
   workflow the PR woke up (e.g. `msi.yml` on packaging changes). A slow
   leg still running means not done.
2. **A quiet Copilot round** — the most recent requested review produced
   zero new comments. A quiet round only counts if nothing was pushed
   after it; any later push (docs included) needs one more round.
3. **Every review thread has a reply** (see the loop below).
4. **No merge conflicts** (`gh pr view <n> --json mergeable`).

Then report merge readiness and stop. Merging is the repo owner's
decision and action — never merge the PR yourself, and all work stays
on the feature branch, never on `main` (rule 5).

## The loop

Request the first Copilot round immediately after opening the PR, then
iterate:

1. **Watch** CI (`gh pr checks <n>`), mergeability, and new review
   comments (`gh api 'repos/{owner}/{repo}/pulls/<n>/comments'` — gh
   fills the `{owner}`/`{repo}` placeholders from the current repo).
2. **CI failure** → reproduce and fix locally; run the full quality gate
   (rule 4) before every push.
3. **Merge conflict** → merge `origin/main` into the branch (don't
   rebase a branch that has review history), resolve, gate, push.
   Conflict resolution can also import upstream scope changes — re-read
   what landed on `main`, don't just take "ours".
4. **Triage every new comment honestly:**
   - Legitimate (even partially) → fix it.
   - Factually wrong → decline **in the reply**, with evidence: a code
     pointer, doc link, or reproduction. Wrong comments still get
     replies.
   - Never fix silently, never ignore. If the same wrong claim keeps
     recurring, consider making the code or docs unambiguous instead of
     re-litigating — often cheaper than another round.
5. **Push the fixes** (commit author per rule 6).
6. **Reply on every thread** — what changed plus the commit SHA, or why
   declined — *before* requesting the next round:

   ```sh
   gh api 'repos/{owner}/{repo}/pulls/<n>/comments/<comment-id>/replies' \
       -X POST -f body="Fixed in <sha> — <what changed>."
   ```

7. **Request the next Copilot round:**

   ```sh
   gh api 'repos/{owner}/{repo}/pulls/<n>/requested_reviewers' \
       -X POST -f 'reviewers[]=copilot-pull-request-reviewer[bot]'
   ```

   (`gh pr edit --add-reviewer Copilot` does **not** work; use the API
   call above.)

Repeat until the exit criteria hold. Comments from human reviewers go
through the same loop, except: when inclined to decline, ask the
reviewer rather than unilaterally closing the discussion.

## Pacing — watch, don't sleep

Babysitting MUST be event-driven: run a **background watcher** — a loop
that polls the PR cheaply (every ~60–90 s) and exits on the first
actionable event — then act on what it reports. Never sleep for a
guessed interval, and never assume a leg's duration from memory: when a
duration matters, measure it (`gh run list --workflow=<wf>.yml` shows
real run times).

A watcher exits on whichever comes first: a **new Copilot review**
beyond the round count it started with, any **check failed**, or **no
checks pending**. The shape:

```sh
# watch-pr.sh <pr-number> <copilot-round-baseline>
# ($1 must be the numeric PR id: the gh api call below cannot take a URL/branch)
while :; do
  rounds=$(gh api --paginate "repos/{owner}/{repo}/pulls/$1/reviews" \
    | jq -s '[.[][] | select(.user.login == "copilot-pull-request-reviewer[bot]")] | length')
  failed=$(gh pr checks "$1" --json bucket --jq '[.[] | select(.bucket == "fail")] | length')
  pending=$(gh pr checks "$1" --json bucket --jq '[.[] | select(.bucket == "pending")] | length')
  if [ "$failed" -gt 0 ]; then
    sleep 15  # a job re-run's attempt switch can transiently surface the prior attempt's fail
    failed=$(gh pr checks "$1" --json bucket --jq '[.[] | select(.bucket == "fail")] | length')
    [ "$failed" -gt 0 ] && { echo "check failed"; exit 0; }
  fi
  [ "$rounds" -gt "$2" ]  && { echo "new Copilot round"; exit 0; }
  [ "$pending" -eq 0 ]    && { echo "no checks pending"; exit 0; }
  sleep 60
done
```

(`--json bucket` is the machine contract — normalized
`pass`/`fail`/`pending`/`skipping`/`cancel` buckets; never parse the
human-formatted table.)

Run it via your harness's background-task facility (or `&` + `wait`) so
the wait costs nothing and reaction time is one poll interval.

Reference durations — for recognizing a stuck leg, never for sleeping:

- Copilot rounds land ~5–10 minutes after the request.
- `bazel.yml` legs finish in ~4–10 minutes on a typical PR diff, on
  **all three platforms** — the remote cache limits work to the
  affected targets. Only a cold or invalidated cache, or a graph-wide
  change (a dep bump), pushes them past that.
- `windows-latest` **packaging** legs (`msi.yml`) are the true long
  pole at 40–90 minutes. That number applies to packaging workflows
  only — do not transfer it to the bazel test legs.

Don't request a Copilot round on code that is about to change again.
Draft PRs don't get Copilot auto-review; request it explicitly (same
API call) once the PR is ready.

## Triage guidance

Copilot is often right about edge cases (silent fall-throughs, masked
errors, hard-coded values that will drift) and often wrong about
repo-specific facts (labels, conventions, what other files already do).
Verify every claim against the code before acting on it — in both
directions: don't dismiss a real bug because the comment reads pedantic,
and don't "fix" working code because the comment sounds confident.
