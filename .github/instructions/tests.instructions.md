---
applyTo: "**/tests/**,**/*_test.rs,**/test_*.rs,**/*.feature"
---

# Reviewing tests

Auditing tests is one of the highest-yield things you can do on this
repo, because a test that cannot fail looks identical to a passing one
in CI.

## What to look for

**Assertions that cannot fail.** An assertion that holds regardless of
the code under test: comparing a value to itself, asserting a
collection is non-empty when it is seeded non-empty, asserting an
`Option` is `Some` immediately after constructing it, or checking a
field the test itself just set.

**Degenerate fixtures.** A parameter set to the one value that makes
the interesting branch unreachable — a zero offset in a test about
offset handling, a single-element collection in a test about
ordering, an exposure short enough that the timing path never runs.
Say which branch goes unexercised.

**Setup that pre-satisfies the assertion.** State left over from a
previous phase — logs not cleared before asserting on new log lines,
a file present from an earlier step, an environment variable leaking
between tests — so the assertion passes on stale evidence.

**Prose that promises more than the steps do.** In `.feature` files,
a scenario name or feature description claiming coverage that no step
actually exercises.

**Missing negative and boundary cases** where the change introduces a
new failure mode: values at and just past a validated bound, the
disconnected or timed-out path, and the case where an operation is
superseded before it completes.

**Leaks between tests.** Detached tasks or spawned processes not
awaited or killed, temporary directories not cleaned, global or
environment state mutated without restoration, fixed ports or paths
that collide when suites run concurrently.

## Repository conventions

- Prefer failures with clear messages: `result.unwrap()` over
  `assert!(result.is_ok())`. Flag assertions that would report only
  "false is not true" on failure.
- Tests should cover the smallest unit that is still meaningful.
- Production code is never excluded from coverage to make a gate pass;
  only non-shipping mock modules may be excluded. Flag any new
  coverage exclusion on shipping code.
- Gherkin: description lines must not start with a Gherkin keyword
  such as `Rule`, and step text must not contain `|` (the table
  delimiter). Use symbolic names mapped in the step definitions.

## Do not

Do not suggest adding a sleep, retry or readiness loop to stabilize a
test. This repo rejects those as masking real defects; if you believe
there is a race, describe the race in the code under test.

Do not ask for tests in general terms. "This is untested" without
naming the specific behavior that could regress unnoticed is not
actionable, and such comments go unanswered here. Name the input and
the wrong result that would slip through.
