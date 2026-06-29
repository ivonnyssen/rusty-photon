# Phase 1 — Complete the Event Surface (implementation plan)

**Status: COMPLETE (archived 2026-06-14).** Delivered in PR #346. The
operation-event envelope (`EventEnvelope`), the `EventBus` `broadcast`
refactor, and the `*_started` / `*_complete` / `*_failed` triple on every
blocking operation all shipped to `main` and are verified there
(`services/rp/src/events.rs`, the emit sites in
`services/rp/src/mcp/internals.rs` and `services/rp/src/mcp/built_in/`, the
`services/rp/tests/features/operation_events.feature` BDD suite, and
`docs/services/rp.md` §Event Envelope). Parent plan:
[`predictive-deadlines-and-watchdog.md`](predictive-deadlines-and-watchdog.md).

This is the execution breakdown for **Phase 1** of
[`predictive-deadlines-and-watchdog.md`](predictive-deadlines-and-watchdog.md).
The parent plan defines *what* Phase 1 is (§Phase 1, §1.1–1.4); this doc
records *how* it lands: concrete code changes at verified `file:line`
anchors, the test seam, BDD scenarios, doc updates, decisions taken, and
sequencing. Ordered test-first per
[`development-workflow.md`](../../skills/development-workflow.md).

## Status

- **Phase:** 1 of 5 (Event surface). **Blocking** for Phases 2–5.
- **Scope:** one PR.
- **Branch:** `worktree-predictive-deadlines-and-watchdog`, branched from
  `main` @ `3aa2d72`.
- **Progress (2026-05-31):** Steps 0–4 landed; Phase 1 complete.

  | Step | State | Commit |
  |---|---|---|
  | (plan) | done | `ac13be0` |
  | 0 — Doc reconciliation (rp.md §Event Envelope, workspace.md index, parent §1.1 as-built note) | **done** | `e27efe6` |
  | 1 — `EventEnvelope` + `EventBus` broadcast refactor (`events.rs`) + unit tests | **done** | `cdee24b` |
  | 2 — `operation_id` + started/complete/failed triple at each blocking entry point | **done** | this PR |
  | 3 — Per-family unit tests (broadcast seam) + `operation_events.feature` + `event_steps` | **done** | this PR |
  | 4 — New event rows in rp.md table + fmt/rail/build gate | **done** | this PR |

  Gates green through Step 1: `cargo rail --profile commit` (438 tests),
  clippy `-D warnings`, husky pre-commit hook, 4 new `events::` tests.

## Goal

Every blocking MCP tool emits a `*_started` / `*_complete` / `*_failed`
triple wrapped in a uniform **event envelope** that already reserves the
predictive-deadline fields Phase 2 will populate (omitted from the wire
in Phase 1 until then). No behaviour of the existing webhook-delivery
path changes; the envelope is purely additive.

## Starting-state facts (verified against code)

- `services/rp/src/events.rs` (88 lines): `EventBus { plugins:
  Vec<EventPlugin> }`; `emit(&self, event_type: &str, payload: Value)`
  (`events.rs:41`) builds `{event_id, event, timestamp, payload}` and
  fire-and-forget `tokio::spawn`s a 5 s webhook POST per subscribed
  plugin. Uses `chrono::Utc`, `uuid::Uuid`.
- `McpHandler` (`mcp/handler.rs:19`) holds `pub event_bus: Arc<EventBus>`
  (`:21`). All blocking helpers are `pub(crate) async fn` on it.
- **9 existing emit sites / 8 event types** (payloads kept verbatim):
  - `session.rs:71` `session_started`; `:119` / `:151` `session_stopped`
  - `mcp/built_in/auto_focus.rs:133` `focus_started`; `:178`
    `focus_complete`
  - `mcp/built_in/filter_wheel.rs:62` `filter_switch`
  - `mcp/internals.rs:389` `document_persistence_failed`; `:472`
    `exposure_started`; `:668` `exposure_complete`
  - `mcp/built_in/center_on_target.rs:102/135/159`
    `centering_started` / `_iteration` / `_complete`
- **Blocking entry points** (where `operation_id` originates and the
  triple is emitted):

  | Operation | Helper / tool seam | Returns | Today |
  |---|---|---|---|
  | slew | `internals.rs:810 do_slew_blocking` (tool `mount.rs:74 slew` → `slew_inner:91`) | `Result<(f64,f64),String>` | no events |
  | park | `internals.rs:892 do_park_blocking` (tool `mount.rs:247 park` → `park_inner:261`) | `Result<(),String>` | no events |
  | unpark | `mount.rs:275 unpark` (per-driver, multi-step via Actions; **no internals helper**) | tool-level | no events |
  | sync_mount | `internals.rs:941 do_sync_mount` (tool `mount.rs:144`) | instant | no events |
  | move_focuser | `internals.rs:693 do_move_focuser_blocking` | `Result<i32,String>` | no events |
  | capture | `internals.rs:433 do_capture` | `Result<(String,String),String>` | emits `exposure_started`/`_complete`; **add `_failed`** |
  | plate_solve | `internals.rs:1007 do_plate_solve` | `Result<DoPlateSolveOutput,String>` | no events |
  | center_on_target | `center_on_target.rs:46` | — | emits started/iter/complete; **add `_failed`** |

