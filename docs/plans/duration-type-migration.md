# Plan: Migrate config duration fields to `std::time::Duration`

## Background

The previous duration-naming-convention work normalised the names of
`u64`/`u32` duration fields (`_seconds` → `_secs`, no bare `time` /
`timeout`). This plan picks up from that point and considers replacing
the underlying type with `std::time::Duration`.

The codebase already uses `Duration` everywhere at runtime — any time we
call `tokio::time::sleep`, set a `reqwest` timeout, or build a polling
loop, we wrap the config integer in `Duration::from_secs(...)` /
`Duration::from_millis(...)`. The only place `Duration` is *not* used is
in serde-deserialised config structs, where the type is currently `u64` /
`u32` plus a unit-suffixed name.

The motivation for switching is type safety: a `Duration` field cannot be
silently mixed with a different-unit integer at a call site, and the
`Duration::from_secs()` wrap at every consumer goes away.

## Why a separate PR

- It's a wider refactor (every call site that wraps these fields in
  `Duration::from_*` gets simplified, plus tests need new assertion
  shapes).
- It introduces a new workspace dependency (`serde_with`).
- It changes the documented coding convention in `docs/workspace.md`
  ("always include the unit suffix in the field name") — that rule
  becomes either softened or scoped to "fields that aren't `Duration`".
- The naming-only PR is small, mechanical, and reviewable; bundling
  would obscure what's a rename and what's a real semantic change.

## Approach

Use `serde_with`'s `DurationSeconds<u64>` / `DurationMilliSeconds<u64>`
adapters so the JSON wire format stays a bare integer (no breakage of
existing sample configs, no new format for users to learn). Example:

```rust
use serde_with::{serde_as, DurationSeconds};
use std::time::Duration;

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Phd2Config {
    #[serde_as(as = "DurationSeconds<u64>")]
    #[serde(default = "default_connection_timeout")]
    pub connection_timeout: Duration,
    // …
}

fn default_connection_timeout() -> Duration {
    Duration::from_secs(10)
}
```

The wire format remains `"connection_timeout": 10`. Internal call sites
that previously wrote `Duration::from_secs(cfg.connection_timeout_secs)`
become just `cfg.connection_timeout`.

### Alternative considered: `humantime-serde`

`humantime-serde` lets users write `"5m30s"` strings in JSON. Rejected
because it changes the wire format (string instead of integer), which
would break the PHD2 `SettleParams` use — PHD2's JSON-RPC protocol
requires integer `"time"` / `"timeout"` keys. Sticking with
`DurationSeconds<u64>` keeps PHD2 happy.

### Alternative considered: hand-rolled `#[serde(with = "...")]`

Workable, ~10 lines of helper module per unit. Rejected as a small,
consistent dep is preferable to maintaining serde glue ourselves.

## Naming convention update

Once fields are `Duration`, the unit suffix in the field name is no
longer load-bearing — the type carries the unit. Two options:

1. **Drop the suffix** (`connection_timeout: Duration`,
   `polling_interval: Duration`). Cleaner, but the JSON key changes
   shape (e.g. `connection_timeout_secs` → `connection_timeout`). With
   no external users today, that's safe.
2. **Keep the suffix** to document the JSON representation
   (`connection_timeout_secs: Duration` with `DurationSeconds<u64>`).
   Slightly redundant but minimises churn — the JSON keys land at the
   same names as the rename PR.

Option 1 is preferable long-term. Pick one and update
`docs/workspace.md` § "Duration Units" accordingly:

- The rule "always include the unit suffix in the field name" becomes
  "include the unit suffix in the field name **when the field type is
  not `Duration`**".
- Add a paragraph: "For new code, prefer `std::time::Duration` with a
  `serde_with::DurationSeconds<u64>` / `DurationMilliSeconds<u64>`
  adapter. The unit suffix is then unnecessary — the type carries the
  unit."

## Inventory

The same nine fields covered by the rename PR, post-rename:

