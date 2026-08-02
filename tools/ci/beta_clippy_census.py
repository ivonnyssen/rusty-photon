#!/usr/bin/env python3
"""Aggregate `cargo clippy --message-format=json` output into a per-lint census.

Reads cargo's JSON-lines diagnostics on stdin and writes a JSON census on
stdout, one entry per lint name, ordered by site count. The nightly
`beta / clippy` job feeds that census to a reporting step that keeps one
tracking issue per lint.

Only diagnostics belonging to workspace members are counted: `manifest_path`
must live under `--root`. Registry dependencies are compiled with
`--cap-lints allow` by cargo and should not surface anyway, but a path
dependency outside the workspace would.

Sites are deduplicated on (lint, file, line, column). `--all-targets` compiles
the same source file once per target, so a single offending line is otherwise
reported two or three times over.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

CLIPPY_LINT_INDEX = "https://rust-lang.github.io/rust-clippy/master/index.html"


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        default=os.getcwd(),
        help="workspace root; diagnostics outside it are ignored",
    )
    parser.add_argument(
        "--toolchain",
        default="",
        help="toolchain banner recorded in the census (e.g. `clippy --version`)",
    )
    parser.add_argument(
        "--max-sites",
        type=int,
        default=50,
        help="cap on sites recorded per lint; `count` stays the true total",
    )
    parser.add_argument(
        "--summary",
        help="also write a Markdown table here (for $GITHUB_STEP_SUMMARY)",
    )
    return parser.parse_args(argv)


def primary_span(message: dict) -> dict | None:
    """The span a diagnostic points at: its primary one, else the first."""
    spans = message.get("spans") or []
    if not spans:
        return None
    return next((s for s in spans if s.get("is_primary")), spans[0])


def site_of(span: dict | None, root: Path) -> str | None:
    """Repo-relative `file:line` for a span, or None if it has none usable."""
    if span is None:
        return None
    name = span.get("file_name", "")
    if not name:
        return None
    path = Path(name)
    if path.is_absolute():
        try:
            path = path.relative_to(root)
        except ValueError:
            return None
    return f"{path.as_posix()}:{span.get('line_start', 0)}"


def in_workspace(entry: dict, root: Path) -> bool:
    manifest = entry.get("manifest_path")
    if not manifest:
        return False
    try:
        Path(manifest).resolve().relative_to(root)
    except ValueError:
        return False
    return True


def collect(lines, root: Path) -> dict[str, dict]:
    """Group diagnostics by lint name, deduplicating repeated sites."""
    lints: dict[str, dict] = {}
    for line in lines:
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue
        if entry.get("reason") != "compiler-message":
            continue
        if not in_workspace(entry, root):
            continue
        message = entry.get("message") or {}
        if message.get("level") not in ("warning", "error"):
            continue
        code = (message.get("code") or {}).get("code")
        if not code:
            # Summary lines ("N warnings emitted") and rustc errors carry no
            # lint name; there is nothing to open an issue about.
            continue
        span = primary_span(message)
        key = (
            (span or {}).get("file_name"),
            (span or {}).get("line_start"),
            (span or {}).get("column_start"),
        )
        record = lints.setdefault(
            code, {"lint": code, "seen": set(), "sites": [], "example": message.get("message", "")}
        )
        if key in record["seen"]:
            continue
        record["seen"].add(key)
        site = site_of(span, root)
        if site is not None:
            record["sites"].append(site)
    return lints


def docs_url(lint: str) -> str | None:
    if lint.startswith("clippy::"):
        return f"{CLIPPY_LINT_INDEX}#{lint.removeprefix('clippy::')}"
    return None


def census(lints: dict[str, dict], max_sites: int, toolchain: str) -> dict:
    entries = []
    for record in lints.values():
        sites = sorted(record["sites"])
        entries.append(
            {
                "lint": record["lint"],
                "count": len(record["seen"]),
                "sites": sites[:max_sites],
                "example": record["example"],
                "docs": docs_url(record["lint"]),
            }
        )
    entries.sort(key=lambda e: (-e["count"], e["lint"]))
    return {"toolchain": toolchain, "total": sum(e["count"] for e in entries), "lints": entries}


def markdown(result: dict) -> str:
    if not result["lints"]:
        return "## beta clippy census\n\nNo lints fired. ✅\n"
    rows = "\n".join(
        f"| `{e['lint']}` | {e['count']} | {e['sites'][0] if e['sites'] else '—'} |"
        for e in result["lints"]
    )
    return (
        "## beta clippy census\n\n"
        f"{result['total']} findings across {len(result['lints'])} lints "
        f"on `{result['toolchain']}`.\n\n"
        "| Lint | Sites | First site |\n|---|---:|---|\n"
        f"{rows}\n"
    )


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    root = Path(args.root).resolve()
    result = census(collect(sys.stdin, root), args.max_sites, args.toolchain)
    json.dump(result, sys.stdout, indent=2)
    sys.stdout.write("\n")
    if args.summary:
        Path(args.summary).write_text(markdown(result), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