- **Test seam today:** BDD has a real `WebhookReceiver`
  (`crates/bdd-infra/src/rp_harness/webhook.rs`, `ReceivedEvent{event_id,
  event_type, timestamp, payload, received_at}`),
  `RpWorld::wait_for_events` (`world.rs:266`), `event_delivery.feature`
  (9 scenarios), `event_steps.rs`. **Unit tests** build
  `EventBus::from_config(&[])` → emits are no-ops. There is **no**
  `CountingEventEmitter` analogue to `CountingProgressEmitter`
  (`mcp/progress.rs:135`).

## Decisions

### Decision A — wire key stays `payload`, not `details`

The parent plan's §1.1 JSON examples nest operation data under `details`,
but §Backwards-compatibility (parent lines 688–700) promises webhook
plugins receive "the *same* `payload` they got before" with "no existing
field changes meaning." The live webhook body, the `WebhookReceiver`, and
`event_delivery.feature` all read **`payload`**.

**Decision:** keep the wire key `payload`; do **not** rename to `details`.
Reconcile the parent plan's §1.1 examples accordingly. Renaming would
break `calibrator-flats` and 9 BDD scenarios for no benefit.

### Decision B — unit-test seam is the broadcast channel, not a new trait

Once `EventBus` owns a `broadcast::Sender` (§1.3), the channel *is* the
unit-test seam: a test calls `event_bus.subscribe()`, exercises the
handler, and drains envelopes from the `Receiver`.

**Decision:** assert via `broadcast::Receiver` — this exercises the real
production path and needs no parallel test-only abstraction. A
`CountingEventEmitter` is kept only as a fallback if a synchronous,
non-async assertion proves necessary.

### Decision C — emit `slew_*` at the helper; let Sentinel filter later

Wiring slew/park at the `do_*_blocking` helper (parent §1.2) means
`center_on_target`'s inner slews (`center_on_target.rs:272`) emit their
own `slew_*` triple. The parent's centering open-question (parent lines
762–768) scopes "track outer only" to *what Sentinel tracks (Phase 4)*,
not what rp emits. Each inner slew carries its own `operation_id`, so the
events are distinguishable.

**Decision:** emit at the helper; Sentinel filters inner vs outer in
Phase 4. Flagged for reviewer.

## Work breakdown (test-first, single PR)

### Step 0 — Design-doc reconciliation (before code)

- `docs/plans/predictive-deadlines-and-watchdog.md` §1.1: reconcile
  `details` → `payload` (Decision A).
- `docs/services/rp.md` §Event System (line 319): document the envelope
  schema (`event_id`, `event_seq`, `operation_id`, `started_at` /
  `ended_at`, `elapsed_ms`, `predicted_duration_ms` / `max_duration_ms`
  omitted until Phase 2, `payload`); add the new `*_started` / `*_failed`
  rows; note the webhook body is additive.
- `docs/workspace.md`: add `predictive-deadlines-and-watchdog.md` to the
  Plans index (currently missing — pre-existing gap).

### Step 1 — Envelope + `EventBus` refactor (`events.rs`)

```rust
pub struct EventEnvelope {            // serializes to the wire body
    pub event_id: String,             // existing per-emission UUID (unchanged meaning)
    pub event_seq: u64,               // NEW monotonic; SSE id in Phase 3
    pub operation_id: Option<String>, // NEW correlation key; None for non-op events
    pub event: String,                // event_type
    pub timestamp: String,            // ISO-8601 (keep)
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub elapsed_ms: Option<u64>,
    pub predicted_duration_ms: Option<u64>, // None in Phase 1 (omitted from JSON)
    pub max_duration_ms: Option<u64>,       // None in Phase 1 (omitted from JSON)
    pub payload: Value,               // inputs (started) + outcome (complete/failed), one key — Decision A
}

pub struct EventBus {
    plugins: Vec<EventPlugin>,
    broadcast: tokio::sync::broadcast::Sender<EventEnvelope>, // capacity 256
    next_seq: std::sync::atomic::AtomicU64,
}

impl EventBus {
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> { self.broadcast.subscribe() }
    pub fn emit(&self, event_type: &str, payload: Value) { /* legacy: operation_id = None */ }
    pub fn emit_operation(&self, ev: EventEnvelope) { /* started / complete / failed */ }
}
```