| File | Field (post-rename) | Adapter |
|---|---|---|
| `services/phd2-guider/src/config.rs` | `Phd2Config::connection_timeout_secs` | `DurationSeconds<u64>` |
| `services/phd2-guider/src/config.rs` | `Phd2Config::command_timeout_secs` | `DurationSeconds<u64>` |
| `services/phd2-guider/src/config.rs` | `ReconnectConfig::interval_secs` | `DurationSeconds<u64>` |
| `services/phd2-guider/src/config.rs` | `SettleParams::time_secs` | `DurationSeconds<u64>` |
| `services/phd2-guider/src/config.rs` | `SettleParams::timeout_secs` | `DurationSeconds<u64>` |
| `services/filemonitor/src/lib.rs` | `FileConfig::polling_interval_secs` | `DurationSeconds<u64>` |
| `services/qhy-focuser/src/config.rs` | `SerialConfig::timeout_secs` | `DurationSeconds<u64>` |
| `services/ppba-driver/src/config.rs` | `SerialConfig::timeout_secs` | `DurationSeconds<u64>` |
| `services/sentinel/src/config.rs` | `MonitorConfig::AlpacaSafetyMonitor::polling_interval_secs` | `DurationSeconds<u64>` |

(One additional candidate exists today as `polling_interval_ms` in
`qhy-focuser/src/config.rs:21`. It already uses `_ms` and complies, but
when migrating to `Duration` it would use `DurationMilliSeconds<u64>`.)

## PHD2 wire-protocol check

`SettleParams::time_secs` and `SettleParams::timeout_secs` feed the
`json!` macro at `services/phd2-guider/src/client.rs:373-374, 466-467,
917-918` to construct the PHD2 settle payload. After migration, those
calls need to convert back to integer seconds:

```rust
"time": settle.time_secs.as_secs(),
"timeout": settle.timeout_secs.as_secs(),
```

That's a single-token edit at each site, but easy to miss — add it to
the PR checklist.

## Stepwise plan

1. Add `serde_with` (latest stable 3.x) to `[workspace.dependencies]` in
   the root `Cargo.toml` per CLAUDE.md rule 10. Re-run
   `CARGO_BAZEL_REPIN=1 bazel mod tidy` to refresh `MODULE.bazel.lock`.
2. Decide naming policy (Option 1 — drop suffix — recommended) and
   update `docs/workspace.md` § "Duration Units" in the same PR.
3. For each service's config.rs:
   - Pull in `use serde_with::{serde_as, DurationSeconds};` and
     `use std::time::Duration;`.
   - Annotate the struct with `#[serde_as]`, change the field type to
     `Duration`, and add `#[serde_as(as = "DurationSeconds<u64>")]`.
   - Update `default_*` fns to return `Duration::from_secs(N)`.
4. Update all call sites that previously wrapped the field in
   `Duration::from_secs(...)` — drop the wrap. A grep for
   `Duration::from_secs.*\.connection_timeout` etc. will find them.
5. Update the PHD2 `client.rs` `json!` calls per the section above.
6. Update tests:
   - Equality assertions need `Duration::from_secs(N)`, not bare
     integers.
   - Sample config tests round-trip via `serde_json::from_str::<Config>`
     and back.
7. Sample `config.json` files:
   - `DurationSeconds<u64>` keeps the **integer values** unchanged in
     JSON — no value rewrites.
   - The **JSON key names** are decided by the naming choice above:
     - Option 1 (drop suffix) → keys change in lockstep
       (`connection_timeout_secs` → `connection_timeout`); update every
       sample `config.json`.
     - Option 2 (keep suffix) → keys stay identical to the rename PR;
       sample configs untouched.
8. Run `cargo rail run --merge-base -q -- --color never` and
   `cargo fmt --all`. Fix anything red.

## Acceptance

- New workspace dep `serde_with` is in `[workspace.dependencies]`.
- All nine target fields are typed `Duration`.
- No `Duration::from_secs(cfg.<field>)` patterns remain — direct field
  access only.
- `docs/workspace.md` § "Duration Units" reflects the new policy.
- `cargo rail run --merge-base -q -- --color never` is green.
- All sample `config.json` files still deserialise — value formats are
  unchanged; key names only change if Option 1 (drop suffix) is chosen,
  in which case every sample is updated in the same PR.

## Out of scope

- Migrating runtime-only `Duration` usages (already `Duration` today).
- Adding a `humantime` representation as an opt-in alternative — punt
  to a separate decision if user-facing string durations become
  desirable.
