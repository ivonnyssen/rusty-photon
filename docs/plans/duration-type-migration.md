# Plan: Migrate config duration fields to `std::time::Duration` (humantime)

## Status

**Done.** Implemented on the `feature/duration-type-migration` branch.
This document is kept as the record of what was decided and why.

## Background

Follow-up to the duration-naming-convention work (PR #96, normalising
`_seconds` → `_secs`). Now that the renames have landed we follow up
by replacing the underlying types with `std::time::Duration`.

The codebase already uses `Duration` everywhere at runtime — every
`tokio::time::sleep`, `reqwest` timeout, and polling loop wraps the
config integer in `Duration::from_secs(...)` /
`Duration::from_millis(...)`. The only place `Duration` was *not* used
was in the serde-deserialised config structs themselves, where the
type was `u64` / `u32` plus a unit-suffixed name.

The motivation for switching is type safety: a `Duration` field cannot
be silently mixed with a different-unit integer at a call site, and
the `Duration::from_secs()` wrap at every consumer goes away.

## Why a separate PR

- It's a wider refactor (every call site that wrapped these fields in
  `Duration::from_*` got simplified, plus tests needed new assertion
  shapes).
- It introduces a new workspace dependency (`humantime-serde`).
- It changes the documented coding convention in `docs/workspace.md`
  ("always include the unit suffix in the field name").
- The naming-only rename PR was small and mechanical; bundling would
  have obscured what's a rename and what's a real semantic change.

## Approach (chosen: humantime + Option C for PHD2)

Use **`humantime-serde`** for every config duration field, so the wire
format becomes a self-describing string (`"30s"`, `"500ms"`,
`"1m30s"`, `"5m"`). Example:

```rust
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Phd2Config {
    #[serde(default = "default_connection_timeout", with = "humantime_serde")]
    pub connection_timeout: Duration,
    // …
}

fn default_connection_timeout() -> Duration {
    Duration::from_secs(10)
}
```

The wire format becomes `"connection_timeout": "10s"` (was
`"connection_timeout_secs": 10`). Internal call sites that previously
wrote `Duration::from_secs(cfg.connection_timeout_secs)` become just
`cfg.connection_timeout`.

Field names drop the unit suffix entirely (`connection_timeout` not
`connection_timeout_secs`) — the type carries the unit, the JSON value
also carries it, so the suffix is redundant.

### Why humantime (not `serde_with::DurationSeconds<u64>`)

The earlier draft of this plan picked `serde_with::DurationSeconds<u64>`
because it kept the wire format as a bare integer and didn't break
PHD2's JSON-RPC settle payload (which requires integer `time` /
`timeout` keys). On reflection that was solving the wrong problem:

- The PHD2 settle payload is a wire format **we don't own** — it
  belongs to PHD2's JSON-RPC protocol. The operator's config file is a
  wire format **we do own** and should optimise for human editing.
- Using one adapter (`DurationSeconds`) for "seconds in JSON" and
  another (`DurationMilliSeconds`) for "milliseconds in JSON" reproduces
  the same `_secs` vs `_ms` ambiguity in code that we were trying to
  remove from field names.
- `humantime` accepts `"30s"`, `"500ms"`, `"1m30s"`, `"2h"` — one
  adapter for every magnitude, the value reads naturally, the unit is
  visible at the operator's editing site.

### PHD2 SettleParams: Option C (config struct + manual wire conversion)

`SettleParams` is special because the same struct serves two roles:
operator-facing config **and** the JSON-RPC payload sent to PHD2 (which
requires integer `time` / `timeout`).

We split the responsibilities at the JSON-RPC site rather than
splitting the struct:

- `SettleParams` itself (in `services/phd2-guider/src/config.rs`) uses
  humantime + `Duration` like every other config struct.
- The `json!` payload constructions in
  `services/phd2-guider/src/client.rs` (5 sites: `start_guiding`,
  `dither`, plus 3 in unit tests) route `settle.time` and
  `settle.timeout` through the local `settle_secs_ceil` helper to
  produce the integer values PHD2 needs. Ceil-rounding (with
  saturation) preserves `"0s"` as `0`, lifts sub-second humantime
  inputs like `"500ms"` to `1` second so they don't silently truncate
  to `0` on the wire, and avoids `u64` overflow at extreme inputs.