- `emit()` keeps its signature (legacy events: `session_*`,
  `filter_switch`, `document_persistence_failed`): builds an envelope with
  `operation_id = None`, bumps `next_seq`, sends to **both** the webhook
  list (today's path) and the broadcast channel.
- Lagged consumers log `broadcast::error::RecvError::Lagged(n)`; the
  channel survives.
- **Units** (`workspace.md` §Duration Units): internal time is
  `Duration`; the JSON wire fields are integer-ms with the `_ms` suffix
  (`elapsed_ms`, `*_duration_ms`), matching the "JSON/epoch ms keep the
  suffix" carve-out.

### Step 2 — `operation_id` + triple at each entry point

Pattern (the `_inner` precedent already exists — `slew_inner:91`,
`park_inner:261`):

```rust
let operation_id = Uuid::new_v4().to_string();
let started_at = Utc::now();
self.event_bus.emit_operation(EventEnvelope::started("slew", &operation_id, started_at, details));
let result = /* existing body (or extracted *_inner) */;
match &result {
    Ok(v)  => self.event_bus.emit_operation(EventEnvelope::complete("slew", &operation_id, started_at, outcome_of(v))),
    Err(e) => self.event_bus.emit_operation(EventEnvelope::failed ("slew", &operation_id, started_at, e)),
}
result
```

- Multiple early returns → extract bodies into `*_inner` returning
  `Result`, wrap once (mirrors the existing `slew_inner` / `park_inner`).
- `payload` carries today's keys verbatim — inputs on `*_started`,
  outcome on `*_complete` (slew: `{ra,dec,from_ra,from_dec}` /
  `{actual_ra,actual_dec}`; focuser: target / final position; etc.).
- **sync_mount:** complete / failed only (instant, no `_started`/timer) —
  parent §1.2.
- **unpark:** wire at the tool method `mount.rs:275` (no helper); the
  per-driver multi-step Action wrapped as one operation.
- **capture / center_on_target:** add only the missing `*_failed`;
  migrate the existing emits onto `emit_operation` with a shared
  `operation_id`.

### Step 3 — Tests (write before / with Step 2)

- **Unit** (`mcp/tests.rs`), via Decision B: subscribe to the bus, drive
  each helper with a mock Alpaca (success + failure), assert the triple —
  fresh `event_id` / `event_seq` per emission, `event_seq` monotonic,
  **same `operation_id`** on start↔end, `started_at` / `ended_at`
  present, deadline fields **absent**. Use `result.unwrap()` (AGENTS rule
  7); `tokio::test(start_paused = true)` for timed paths.
- **BDD:** new `services/rp/tests/features/operation_events.feature` —
  per family, a scenario asserting envelope shape over the
  `WebhookReceiver`; extend `event_steps.rs` with envelope-field steps
  (contract constants visible in Gherkin, not hidden in step defs). Keep
  `event_delivery.feature` and the `calibrator-flats` BDD green
  (compat).

### Step 4 — Pre-commit gate

`cargo fmt` + `cargo rail run --profile commit -q` (requires
`cargo-nextest`; `--all-features` so mock-gated tests compile). Final
`cargo build` before push.

## Acceptance → parent §1.4

1. Every blocking MCP tool emits the start / complete / failed triple
   with the new envelope. → Step 2.
2. Each tool's BDD asserts envelope shape (fresh `event_id` +
   `event_seq`, monotonic `event_seq`, `started_at` / `ended_at`, shared
   `operation_id`). → Step 3 BDD.
3. Existing webhook delivery still works (`calibrator-flats` BDD green).
   → Decision A + Step 3 compat.

## Risks

- **Webhook compat** (Decision A) — the one place a careless rename
  breaks real plugins. Mitigation: keep `payload`, additive-only.
- **`broadcast` lag** — no consumer exists in Phase 1; capacity-256 +
  `Lagged` logging avoids surprises when Phase 3 subscribes.
- **Centering event noise** (Decision C) — acceptable; flagged for
  reviewer.
- **`_inner` extraction churn** in `internals.rs` — mechanical but
  touches hot paths; the existing `slew_inner` / `park_inner` precedent
  de-risks it.

## Footprint

~8–12 files: `events.rs`, `internals.rs`, `mount.rs`, `auto_focus.rs`,
`filter_wheel.rs`, `center_on_target.rs`, `session.rs`, `tests.rs`, a new
`operation_events.feature` + `event_steps.rs`, `rp.md`, `workspace.md`.
No new crate deps (`tokio::sync::broadcast` is in-tree).
