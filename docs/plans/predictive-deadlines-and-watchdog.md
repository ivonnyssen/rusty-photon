# Predictive Operation Deadlines + Sentinel Operation Watchdog

## Status

**Draft — design.** This plan describes how to close the implementation
gap behind two long-standing sections of the rp design doc:

- [`docs/services/rp.md` §Sentinel Watchdog Integration](../services/rp.md#sentinel-watchdog-integration)
- [`docs/services/rp.md` §Real-Time Stream](../services/rp.md#real-time-stream)

Both have lived in the design doc since rp was first sketched; neither
has any code behind them today. The catalyst for picking this up was
the macOS BDD hang investigated on PR #267: a tiny corrective slew in
OmniSim stalled past the hardcoded 300 s `poll_slewing_until_idle`
deadline, which by accident is also rmcp's default session
keep-alive, and the rmcp transport then swallowed the would-be error
into a permanent client wedge. The investigation confirmed that *every
piece* the design doc names — predictive per-operation deadlines, an
event stream, an external watchdog, a corrective-action ladder — is
missing. With them in place the hang would have surfaced as a
fast-failing test rather than a 1 h 36 min cancelled CI job.

Implementation is sequenced in five phases (Phase 1 → Phase 5), one or
two PRs each. Phase 1 lands first because every later phase needs the
events it adds to exist.

## Motivation

The current state of operation supervision:

- `rp` blocks tool calls on hardcoded internal deadlines:
  `services/rp/src/mcp/internals.rs::poll_slewing_until_idle` is
  300 s, `do_park_blocking` is 300 s, the focuser move helper is
  120 s. None scale with the operation's actual workload. A small
  corrective slew of arc-seconds and a meridian-flip-class slew
  across hours share the same 300 s ceiling.
- Three of the five lifecycle event pairs the design names (rp.md
  table at line 2766) are unemitted in code: `slew_started`,
  `slew_complete`, and the per-step events for the primitive
  `move_focuser` / `park` tools that aren't compound. The two that
  are wired (`exposure_*`, `centering_*`, `focus_*`) deliver via
  fire-and-forget POST to webhook URLs declared statically in config
  (`services/rp/src/events.rs`).
- `GET /api/events/subscribe` is documented (rp.md line 2869) but
  has no route handler in `services/rp/src/routes.rs`. There is no
  way for an external supervisor to listen to events in real time.
- `sentinel` is exclusively an Alpaca `SafetyMonitor` poller. Its
  `Monitor` trait has one impl (`AlpacaSafetyMonitor` polling
  `issafe`) and its `Notifier` trait has one impl (`PushoverNotifier`).
  No event subscription. No deadline tracking. No corrective actions.
  `docs/services/sentinel.md` describes only what is built — the
  watchdog story lives entirely in `rp.md`.
- The corrective-action ladder (rp.md line 2774: health check →
  abort → restart → notify) has no code behind it. Even the
  safety-monitor "park on unsafe" reaction described at rp.md
  lines 2737–2747 is not implemented in rp.

What this costs operationally:

1. **A stuck operation eats its full hardcoded ceiling** — 5 minutes
   for slew or park, 2 minutes for focuser — before surfacing an
   error. In a real session that becomes a 5-minute pause that is
   indistinguishable from "still slewing"; in CI it triggers cliffs
   like the macOS BDD hang where the 300 s coincides with rmcp's
   session keep-alive and the failure is swallowed entirely.
2. **There is no fast-fail layer for predictable slew distances.**
   The driver knows its slew rate and the planner knows the
   distance; a 2 ″ corrective slew has no business taking 5 minutes
   to time out.
3. **There is no recovery path for a wedged Alpaca service.** A
   misbehaving driver leaves the operator with a hung MCP call and
   no automated remediation; the human has to notice, log in,
   restart the right process, and reconnect.
4. **There is no external pulse on rp itself.** If rp crashes or
   wedges, nothing notices until the operator looks at a UI.
   Sentinel, the natural place to host that pulse, has no link
   to rp today.

The design's answer to all four is a two-loop structure:

- **Inner loop in rp** — every blocking tool carries a *predicted*
  deadline derived from the operation's parameters (slew distance,
  focuser travel, exposure duration), emits a `*_started` event
  carrying that deadline at the start, and a `*_complete` /
  `*_failed` event at the end.
- **Outer loop in Sentinel** — subscribes to rp's event stream,
  tracks the deadlines independently, escalates through the
  corrective-action ladder on expiry, and uses the stream
  itself as a liveness pulse on rp.

That structure is the entire content of this plan.

## Goals

- Emit start / complete events for every blocking operation rp
  exposes, with a uniform payload schema that includes the
  predicted and maximum durations.
- Replace `rp`'s hardcoded internal deadlines with parameters the
  caller computes from operation characteristics, with sensible
  defaults baked into the tool layer.
- Add `GET /api/events/subscribe` as an SSE endpoint that streams
  every event the `EventBus` emits.
- Add an `OperationDeadlineMonitor` impl to sentinel's `Monitor`
  trait that consumes the stream, tracks per-operation deadlines,
  and dispatches escalations.
- Implement the four-step corrective-action ladder
  (health check → abort → restart → notify) behind a configurable
  per-service policy.
- Make the rp event stream disconnect detectable on the Sentinel
  side so a hung or crashed rp is also a watchdog trigger.

## Non-goals

- A generic timeline / scheduler view of operations in the
  dashboard. The dashboard already exists; adding visualisation is a
  follow-up once events stream.
- Replacing or rewriting Sentinel's notifier surface
  (`Notifier`/`PushoverNotifier`). The watchdog escalation reuses
  the existing notifier dispatch path.
- Plugin-process supervision beyond what rp already does. rp.md
  line 103 mentions "Sentinel may restart configurable plugins" as
  a possibility; that is in scope for the *restart command*
  mechanism Phase 5 builds, but plugin lifecycle proper stays with
  rp.
- Multi-rp deployments. Sentinel can monitor multiple Alpaca
  endpoints today; the watchdog stream can be configured to one
  rp endpoint per Sentinel instance. Multi-rp is a future
  consideration once the single-rp case is real.
- Replacing the existing `do_*_blocking` MCP synchrony with
  fire-and-forget + event-driven completion. The MCP tool surface
  stays synchronous (callers still await responses); events are
  *additional*, not a replacement.
- Fixing the upstream rmcp `StreamableHttpClientTransport`
  silent-hang bug — the client-side `StreamableHttpClient` worker
  treats SSE stream termination as a logged warning and continues
  running, so `peer.call_tool().await` never resolves when the
  server-side session closes mid-call (the pending request id sits
  in the responder pool with no one to fulfil it). See the PR #267
  macOS hang investigation thread for the full trace. That work is
  parallel — once predictive deadlines fire in seconds instead of
  minutes, the rmcp bug becomes much less reachable, but filing it
  upstream is still worth doing. Tracked separately.
- Wrapping `crates/bdd-infra/src/rp_harness/mcp_client.rs::call_tool`
  in `tokio::time::timeout` for defensive purposes. That is a
  one-line test-infra change worth doing on its own merits; it does
  not belong in this design plan.

## Architecture overview

```
                    ┌────────────────────────────────────────┐
                    │                  rp                    │
                    │                                        │
   MCP client ──────┼──► tools/call ── do_*_blocking ───┐    │
                    │                       │           │    │
                    │                       ▼           │    │
                    │              EventBus.emit(*_started,  │
                    │              {predicted_ms, max_ms,    │
                    │              operation_id, …})         │
                    │                       │           │    │
                    │                       ▼           │    │
                    │              EventBus.emit(*_complete  │
                    │              | *_failed,               │
                    │              {operation_id, …})        │
                    │                       │           │    │
                    │                       ▼           │    │
                    │         broadcast::Sender (in-mem)     │
                    │            │                │     │    │
                    │            ▼                ▼     │    │
                    │     plugin webhooks   /api/events/subscribe
                    │     (existing path)   (SSE response)    │
                    └──────────────────────────────────┼──────┘
                                                       │
                                                       │ SSE
                                                       ▼
                    ┌──────────────────────────────────────┐
                    │              sentinel                │
                    │                                      │
                    │  Engine ── OperationDeadlineMonitor ─┼─► tracks open ops
                    │     │                                │   by operation_id,
                    │     │                                │   timer per op
                    │     │                                │
                    │     ├── on deadline expired ────────┼─► CorrectiveAction
                    │     │                                │   (health → abort →
                    │     │                                │    restart → notify)
                    │     │                                │
                    │     ├── on SSE stream disconnect ───┼─► rp wedged
                    │     │                                │   (escalate immediately)
                    │     │                                │
                    │     └── existing AlpacaSafetyMonitor (unchanged)
                    └──────────────────────────────────────┘
```

Two loops, one direction of data flow. The MCP call path is unchanged;
the events are an *additional* stream the supervisor uses to react.

## Phase 1 — Complete the event surface

**One PR. Blocking for every later phase.** Adds the missing
`*_started` / `*_complete` / `*_failed` events with a uniform payload
schema, including the predicted-deadline fields that Phase 2 will
populate (omitted from the wire in Phase 1 — see the §1.1 as-built
note).

### 1.1 Uniform event payload schema

> **As-built note (Phase 1.1 shipped).** The envelope landed in
> `services/rp/src/events.rs` as `EventEnvelope`. The **authoritative
> schema** is [`docs/services/rp.md` §Event Envelope](../services/rp.md#event-envelope).
> Three compat-driven refinements over the sketch below, all following
> from Decision A (preserve the historical webhook keys; see the phase-1
> implementation plan, [`predictive-deadlines-phase1-event-surface.md`](archive/predictive-deadlines-phase1-event-surface.md)):
> (1) the envelope keeps the historical **`event`** field (e.g.
> `"slew_started"`) — there is no separate `operation` field; the
> operation family is the event-name prefix. (2) Both the `*_started`
> inputs and the `*_complete` / `*_failed` outcome live under the
> historical **`payload`** key — there is no separate `details` or
> `outcome` key, because today's `exposure_complete` already ships its
> outcome under `payload` and changing that would break webhook
> consumers. (3) `predicted_duration_ms` / `max_duration_ms` are
> **omitted** when absent (rather than serialized as `null`); Phase 2
> populates them. The sketch below is preserved for design intent.

Define a shared envelope for every operation event. Today's events
have inconsistent shapes (`exposure_started` carries
`{camera_id, target, filter, duration}`; `focus_started` carries
`{focuser_id, position}`). Phase 1 standardises:

```json
{
  "event_id": "f3a8b9c0-1d4e-4a2b-8f3a-2c7d9e1f4b6a",
  "event_seq": 1247,
  "operation_id": "0bbc7e54-c2c2-4e3b-9a8d-7f43a3a8b2f1",
  "operation": "slew",
  "started_at": "2026-05-19T20:14:33.412Z",
  "predicted_duration_ms": 1200,
  "max_duration_ms": 10000,
  "details": { "ra": 12.0, "dec": -30.0, "from_ra": 11.998, "from_dec": -29.991 }
}
```

Three identifiers, each with a distinct job:

- `event_id` — the existing per-emission UUID from
  `services/rp/src/events.rs:42-49`, kept unchanged. This is the
  routing key for the plugin webhook ack/completion contract
  (`POST /api/plugins/{event_id}/complete`); changing its meaning
  would break the existing plugin protocol.
- `event_seq` — a new monotonically increasing per-emission
  counter (an `AtomicU64` on `EventBus`). Used as the SSE `id`
  in Phase 3 so `Last-Event-ID` replay has total order.
- `operation_id` — a fresh UUID per *call*, generated at the entry
  point of every blocking tool. It is the correlation key joining
  `*_started`, `*_complete`, and `*_failed` for the same call.
  Multiple events share one `operation_id`; that's why it cannot
  double as the SSE id.

`predicted_duration_ms` and `max_duration_ms` are nullable in
Phase 1 (the existing tool code doesn't know how to compute them
yet); Phase 2 populates them. `details` is per-operation and
unchanged from today's payload contents — only the envelope is
new.

`*_complete` and `*_failed` mirror the envelope (fresh `event_id`
and `event_seq` per emission, same `operation_id` as the matching
`*_started`) and add an `outcome` block:

```json
{
  "event_id": "9d2e1f3a-4b5c-4d6e-9f7a-8b1c2d3e4f5a",
  "event_seq": 1248,
  "operation_id": "0bbc7e54-…",
  "operation": "slew",
  "started_at": "2026-05-19T20:14:33.412Z",
  "ended_at":   "2026-05-19T20:14:34.601Z",
  "elapsed_ms": 1189,
  "outcome":    { "actual_ra": 12.0, "actual_dec": -30.0 }
}
```

### 1.2 Events to add

| Operation | Add | Notes |
|---|---|---|
| `slew`           | `slew_started`, `slew_complete`, `slew_failed`  | Currently emits nothing. Wire at `services/rp/src/mcp/internals.rs::do_slew_blocking`. |
| `park`           | `park_started`, `park_complete`, `park_failed`  | Wire at `do_park_blocking`. |
| `unpark`         | `unpark_started`, `unpark_complete`, `unpark_failed` | Per-driver — the GTi driver has a multi-step unpark via Actions. |
| `move_focuser`   | `move_focuser_started`, `move_focuser_complete`, `move_focuser_failed` | Wire at the focuser move helper. |
| `sync_mount`     | `sync_mount_complete`, `sync_mount_failed` | Sync is instant per ASCOM — no `_started`/timer needed. |
| `capture`        | `exposure_failed` | `exposure_started` and `exposure_complete` already exist; add the failed variant. |
| `plate_solve`    | `plate_solve_started`, `plate_solve_complete`, `plate_solve_failed` | Wire at the proxy in `services/rp/src/mcp/built_in/plate_solve.rs`. |
| `auto_focus`     | `focus_failed` | `focus_started` / `focus_complete` already exist. |
| `center_on_target` | `centering_failed` | `centering_started` / `centering_complete` / `centering_iteration` already exist. |

Every existing event keeps its current payload contents under
`details`; the envelope is additive.

### 1.3 EventBus refactor

`services/rp/src/events.rs` today owns a `Vec<EventPlugin>` and
fire-and-forget POSTs. Phase 1 widens it:

```rust
pub struct EventBus {
    plugins: Vec<EventPlugin>,
    broadcast: tokio::sync::broadcast::Sender<EventEnvelope>,
    next_seq: std::sync::atomic::AtomicU64,
}
```

The broadcast sender is created with capacity (e.g. 256). Existing
`emit(event_type, payload)` keeps its signature and now also
allocates a fresh `event_id` (UUID, unchanged from today), bumps
`next_seq` to populate `event_seq`, builds the envelope, and
publishes both to the plugin webhook list (today's path) and to
the broadcast channel (Phase 3 subscribes here). Slow broadcast
consumers that fall behind get a
`broadcast::error::RecvError::Lagged(n)` they can log; the channel
keeps running.

### 1.4 Acceptance

- Every blocking MCP tool emits the start / complete / failed
  triple with the new envelope.
- Each tool's BDD coverage adds at least one scenario asserting the
  envelope shape — fresh `event_id` and `event_seq` per emission
  (monotonic across events), the `started_at` / `ended_at`
  timestamps, and that the same `operation_id` appears on both the
  start and the matching end event.
- Existing webhook-plugin delivery still works (the
  `calibrator-flats` BDD remains green).

## Phase 2 — Predictive deadlines

**One PR per operation family, two or three PRs total.** Replaces
each hardcoded `Duration::from_secs(300)` (or `120`) with a
parameter computed at the tool call site from the operation's
characteristics. Phase 1's envelope already reserves the fields for
this.

### 2.1 Slew deadline (do_slew_blocking)

```text
predicted = slew_distance_arcsec / mount.slew_rate_arcsec_per_sec
            + mount.settle_after_slew.as_secs_f64()
max       = max(predicted * 3, MIN_SLEW_DEADLINE)   // e.g. 5 s floor
```

The slew rate per driver is already known (the star-adventurer-gti
driver has its `MAX_SLEW_RATE_DEG_PER_SEC`-class constants; the
generic Alpaca path can fall back to a config value
`mount.slew_rate_arcsec_per_sec` with a conservative default
matching common stepper mounts). Slew distance is the great-circle
between the mount's currently-reported pointing and the requested
`(ra, dec)`.

Threading the deadline through:

- `services/rp/src/mcp/internals.rs::poll_slewing_until_idle`
  gains a `deadline: Duration` parameter; the call site at
  `do_slew_blocking` computes it as above and passes it in.
- The default `MIN_SLEW_DEADLINE` is documented in rp.md alongside
  the slew tool catalog row.
- `center_on_target`'s per-iteration slew can compute its own
  tighter deadline from the *residual* (which is already a
  great-circle distance in arc-seconds).

### 2.2 Park deadline (do_park_blocking)

Park's effective deadline is the longest one-axis slew from the
worst-case current pointing to the park position. For drivers that
expose a static park position (most do), this is a known constant.
For drivers with operator-configurable park positions, the deadline
is computed at park time from the current pointing.

Park's `do_park_blocking` already documents that it does *not*
auto-abort on timeout (rp.md line 678: "a partially-completed park
is closer to safe than an aborted one"). That stays. The deadline
becomes the signal that something is wrong, not a trigger for
unilateral abort by rp itself — the corrective-action ladder owns
that decision in Phase 5.

### 2.3 Move-focuser deadline

```text
predicted = abs(target_position - current_position) / focuser.steps_per_sec
max       = max(predicted * 2, MIN_FOCUSER_DEADLINE)
```

The focuser driver knows its step rate; expose it via the Alpaca
`MaxIncrement` / driver-specific config. Default
`MIN_FOCUSER_DEADLINE` of e.g. 5 s for tiny moves.

### 2.4 Exposure deadline

> **As-built note (§2.4 shipped).** Landed in
> `services/rp/src/config/camera.rs` (`readout_time_estimate: Option<Duration>`,
> humantime, per-camera) + `services/rp/src/mcp/internals.rs`
> (`exposure_deadlines()` + `DEFAULT_READOUT_TIME_ESTIMATE = 15 s`,
> `EXPOSURE_READOUT_HEADROOM = 30 s`), wired onto `exposure_started` in
> `do_capture` via `.with_deadlines()`. Two decisions over the sketch
> below: (1) the deadline is **advisory** — rp does **not** enforce it.
> The plan said "rp itself does not need to enforce it (the camera driver
> does)", so `do_capture`'s existing internal readout backstop
> (`CAPTURE_READOUT_GRACE`, a separate and deliberately more-generous
> `duration + 120 s` ceiling) is unchanged; the new `predicted`/`max`
> ride the envelope purely for the Sentinel watchdog. (2) a single
> conservative `DEFAULT_READOUT_TIME_ESTIMATE` (15 s) replaces the
> per-detector-family default table — over-estimating `predicted` for fast
> CMOS is harmless (it only relaxes the watchdog), and a real rig sets the
> per-camera value. The authoritative contract is
> [`docs/services/rp.md` §Event Envelope](../services/rp.md#event-envelope)
> and the `capture` catalog row.

```text
predicted = exposure_duration + camera.readout_time_estimate
max       = predicted + READOUT_HEADROOM   // e.g. + 30 s for slow USB 2 cameras
```

`camera.readout_time_estimate` is a per-camera config field with
sensible defaults per detector family. The existing `capture`
implementation does not have a deadline today; the camera driver
handles it. The deadline goes in the `exposure_started` envelope
for Sentinel to track; rp itself does not need to enforce it (the
camera driver does), but Sentinel's view is "we expected 300 s,
it's been 330 s, what's going on?"

### 2.5 Centering deadline

> **As-built note (§2.5 shipped — completes Phase 2).** Landed in a new
> `services/rp/src/config/centering.rs` (`CenteringConfig {
> solve_time_estimate (default 30 s), slew_overhead_estimate (default
> 10 s) }`, humantime, `#[serde(default)]` on `Config.centering`) +
> `centering_deadlines()` in `services/rp/src/mcp/internals.rs`, threaded
> onto `McpHandler` via `with_centering_config` and stamped on
> `centering_started` in `center_on_target_inner`. Decisions: (1) **outer
> loop only**, per the design's open-question resolution — the watchdog
> tracks overall convergence; each per-iteration `slew`/`capture` already
> carries its own predictive deadline, so the centering envelope does not
> double-count them. (2) `predicted` is one iteration (optimistic
> single-pass convergence) and `max` is `max_attempts × per_iter`, giving
> the envelope's two fields distinct, useful meanings. (3) advisory only —
> rp does not enforce it (same posture as §2.4 exposure). The
> authoritative contract is
> [`docs/services/rp.md` §Event Envelope](../services/rp.md#event-envelope)
> and the [`center_on_target` Contract](../services/rp.md#center_on_target-contract).

`max = max_attempts * (capture_duration + solve_time_estimate +
slew_overhead_estimate)`. `capture_duration` is the operator's
`duration` parameter; the others are per-rig config values with
defaults.

### 2.6 Acceptance per family

- Every internal `Duration::from_secs(300)` / `from_secs(120)` is
  removed or repurposed.
- The MCP catalog rows in `rp.md` document the deadline derivation
  formula for each operation.
- Unit tests use `tokio::test(start_paused = true)` to verify that
  small predicted deadlines fire correctly (e.g. a synthetic
  "stuck" mount in a 5 s slew window now fails in 5 s, not 300 s).
- The existing test
  `services/rp/src/mcp/tests.rs::test_slew_timeout_returns_error_after_abort`
  becomes parameterised over the new deadline.

## Phase 3 — `/api/events/subscribe` SSE endpoint

> **As-built note (Phase 3 shipped).** The endpoint streams every
> `EventBus` event as SSE — `id` = `event_seq`, `event` = type, `data` =
> the full `EventEnvelope` JSON. Landed in `services/rp/src/events.rs` (a
> bounded 512-event history ring + `subscribe_with_history`, which snapshots
> history and subscribes under one lock for a race-free replay→live handoff),
> `services/rp/src/routes.rs` (`GET /api/events/subscribe` via a hand-rolled
> `async-stream` body: replay-after-`Last-Event-ID` → live tail → end), and
> the shutdown seam in `lib.rs` (a `CancellationToken` in `AppState`,
> cancelled by `start()` so in-flight streams end and graceful shutdown
> drains — no `main.rs` change). `Last-Event-ID` is read from the header or
> `?last_event_id=`; a reconnect whose cursor predates the ring gets a leading
> `stream_gap`; a consumer that lags `BROADCAST_CAPACITY` (256) gets a final
> `stream_gap` and is disconnected (it reconnects and replays from history).
> One new dep: **`async-stream`** (the parent's "no dep churn" note was
> optimistic — axum's `Sse` consumes a `Stream` but doesn't make one from a
> `broadcast::Receiver`). Verified by unit tests (ring/handoff, gap detection,
> `Last-Event-ID` parsing, lag→`stream_gap` via a deterministic
> broadcast-overrun integration test) and BDD `event_subscribe.feature`
> (subscribe→live and reconnect→replay over real HTTP, driven by a new
> `bdd_infra::rp_harness::SseClient` that reads the body with
> `reqwest::Response::chunk` — no `stream` feature needed). The
> authoritative contract is
> [`docs/services/rp.md` §Real-Time Stream](../services/rp.md#real-time-stream);
> the execution plan is
> [`predictive-deadlines-phase3-sse.md`](predictive-deadlines-phase3-sse.md)
> (archive on merge). **Sentinel's consumer (Phase 4) is the next gate.**

**One PR.** Wires the broadcast channel from Phase 1.3 to an HTTP
endpoint.

### 3.1 Endpoint shape

`GET /api/events/subscribe`. Accept `text/event-stream`. Response:

```
HTTP/1.1 200 OK
Content-Type: text/event-stream
Cache-Control: no-cache
Connection: keep-alive

retry: 3000

event: exposure_started
id: 1247
data: { "event_id": "f3a8b9c0-…", "event_seq": 1247, "operation_id": "0bbc7e54-…", "operation": "exposure", … }

:keep-alive

event: exposure_complete
id: 1248
data: { "event_id": "9d2e1f3a-…", "event_seq": 1248, "operation_id": "0bbc7e54-…", "operation": "exposure", … }
```

- SSE `id` is the per-emission `event_seq` (monotonically
  increasing across all events from the broadcast channel).
  Reconnecting clients send their last seen `event_seq` in
  `Last-Event-ID`; the server replays everything with
  `event_seq > last_seen` that is still in the ring buffer
  (Phase 3.2). `operation_id` lives inside the JSON payload as
  the correlation key across an operation's lifecycle events
  — *not* as the SSE id, because multiple lifecycle events share
  one `operation_id` and `Last-Event-ID` requires a total order.
- A 15-second `:keep-alive` comment line keeps middleboxes from
  closing the connection.
- Backpressure: once the SSE response headers are committed the
  HTTP status is fixed, so we cannot switch to a 4xx mid-stream.
  A consumer that lags more than the broadcast channel capacity
  has its stream closed (server-initiated end-of-body). On the
  next connect, if its `Last-Event-ID` is below the ring buffer's
  oldest entry, the server replays what is still buffered and
  emits a `stream_gap` SSE event so the client knows it lost
  some history (Sentinel handles this by escalating any in-flight
  `operation_id` it was tracking when the gap occurred). Repeat
  offenders may be refused with `429 Too Many Requests` on the
  *next* connect attempt, but this is policy, not the
  in-stream mechanism.

### 3.2 Persistence buffer

The `EventBus` keeps a small ring (e.g. last 512 events) in
memory. A reconnecting Sentinel can request events after a known
`Last-Event-ID`; missed events are replayed if still in the
buffer, dropped beyond it (and Sentinel logs a warning — its
deadline tracker handles "I don't know whether this operation
completed" by escalating, which is the conservative path).

Cross-process durability is *not* required — Sentinel is on the
same host or LAN as rp; restart-during-restart is a corner case
that the corrective-action ladder catches as "service
unresponsive" anyway.

### 3.3 Acceptance

- An integration test starts rp, opens an SSE connection to
  `/api/events/subscribe`, calls a tool that emits events, and
  asserts the SSE stream delivers them with the right envelope.
- A second test confirms reconnect-with-`Last-Event-ID` replays
  buffered events.
- A load test (BDD or integration) verifies a slow consumer is
  disconnected rather than backing up the broadcast channel.

## Phase 4 — Sentinel `OperationDeadlineMonitor`

> **As-built note (Phase 4 shipped).** Landed in a new
> `services/sentinel/src/watchdog.rs` plus config + engine + builder wiring.
> The authoritative contract is
> [`docs/services/sentinel.md` §Operation Watchdog](../services/sentinel.md#operation-watchdog).
> Decisions over the sketch below:
> (1) **New `EventMonitor` trait, not a widened `Monitor`** — the plan's
> conservative option (§4.1). It is push-based (`name()` + `run(cancel)`);
> the `Engine` spawns one task per `EventMonitor` alongside the per-`Monitor`
> poll loops. `OperationDeadlineMonitor` is the first impl.
> (2) **Injectable event source.** A `WatchdogEventSource` seam separates the
> SSE transport from the deadline-tracking logic: `HttpWatchdogEventSource`
> reads the stream with `reqwest::Response::chunk` (no `stream` cargo feature,
> matching Phase 3's `SseClient`), and a mock source feeds scripted frames to
> the unit tests under `start_paused` virtual time.
> (3) **Notify-only rung only** (§5.1 lands implicitly here). `on_expiry` is
> an enum (`notify_only` | `abort_then_restart`) that is parsed and stored,
> but both values notify-only this release; the abort/restart rungs and their
> `services` config block (abort_url/restart_command) are Phase 5 — left out
> now to avoid dead config. Escalation reuses the existing `Notifier` chain +
> dashboard history.
> (4) **Arrival-anchored deadlines.** The timer is armed for
> `max_duration_ms + buffer` from when the `*_started` frame is *received*
> (no host clock-sync needed; a replayed-overdue start fires immediately). A
> `*_started` with no `max_duration_ms` (e.g. `plate_solve`) is tracked open
> with no timer.
> (5) **Liveness.** Disconnect → reconnect with `Last-Event-ID` (fixed
> backoff, up to `reconnect_max_attempts`); a `stream_gap` escalates every
> open operation (completions may have been lost); reconnect exhaustion
> escalates "rp unresponsive". The whole `operation_watchdog` block is
> optional — absent means today's safety-polling-only behavior.
> (6) **Tests.** Six `start_paused` unit tests cover all four §4.4 behaviors
> deterministically (complete-in-time, overrun, stream-gap, reconnect-replay,
> rp-unresponsive) plus SSE parsing/classification. BDD
> `operation_watchdog.feature` (complete→no alert, overrun→escalate,
> unreachable→unresponsive) drives the **real sentinel binary** over real
> HTTP against a controllable raw-tokio chunked SSE stub (no new dependency);
> the reconnect/replay case is unit-covered rather than via a flaky
> real-timeout BDD. **No Cargo.toml dep churn.** Phase 5 (health → abort →
> restart) is the next gate.

**One or two PRs.** Adds the second `Monitor` impl alongside
`AlpacaSafetyMonitor`.

### 4.1 Monitor lifecycle

```rust
pub struct OperationDeadlineMonitor {
    rp_url: String,
    in_flight: Mutex<HashMap<OperationId, Tracked>>,
    notifier_dispatch: Arc<dyn NotifierDispatch>,
    health_checker: Arc<dyn HealthChecker>,
    aborter: Arc<dyn Aborter>,
    restarter: Arc<dyn Restarter>,
    config: OperationDeadlineConfig,
}

struct Tracked {
    operation: String,
    started_at: Instant,
    max_deadline: Instant,
    cancel: tokio::sync::oneshot::Sender<()>,
}
```

`connect()` opens the SSE connection. `disconnect()` closes it.
`poll()` is not the right interface here (it's pull-based); Phase
4 widens the `Monitor` trait to accept push-based monitors, or —
more conservatively — adds a sibling trait `EventMonitor` with the
same lifecycle hooks but a `run(&mut self) -> impl Future` instead
of `poll() -> MonitorState`. The Engine's polling-task spawn logic
gains a parallel "event monitor task" spawn path.

### 4.2 Per-event handling

```text
on *_started { operation_id, operation, max_duration_ms }:
    spawn a timer task that sleeps max_duration_ms
    record Tracked { … } keyed by operation_id

on *_complete | *_failed { operation_id }:
    remove the in_flight entry, cancel its timer

on timer expiry without complete/failed:
    escalate (Phase 5)

on SSE stream disconnect:
    log warning, schedule reconnect with exponential backoff
    if reconnect fails N times: escalate as "rp unresponsive"
```

The disconnect-as-escalation path is what gives Sentinel an
end-to-end pulse on rp's liveness, which the design doc calls out
explicitly at rp.md line 2761: *"the disconnection is an immediate
trigger for Sentinel to attempt recovery."*

### 4.3 Configuration additions

Sentinel config gains:

```json
{
  "operation_watchdog": {
    "rp_url": "http://localhost:8080",
    "reconnect_max_attempts": 5,
    "reconnect_backoff": "5s",
    "operations": {
      "slew":          { "buffer": "5s",  "on_expiry": "abort-then-restart" },
      "park":          { "buffer": "30s", "on_expiry": "notify-only"        },
      "exposure":      { "buffer": "30s", "on_expiry": "abort-then-restart" },
      "centering":     { "buffer": "0s",  "on_expiry": "abort-then-restart" },
      "move_focuser":  { "buffer": "5s",  "on_expiry": "abort-then-restart" },
      "plate_solve":   { "buffer": "10s", "on_expiry": "notify-only"        }
    },
    "services": {
      "qhyccd-alpaca":   { "abort_url": "…", "restart_command": "systemctl restart qhyccd-alpaca" },
      "qhy-focuser":     { "abort_url": "…", "restart_command": "systemctl restart qhy-focuser"   },
      "star-adventurer": { "abort_url": "…", "restart_command": null }
    }
  }
}
```

`buffer` is added to rp's `max_duration_ms` to give the wire
roundtrip + some slack. `on_expiry` selects the corrective-action
policy (Phase 5).

### 4.4 Acceptance

- A new BDD feature `operation-watchdog.feature` covers:
  - Operation started → completed within deadline → no notification.
  - Operation started → no completion in deadline → notification fired.
  - SSE stream interrupted → Sentinel reconnects, replays missed
    events, resumes tracking.
  - rp killed mid-operation → SSE stream closes → Sentinel logs
    "rp unresponsive" and notifies.
- The existing `safety-monitor` BDD scenarios still pass — the new
  monitor coexists with the old, no breaking change.

## Phase 5 — Corrective-action ladder

**One PR per ladder rung, three or four PRs total.** Each rung is
independently useful; ship them in order.

### 5.1 `Notifier`-only (already partial)

The first rung is "always notify on expiry." The existing
`PushoverNotifier` already handles safety-monitor transitions.
Phase 4's `on_expiry: notify-only` path uses the same dispatch.
This rung lands implicitly with Phase 4 — listing it here so the
ladder is complete in this document.

### 5.2 Health check

```rust
#[async_trait::async_trait]
pub trait HealthChecker {
    async fn check(&self, service: &str) -> Healthiness;
}

pub enum Healthiness {
    Responsive,
    Unresponsive,
    Unknown,
}
```

Default impl: HTTP `GET` on the configured Alpaca service's
`/api/v1/{device-type}/{n}/connected` (a cheap, side-effect-free
ASCOM call). Timeout 2 s. Anything other than a clean 200
becomes `Unresponsive`.

### 5.3 Abort

```rust
#[async_trait::async_trait]
pub trait Aborter {
    async fn abort(&self, service: &str, operation: &str) -> Result<(), …>;
}
```

Default impl: PUT to the configured `abort_url` for that service.
Each operation has a known abort verb in ASCOM
(`telescope/0/abortslew`, `camera/0/abortexposure`,
`focuser/0/halt`, etc.) — Phase 5 ships a table mapping
`operation` to the right verb so config only needs the
`abort_url` base.

After a successful abort, Sentinel POSTs
`/api/internal/operation-aborted` on rp so rp clears any sticky
state and re-plans. (New endpoint; minimal payload
`{operation_id}`.)

### 5.4 Restart

```rust
#[async_trait::async_trait]
pub trait Restarter {
    async fn restart(&self, service: &str) -> Result<(), …>;
}
```

Default impl: shell out to the configured `restart_command`.
Block until the command exits 0, or until a configurable
`max_restart_duration` (default 60 s) elapses, whichever first.

After a successful restart, Sentinel waits for the configured
service to become Responsive (re-uses the `HealthChecker`),
then POSTs `/api/internal/service-restarted` on rp so rp
reconnects.

`restart_command: null` in config means "this service is not
restartable" (the star-adventurer-gti embedded port being the
canonical example — it's a remote MCU we can't `systemctl`).
For non-restartable services the ladder stops at abort.

### 5.5 Notify

After any of the above, Sentinel dispatches a notification
through the existing `Notifier` chain describing what happened:
which operation, how long, which rung of the ladder ran,
outcome. This reuses the existing dispatch — no new code in the
notifier surface.

### 5.6 Acceptance per rung

- Health-check rung: BDD scenarios for "responsive but stuck" and
  "unresponsive" exercising the right branch.
- Abort rung: a stuck mock-Alpaca service that releases on abort
  unwedges within seconds.
- Restart rung: a mock service backed by a script that exits 0
  and restarts cleanly; sentinel notices restart, tells rp,
  rp reconnects.
- Notify rung: each rung's outcome fires the right notifier
  template.

## Cross-cutting concerns

### Backwards compatibility

- Event envelope: additive, not breaking. Existing webhook
  plugins receive the *same* `payload` they got before, now
  wrapped in the new envelope. The `event_id` field
  (`services/rp/src/events.rs:42-49`) keeps its current meaning —
  a unique id per emission, which is the routing key for the
  plugin webhook ack/completion contract
  (`POST /api/plugins/{event_id}/complete`, see
  [`docs/services/rp.md` §Step 2: Completion](../services/rp.md#step-2-completion-callback-post-to-rp)).
  The new `operation_id` is *additional*: a correlation id shared
  across an operation's `*_started`, `*_complete`, and `*_failed`
  events for the same call. The new `event_seq` is the SSE replay
  key. No existing field changes meaning or semantics, so plugin
  completion routing is unaffected.
- Sentinel config: the `operation_watchdog` block is optional. An
  upgrade that doesn't add it preserves today's safety-monitor
  behaviour.
- rp internal deadlines: the *catalog* documents the formula;
  existing callers that omit a deadline parameter get the
  formula's defaults, so no MCP-tool-call signature changes.

### Testing

- Unit: every event emission and every deadline derivation has a
  unit test, mostly `start_paused = true` + advance virtual time.
- BDD: each phase has its own `.feature` file; the existing BDD
  suite is the regression net.
- Integration: a `tests/operation-watchdog-e2e/` harness wires
  real rp + sentinel + a mock Alpaca and asserts the full ladder
  runs through wedge → recovery.

### BDD harness improvement (parallel)

Independent of this plan, but worth flagging:
`crates/bdd-infra/src/rp_harness/mcp_client.rs::call_tool` should
gain a `tokio::time::timeout` wrapper so that latent rmcp transport
bugs surface as fast step failures rather than indefinite hangs.
This is a one-line defensive change and should land separately
(it doesn't depend on any of the phases here).

### rmcp upstream

The hang investigated on PR #267 also exposed a real bug in
`rmcp::transport::streamable_http_client::StreamableHttpClientWorker`
where SSE stream termination is logged but not propagated to
pending request futures. With predictive deadlines in place
(Phase 2), the operation fails in seconds and the response
arrives well before any rmcp session keep-alive could fire, so
the bug becomes much harder to hit — but it remains real, and
upstream filing is appropriate. Tracked separately.

## Rollout

| Phase | PR count | Depends on | Lands when |
|---|---|---|---|
| 1: Event surface complete           | 1   | —         | First |
| 2: Predictive deadlines             | 2–3 | Phase 1   | Per operation family, can interleave with Phase 3 |
| 3: `/api/events/subscribe` SSE      | 1   | Phase 1   | After or parallel with Phase 2 |
| 4: `OperationDeadlineMonitor`       | 1–2 | Phase 3   | After Phase 3 |
| 5.1 Notify-only rung                | 0   | Phase 4   | Implicit |
| 5.2 Health-check rung               | 1   | Phase 4   | After Phase 4 |
| 5.3 Abort rung                      | 1   | Phase 5.2 | After Phase 5.2 |
| 5.4 Restart rung                    | 1   | Phase 5.3 | After Phase 5.3 |

Total: 7–10 PRs across rp and sentinel, no Cargo.toml dep churn
beyond `tokio::sync::broadcast` (already in tree) and an SSE
library (axum already has `sse` support).

## Open questions

- **Slew-rate config surface.** Phase 2.1 needs each driver to
  expose its slew rate to rp. The GTi driver has a constant; the
  generic Alpaca path needs a config field. Decision: ship the
  config field with a conservative default (e.g. 2°/s, a slow
  stepper mount), let drivers override via their own config.
- **Centering deadline composition.** `center_on_target`'s
  deadline today would be `max_attempts × per-iter`. Per-iter is
  itself capture + solve + slew. Should Sentinel track only the
  outer `centering_*` event, or the inner per-iteration timing
  too? Decision: outer only — the inner iteration events
  (`centering_iteration`) are informational; the watchdog cares
  about overall convergence.
- **Manual operator overrides.** Should the operator be able to
  *extend* a deadline mid-flight ("I know this slew is slow,
  give it 30 s more")? Decision: defer — once the basic ladder
  works, surface this via a Sentinel REST endpoint that pauses
  a specific operation_id's timer. Not in MVP.
- **Cross-host Sentinel.** Sentinel is colocated with rp in
  current deployments. Cross-host adds TLS + auth complexity. The
  design doc's ADR-002 (TLS) and ADR-003 (auth) already cover
  the primitives; Phase 4 should reuse the existing auth header
  flow that `AlpacaSafetyMonitor` already supports.

## References

- [`docs/services/rp.md` §Safety / §Sentinel Watchdog Integration](../services/rp.md#safety) — design intent.
- [`docs/services/rp.md` §Real-Time Stream](../services/rp.md#real-time-stream) — SSE event-stream contract.
- [`docs/services/sentinel.md`](../services/sentinel.md) — current sentinel scope (SafetyMonitor only).
- [`docs/decisions/002-tls-for-inter-service-communication.md`](../decisions/002-tls-for-inter-service-communication.md) — TLS primitives Sentinel→rp can reuse.
- [`docs/decisions/003-authentication-for-device-access.md`](../decisions/003-authentication-for-device-access.md) — auth header pattern.
- `services/rp/src/mcp/internals.rs::poll_slewing_until_idle` — current hardcoded 300 s.
- `services/rp/src/events.rs::EventBus` — current webhook-only delivery path.
- `services/sentinel/src/{engine,monitor,alpaca_client}.rs` — current sentinel architecture.
- PR #267 macOS BDD hang investigation — catalyst incident; see commit history on `main` for the analysis (no separate issue filed yet).
