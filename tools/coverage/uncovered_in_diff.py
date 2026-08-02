#!/usr/bin/env python3
"""Report which lines *this change adds* are uncovered, per LCOV.

`codecov/patch` gates a PR on the coverage of the lines it touches, not on the
whole-repo total -- but neither the combined `bazel coverage` report nor the
per-package files from `split_lcov.py` know anything about the diff. This tool
intersects the two: it takes the added/modified lines from `git diff` and the
`DA:` records from an LCOV report, and prints only the added lines with zero
hits. That is the answer to "will `codecov/patch` be happy", available before
pushing and without a Codecov account.

Source paths in an LCOV record (`SF:`) may be absolute, workspace-relative, or
bazel-out-prefixed, so a record is matched to a diffed file by path *suffix*.

A changed first-party `.rs` file with no LCOV record at all is reported
separately: that means nothing instrumented it, which for shipping code is a
stronger finding than a few missed lines. Files Codecov ignores (see
`.github/codecov.yml`) are skipped so this agrees with the gate.

Exit status is 1 when anything is uncovered, so it can be used as a check.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from collections import defaultdict

# `+++ b/path/to/file.rs` -- the post-image path of a diff header. `/dev/null`
# appears for deletions, which have no added lines to cover.
_PLUS_FILE_RE = re.compile(r"^\+\+\+ (?:b/)?(.+)$")

# `@@ -12,3 +14,5 @@ optional section heading`. With --unified=0 the counts are
# exact added-line ranges; a missing count means 1 line.
_HUNK_RE = re.compile(r"^@@ -\S+ \+(\d+)(?:,(\d+))? @@")

# Mirrors `ignore:` in .github/codecov.yml -- keep the two in step.
_IGNORED_RE = re.compile(r"(?:^|/)tests/|(?:^|/)bin/(?:mock|test)_[^/]*\.rs$|(?:^|/)examples/")


def added_lines(base: str, diff_source: "str | None") -> "dict[str, set[int]]":
    """Map each changed file to the set of line numbers the diff adds.

    ``diff_source`` is a path to a unified diff, ``-`` for stdin, or ``None``
    to run ``git diff --unified=0 <base>...HEAD`` here.
    """
    if diff_source is None:
        text = subprocess.run(
            ["git", "diff", "--unified=0", f"{base}...HEAD"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    elif diff_source == "-":
        text = sys.stdin.read()
    else:
        with open(diff_source, "r", encoding="utf-8", errors="replace") as fh:
            text = fh.read()

    added: "dict[str, set[int]]" = defaultdict(set)
    current: "str | None" = None
    for line in text.splitlines():
        file_match = _PLUS_FILE_RE.match(line)
        if file_match:
            path = file_match.group(1).strip()
            current = None if path == "/dev/null" else path
            continue
        hunk_match = _HUNK_RE.match(line)
        if hunk_match and current is not None:
            start = int(hunk_match.group(1))
            count = int(hunk_match.group(2)) if hunk_match.group(2) else 1
            added[current].update(range(start, start + count))
    return added


def coverage_by_path(lcov_paths: "list[str]") -> "dict[str, dict[int, int]]":
    """Map each LCOV ``SF:`` path to its ``line -> hit count`` records."""
    hits: "dict[str, dict[int, int]]" = defaultdict(dict)
    current: "str | None" = None
    for lcov_path in lcov_paths:
        with open(lcov_path, "r", encoding="utf-8", errors="replace") as fh:
            for line in fh:
                if line.startswith("SF:"):
                    current = line[3:].strip()
                elif line.startswith("DA:") and current is not None:
                    number, _, rest = line[3:].strip().partition(",")
                    count = rest.split(",")[0]
                    # A line can appear in several records (generics, inlining);
                    # any record that executed it makes it covered.
                    previous = hits[current].get(int(number), 0)
                    hits[current][int(number)] = max(previous, int(count))
    return hits


def lcov_inputs(path: str) -> "list[str]":
    """Resolve ``path`` to a list of LCOV files (a directory yields its reports)."""
    if os.path.isdir(path):
        return sorted(
            os.path.join(path, name)
            for name in os.listdir(path)
            if name.endswith((".info", ".dat"))
        )
    return [path]


def _records_for(file_path: str, coverage: "dict[str, dict[int, int]]") -> "dict[int, int]":
    """Merge every LCOV record whose source path ends with ``file_path``."""
    merged: "dict[int, int]" = {}
    for source_path, lines in coverage.items():
        if source_path == file_path or source_path.endswith("/" + file_path):
            for number, count in lines.items():
                merged[number] = max(merged.get(number, 0), count)
    return merged


def main(argv: "list[str] | None" = None) -> int:
    parser = argparse.ArgumentParser(
        description="Print the lines a diff adds that no test covers."
    )
    parser.add_argument(
        "lcov",
        help="an LCOV report, or a directory of lcov-<pkg>.info files",
    )
    parser.add_argument(
        "--base",
        default="origin/main",
        help="revision to diff against (default: origin/main)",
    )
    parser.add_argument(
        "--diff",
        default=None,
        help="read a unified diff from this file, or - for stdin, instead of running git",
    )
    args = parser.parse_args(argv)

    reports = lcov_inputs(args.lcov)
    if not reports:
        print(f"error: no LCOV reports found at {args.lcov}", file=sys.stderr)
        return 1

    coverage = coverage_by_path(reports)
    added = added_lines(args.base, args.diff)

    uncovered_total = 0
    uninstrumented: "list[str]" = []
    for file_path in sorted(added):
        if not file_path.endswith(".rs") or _IGNORED_RE.search(file_path):
            continue
        records = _records_for(file_path, coverage)
        if not records:
            uninstrumented.append(file_path)
            continue
        # Lines the diff added that LCOV instrumented and nothing executed.
        # Added lines absent from LCOV are not code (blank, comment, `}`).
        missed = sorted(n for n in added[file_path] if records.get(n) == 0)
        if missed:
            uncovered_total += len(missed)
            print(f"{file_path}: {len(missed)} uncovered added line(s)")
            print(f"  {', '.join(str(n) for n in missed)}")

    for file_path in uninstrumented:
        print(f"{file_path}: no coverage data — nothing instrumented this file")

    if uncovered_total or uninstrumented:
        return 1
    print("every added line is covered")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
