# rp-plate-solver

rp-managed service that wraps the ASTAP CLI and exposes a narrow HTTP
solve API to `rp`. Operator installs ASTAP separately (BYO per ADR-005);
`rp-plate-solver` ships no ASTAP binary or index database.

See [`docs/services/rp-plate-solver.md`](../../docs/services/rp-plate-solver.md)
for the design contract and [`docs/plans/rp-plate-solver.md`](../../docs/plans/rp-plate-solver.md)
for implementation sequencing.

## Status

This crate is at **Phase 2** of its sequenced plan:

- [x] Crate skeleton, `AstapRunner` trait, `.wcs` parser, supervision
      module, `mock_astap` test double.
- [ ] Phase 3 — BDD scenarios.
- [ ] Phase 4 — HTTP server + `main.rs`.

The crate produces a library plus the `mock_astap` test binary today.
The service binary (`main.rs`) lands in Phase 4.

## Operator install (when shipping)

ASTAP is operator-supplied. The
[`install-astap`](../../.github/actions/install-astap/action.yml) GitHub
action is the canonical install recipe — operators can follow the same
per-OS table by hand:

- Linux: download `astap_command-line_version_Linux_*.zip` from
  SourceForge, place `astap_cli` somewhere in `$PATH`.
- macOS: same, plus `xattr -d com.apple.quarantine astap_cli` on first run.
- Windows: download `astap_command-line_version_win64.zip`, extract.

The D05 star database (~100 MB) is required for typical 0.6°–6° FOV
imaging; download from the same SourceForge project.

Configure `rp-plate-solver` to point at the install:

```json
{
  "astap_binary_path": "/opt/astap/astap_cli",
  "astap_db_directory": "/opt/astap/d05"
}
```

Both fields are required; missing or unreadable values exit non-zero so
Sentinel surfaces the misconfiguration.

## Sentinel restart command

Phase 5 wires this up. Operator's Sentinel config will name a per-platform
restart command; on Linux/systemd the typical entry is
`systemctl restart rp-plate-solver`.
