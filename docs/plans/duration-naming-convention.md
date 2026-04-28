# Plan: Align duration field names with the workspace convention

## Background

`docs/workspace.md` § "Coding Conventions / Duration Units" requires:

- `_ms` suffix for sub-second precision.
- `_secs` suffix for human-facing config values (note: `_secs`, not
  `_seconds`).
- Never use a bare `duration` or `timeout` field without a unit suffix.

A grep across `services/*/src/**.rs` and `crates/*/src/**.rs` finds **8
fields across 5 files** that violate the convention. All eight live in
`#[derive(Serialize, Deserialize)]` types, so the renames touch on-disk
sample configs and one PHD2 wire-protocol call site. There are no external
users yet, so straight renames with no compatibility shims are fine.

A separate follow-up — see
[duration-type-migration.md](duration-type-migration.md) — considers
switching these fields to `std::time::Duration` via `serde_with`. That is
intentionally out of scope here; this plan only normalises the existing
`u64`-based names.

## Inventory

### `_seconds` → `_secs`

| File | Line | Field |
|---|---|---|
| `services/phd2-guider/src/config.rs` | 24 | `Phd2Config::connection_timeout_seconds` |
| `services/phd2-guider/src/config.rs` | 26 | `Phd2Config::command_timeout_seconds` |
| `services/phd2-guider/src/config.rs` | 46 | `ReconnectConfig::interval_seconds` |
| `services/filemonitor/src/lib.rs` | 35 | `FileConfig::polling_interval_seconds` |
| `services/qhy-focuser/src/config.rs` | 23 | `SerialConfig::timeout_seconds` |
| `services/ppba-driver/src/config.rs` | 24 | `SerialConfig::timeout_seconds` |
| `services/sentinel/src/config.rs` | 97 | `MonitorConfig::AlpacaSafetyMonitor::polling_interval_seconds` |

### Bare → `_secs`

| File | Line | Field |
|---|---|---|
| `services/phd2-guider/src/config.rs` | 108 | `SettleParams::time` |
| `services/phd2-guider/src/config.rs` | 110 | `SettleParams::timeout` |

## One constraint to be aware of

`SettleParams::time` and `SettleParams::timeout` are also used to build the
PHD2 JSON-RPC `settle` payload at `services/phd2-guider/src/client.rs:
373-374, 466-467, 917-918`. PHD2's wire protocol requires the literal keys
`"time"` and `"timeout"`. The `serde_json::json!` macro takes literal keys,
so renaming the Rust struct fields does not change the wire format —
the `json!` calls just need to read from `settle.time_secs` /
`settle.timeout_secs` instead.

## Stepwise plan

Per service, the loop is:

1. Rename the field in the struct definition.
2. Rename the matching `default_*` fn (e.g. `default_command_timeout` →
   `default_command_timeout_secs`) so reader and constructor stay
   parallel.
3. Update every call site: constructors, `Default` impls, tests,
   doctests, example code in lib.rs module comments.
4. Update sample `config.json` files in lockstep.
5. Update the service's design doc / README if either spells the old
   field name.
6. Run `cargo rail run --merge-base -q -- --color never` and `cargo fmt
   --all` (CLAUDE.md rule 4); fix anything red.

### Per-service notes

- **filemonitor** — one field. Sample configs:
  `services/filemonitor/pkg/config.json`,
  `services/filemonitor/tests/config.json`. Tests in `lib.rs` reference
  the field by name in proptest fixtures (~line 432+); update those too.
- **ppba-driver** — `SerialConfig::timeout_seconds` plus `default_timeout`
  and tests at `config.rs:171, 211, 222`. Sample config:
  `services/ppba-driver/config.json`.
- **qhy-focuser** — same shape as ppba-driver. Sample config:
  `services/qhy-focuser/config.json`. Tests at `config.rs:142, 168`.
- **sentinel** — field lives inside an enum variant; check the
  serialisation tests at `config.rs:280, 313, 341, 354, 377`. Sample
  config: `services/sentinel/examples/config.json`.
- **phd2-guider** — three `_seconds` renames plus `SettleParams`. Touch
  points:
  - `client.rs` `json!` calls — keep wire keys literal, read new field
    names from the struct.
  - Doctest in `lib.rs:22` and any other rustdoc samples.
  - Sample config: `services/phd2-guider/tests/config.json`.

## Out of scope

- Switching to `std::time::Duration` — see
  [duration-type-migration.md](duration-type-migration.md).
- Re-auditing fields that already comply (e.g. `polling_interval_ms`,
  `poll_interval_secs`).

## Acceptance

- `grep -RIn '_seconds\b' services/*/src crates/*/src` returns no struct
  fields.
- `grep -RIn 'pub time:\|pub timeout:\|pub duration:\|pub interval:'
  services/*/src crates/*/src` returns no production-config types.
- `cargo rail run --merge-base -q -- --color never` is green.
- Sample config files round-trip via `serde_json::from_str::<Config>` in
  the existing config tests.
