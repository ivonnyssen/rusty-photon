---
applyTo: ".github/workflows/**,scripts/**,installer/**,**/pkg/**,**/BUILD.bazel,MODULE.bazel"
---

# Reviewing CI, scripts and packaging

These files are not covered by the compiler or the test suite, so
careful review pays off here more than anywhere else in the repo. It
is also where the most confidently wrong comments have been made.

## Claim tool behavior only with a citation

Do not assert how an external tool behaves from memory. Wrong claims
of this shape have each cost a maintainer a researched rebuttal:
udev rule precedence between `/etc` and `/usr/lib`, systemd unit-name
resolution, rootless container networking, GitHub Actions context
availability per trigger, whether the issues API auto-creates labels,
POSIX shell redirection order, and which flags a platform's `shasum`,
`sha256sum` or PowerShell build accepts.

If a finding depends on such behavior, quote the specific manual page
or documentation section. If you cannot, do not raise it.

Note one such rule, with its citation. GitHub Actions expressions cast
operands of different types to a number before comparing, and a
non-numeric string casts to `NaN`, which compares unequal to
everything (Actions docs, "Evaluate expressions in workflows and
actions" — Operators). So `inputs.flag != 'true'` is true even when
the boolean input is true. Do not recommend comparing a boolean input
against a quoted string.

## What to look for

**Secret leakage.** Tokens or credentials reaching logs through
command echo, `set -x`, `pgrep`/`ps` output, server log tails,
uploaded artifacts, or an error message. Registry or cache
credentials in a URL. Check that added diagnostics redact.

**Ordering and atomicity in publish paths.** An index or manifest
replaced before the artifacts it references are in place, so a
concurrent consumer fetches a hash that does not resolve. Retention
that deletes a generation still referenced by a published index.

**Swallowed failures.** A pipeline whose exit status comes from the
last stage only; `2>/dev/null` that hides the diagnostic the step
exists to produce; a redirect that discards the output being checked;
a loop that continues after a step fails; a verification step whose
comparison can pass vacuously when both sides are empty.

**Injection and quoting.** Untrusted values interpolated into shell,
SQL or a `run:` block; unquoted expansions that word-split on paths
with spaces; `${{ }}` interpolation of PR-controlled text.

**Incomplete wiring.** Adding a service, port, artifact or dependency
requires updating every registration site. Ports appear in several
verification scripts and several documentation tables; packages need
their unit file, install scriptlets and doctor entry; a linked native
library must also be shipped by the package that links it; a new
Bazel target must be reachable from `//...`. Flag the sites the diff
missed — this class of finding has been consistently valuable.

**Version and platform pinning.** Actions pinned to a moving tag,
a checksum pinned for one architecture but not another, a build-host
path baked into a shipped artifact, a URL whose "latest" contents
change under the cache key.

## Scope

Do not ask for work outside the PR's stated purpose, and do not
recommend splitting a PR. Do not report that the PR description and
diff disagree.