This gives us a clean operator config (`"time": "10s"`) while
preserving PHD2 wire-protocol compatibility (`"time": 10`).

## Inventory (post-rename → post-migration)

| File | Before | After |
|---|---|---|
| `services/phd2-guider/src/config.rs` | `Phd2Config::connection_timeout_secs: u64` | `connection_timeout: Duration` |
| `services/phd2-guider/src/config.rs` | `Phd2Config::command_timeout_secs: u64` | `command_timeout: Duration` |
| `services/phd2-guider/src/config.rs` | `ReconnectConfig::interval_secs: u64` | `interval: Duration` |
| `services/phd2-guider/src/config.rs` | `SettleParams::time_secs: u32` | `time: Duration` (+ `.as_secs()` at PHD2 wire site) |
| `services/phd2-guider/src/config.rs` | `SettleParams::timeout_secs: u32` | `timeout: Duration` (+ `.as_secs()` at PHD2 wire site) |
| `services/filemonitor/src/lib.rs` | `FileConfig::polling_interval_secs: u64` | `polling_interval: Duration` |
| `services/qhy-focuser/src/config.rs` | `SerialConfig::timeout_secs: u64` | `timeout: Duration` |
| `services/qhy-focuser/src/config.rs` | `SerialConfig::polling_interval_ms: u64` | `polling_interval: Duration` |
| `services/ppba-driver/src/config.rs` | `SerialConfig::timeout_secs: u64` | `timeout: Duration` |
| `services/ppba-driver/src/config.rs` | `SerialConfig::polling_interval_ms: u64` | `polling_interval: Duration` |
| `services/ppba-driver/src/config.rs` | `ObservingConditionsConfig::averaging_period_ms: u64` | `averaging_period: Duration` |
| `services/sentinel/src/config.rs` | `MonitorConfig::AlpacaSafetyMonitor::polling_interval_secs: u64` | `polling_interval: Duration` |

The original 9-field plan inventory expanded to **12** because we
included the 3 `_ms`-suffixed fields (qhy-focuser polling, ppba-driver
polling, ppba-driver averaging period) for consistency — leaving any
config duration as a raw integer would have undermined the
"all config durations are `Duration`" rule.

The internal `MonitorStatus::polling_interval_ms` field in
`services/sentinel/src/state.rs` is **not** a config field — it's
runtime dashboard state serialised for the dashboard's JS consumer,
which uses it for display arithmetic. It stays as `u64` ms.

## Sample config changes

Every `services/*/{config.json, examples/*.json, tests/config.json,
tests/bdd/...}` got the affected duration values rewritten in
humantime form:

- `"polling_interval_secs": 60` → `"polling_interval": "60s"`
- `"polling_interval_ms": 1000` → `"polling_interval": "1s"`
- `"averaging_period_ms": 300000` → `"averaging_period": "5m"`
- `"connection_timeout_secs": 10` → `"connection_timeout": "10s"`

JSON keys also lose the unit suffix (Option 1, "drop suffix" in the
original plan).

## Acceptance

- New workspace dep `humantime-serde` is in `[workspace.dependencies]`,
  and `MODULE.bazel.lock` was refreshed via
  `CARGO_BAZEL_REPIN=1 bazel mod tidy`.
- All 12 target fields are typed `Duration`.
- No `Duration::from_secs(cfg.<field>)` / `Duration::from_millis(...)`
  patterns remain at the consumer sites — direct field access only.
- `docs/workspace.md` § "Duration Units" reflects the new policy
  (humantime, no suffix on `Duration` config fields).
- `cargo rail run --merge-base -q -- --color never` is green.
- All sample `config.json` files still deserialise; values were
  rewritten to humantime strings.

## Out of scope

- Migrating runtime-only `Duration` usages (already `Duration` today).
- Migrating non-config duration-like fields (epoch milliseconds,
  dashboard state) — those keep their integer + unit-suffix form.
- PHD2's outgoing JSON-RPC `time`/`timeout` integers — protocol-fixed,
  handled at the call site.
