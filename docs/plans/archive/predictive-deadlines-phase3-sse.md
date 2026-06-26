# Phase 3 — `/api/events/subscribe` SSE endpoint (implementation plan)

Execution plan for **Phase 3** of
[`predictive-deadlines-and-watchdog.md`](predictive-deadlines-and-watchdog.md):
expose the in-process `EventBus` broadcast channel (landed Phase 1) over HTTP
as a Server-Sent-Events stream so an external supervisor — Sentinel's
Phase 4 `OperationDeadlineMonitor` — can consume every operation event in
real time, replay missed events after a reconnect, and treat stream
disconnect as an rp-liveness signal.

This is the **gate** for the watchdog half of the plan: Phase 4 and Phase 5
cannot begin until the events leave the process. Today nothing consumes the
broadcast channel over the wire — `EventBus::subscribe()` exists
(`services/rp/src/events.rs:235`) but has no HTTP surface.

## Status

**Status: COMPLETE (archived 2026-06-25).** Phase 3 of the predictive-deadlines
plan — the `/api/events/subscribe` SSE endpoint and its replay buffer — shipped
in #373. The authoritative contract lives in
[`docs/services/rp.md` §Real-Time Stream](../../services/rp.md#real-time-stream).
All steps below delivered (commits noted per row).

| Step | What | State |
|---|---|---|
| 0 | Design-doc updates (rp.md §Real-Time Stream as-built, parent §3 as-built note) | ✅ done |
| 1 | Bounded event-history ring on `EventBus` + race-free `subscribe_with_history` | ✅ done (`da4f314`) |
| 2 | `GET /api/events/subscribe` SSE handler (`routes.rs`) + router wiring | ✅ done (`60c5cb8`) |
| 3 | `Last-Event-ID` replay + `stream_gap` + lag/disconnect semantics | ✅ done (`60c5cb8`) |
| 4 | Graceful-shutdown cooperation (thread the lifecycle `CancellationToken` into the handler) | ✅ done (`60c5cb8`) |
| 5 | Unit tests (ring buffer, replay handoff) + BDD `event_subscribe.feature` + SSE test helper | ✅ done |

> **As-built deltas from the plan below.** Step 5's lag-disconnect check is a
> deterministic **routes-level integration test** (overrun the broadcast
> channel before polling the body) rather than a BDD load test — acceptance
> §3.3(3) explicitly permits "BDD or integration", and forcing a 256-event
> lag over real HTTP would be slow and flaky. The SSE test helper
> (`bdd_infra::rp_harness::SseClient`) reads the body with
> `reqwest::Response::chunk` rather than `bytes_stream()`, so **no `stream`
> reqwest feature and no Bazel repin were needed** (correcting Step 6's
> expectation). `event_subscribe.feature` covers subscribe→live and
> reconnect→replay over real HTTP + OmniSim; both pass.

**Scope: the SSE endpoint and its replay buffer only.** Sentinel's consumer
(`OperationDeadlineMonitor`, Phase 4) and the corrective-action ladder
(Phase 5) are **out of scope**. This PR makes rp *emit over the wire*; it
adds no new consumer.

## Goal

```
GET /api/events/subscribe        Accept: text/event-stream
                                 Last-Event-ID: 1247        (optional, on reconnect)

HTTP/1.1 200 OK
Content-Type: text/event-stream

event: slew_started
id: 1248
data: {"event_id":"…","event_seq":1248,"operation_id":"…","event":"slew_started", …}

:keep-alive                       (comment line every 15 s)

event: slew_complete
id: 1249
data: {"event_id":"…","event_seq":1249,"operation_id":"…","event":"slew_complete", …}
```

- The SSE `id:` is the envelope's `event_seq` (`u64`, monotonic across all
  events — `events.rs:64`). It is the `Last-Event-ID` replay key. The
  `operation_id` is the cross-event correlation key and lives **inside** the
  JSON `data:`, not as the SSE id (multiple lifecycle events share one
  `operation_id`; `Last-Event-ID` needs a total order — parent §3.1).
- The SSE `event:` name is the envelope's `event` field (`String`, e.g.
  `"slew_started"` — `events.rs:70`).
- The `data:` is the whole `EventEnvelope` serialized with its existing
  `#[derive(Serialize)]` (`events.rs:55`); no new wire shape.
- On reconnect with `Last-Event-ID: N`, the server first replays every
  buffered envelope with `event_seq > N`, then tails live. If `N` is older
  than the oldest buffered event, it emits one synthetic `stream_gap` event
  (so the client knows it lost history) and replays what remains.

## Decisions

- **D1 — Bounded history ring lives on `EventBus`.** `Last-Event-ID` replay
  needs a queryable history; the `broadcast` channel's capacity-256 buffer
  (`BROADCAST_CAPACITY`, `events.rs:40`) is not addressable by `event_seq`
  and is consumed per-subscriber. Add `history: Mutex<VecDeque<EventEnvelope>>`
  to `EventBus` (`events.rs:192`), capacity **512** (parent §3.2), pushed in
  `dispatch` (`events.rs:266`) after identity assignment, `pop_front` when
  over cap. Precedent for a `VecDeque` ring already in tree:
  `services/rp/src/persistence/cache.rs:171`.
- **D2 — Race-free replay→live handoff via one method under one lock.** The
  hazard: between snapshotting history and subscribing, a `dispatch` could
  slip an event past both paths (missed) or into both (duplicated). Fix:
  `dispatch` holds the `history` lock across **push-to-history + broadcast
  send**, and a new `EventBus::subscribe_with_history(last_seq: Option<u64>)
  -> (Vec<EventEnvelope>, broadcast::Receiver<EventEnvelope>)` holds the same
  lock across **snapshot + `broadcast.subscribe()`**. `subscribe()` only
  receives *future* sends, so under the lock: events already dispatched are
  in the snapshot and were sent before we subscribed (receiver won't repeat
  them); events dispatched after are delivered live exactly once. The lock
  serialises the boundary — no gap, no overlap. This is the one genuinely
  subtle correctness point in Phase 3 and gets a dedicated unit test.
- **D3 — Hand-rolled `async-stream` body, not `BroadcastStream`.** The
  handler does three things in sequence: yield the replay snapshot, then loop
  on `rx.recv()` for live events, then end cleanly on shutdown. `async_stream::stream!`
  expresses that replay-then-live-then-stop flow in one readable async block,
  including the `RecvError::Lagged` → terminate branch. `tokio_stream::wrappers::BroadcastStream`
  (would need the `sync` feature) only covers the live tail and would force a
  `stream::iter(replay).chain(...)` plus a separate lag adapter. Cost: one
  new dep, **`async-stream`** (small, no transitive weight) on
  `services/rp/Cargo.toml`. This corrects the parent plan's optimistic "no
  dep churn beyond axum sse" (parent §Rollout) — axum's `Sse` consumes a
  `Stream` but does not produce one from a `broadcast::Receiver`.
- **D4 — Lag ⇒ end the stream; reconnect recovers.** A consumer that falls
  more than `BROADCAST_CAPACITY` (256) behind gets `RecvError::Lagged(n)`.
  Per parent §3.1 the HTTP status is already committed, so we cannot 4xx
  mid-stream; instead the handler logs `warn!`, yields a final `stream_gap`
  event, and **ends the response body**. The client reconnects with its last
  `Last-Event-ID` and gets ring-buffer replay (D1) or a `stream_gap` (D5).
  Sentinel (Phase 4) treats `stream_gap` / disconnect as "escalate any
  in-flight operation I was tracking" — the conservative path.
- **D5 — `stream_gap` is a real envelope, not a bare comment.** When replay
  cannot reach back to the client's `Last-Event-ID` (history evicted past
  it), emit a first event `event: stream_gap`, `id:` = current head seq,
  `data:` = `{"event":"stream_gap","event_seq":N,"payload":{"requested_after":M,"oldest_available":K}}`.
  Reuses the envelope shape so clients parse it with the same path. It is
  **not** persisted to the ring (it is per-connection diagnostic), so it gets
  no `event_seq` of its own from `next_seq` — it borrows the current head seq
  as its `id` so a subsequent reconnect's `Last-Event-ID` stays meaningful.
- **D6 — `Last-Event-ID` from header *or* `?last_event_id=` query.** Browsers
  and the `EventSource` API resend the `Last-Event-ID` **header** automatically
  on reconnect; Sentinel's reqwest client will set it explicitly. A
  `?last_event_id=` query param is also accepted as an explicit/testable form
  (and avoids pulling `axum-extra`'s `TypedHeader` just for one header — read
  it off `HeaderMap` directly). Header wins if both present.
- **D7 — Cooperative shutdown via a `CancellationToken` in `AppState`.** A
  long-lived SSE stream otherwise blocks `axum::serve(...).with_graceful_shutdown(shutdown)`
  (`lib.rs:227`) from completing — graceful shutdown stops accepting new
  connections but waits for in-flight bodies, and an SSE body never ends on
  its own. Thread a clone of the lifecycle `CancellationToken` (the
  `Shutdown` handed to the server closure wraps one — see `main.rs:145`,
  `rusty-photon-service-lifecycle`) into `AppState` so the `stream!` body can
  `tokio::select!` on `token.cancelled()` and end the stream when rp shuts
  down. This is the only non-additive wiring change in the PR; flagged as the
  one seam to trace carefully (how the token reaches `build_router` /
  `AppState` construction in `lib.rs`).
- **D8 — Keep-alive 15 s via axum.** `Sse::keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))`
  emits the `:keep-alive` comment line that stops middleboxes closing an idle
  connection (parent §3.1). No hand-rolling.
- **D9 — Auth/TLS reuse, no new surface.** The endpoint sits behind the same
  optional auth layer as every other route (`rp_auth::layer`, applied in
  `lib.rs:160`); the agent investigation confirmed the auth middleware is
  early-exit (header check then `next.run`) and does not buffer the body, so
  it is streaming-safe. Cross-host Sentinel reuses the existing auth-header /
  TLS path (parent §Open-questions, ADR-002/003). No endpoint-specific auth.

## Code anchors (verified)

- `services/rp/src/events.rs`
  - `EventEnvelope` (`:49-100`, `#[derive(Serialize)]` at `:55`; `event_seq:
    u64` `:64`; `event: String` `:70`) — wire shape, unchanged.
  - `EventBus` (`:192-198`: `plugins`, `broadcast: broadcast::Sender<EventEnvelope>`,
    `next_seq: AtomicU64`) — **add** `history: Mutex<VecDeque<EventEnvelope>>`.
  - `BROADCAST_CAPACITY = 256` (`:40`) — **add** `HISTORY_CAPACITY = 512`.
  - `subscribe()` (`:235`) — keep; **add** `subscribe_with_history` (D2).
  - `dispatch()` (`:266-277`) — hold the `history` lock across push + send (D2).
- `services/rp/src/routes.rs`
  - `AppState` (`:28-34`: `equipment`, `mcp: McpHandler`, `session`,
    `image_cache`) — **add** the shutdown `CancellationToken` (D7). `EventBus`
    is reachable today via `state.mcp.event_bus` (`Arc<EventBus>`, public on
    `McpHandler`).
  - `build_router` (`:36`), route list (`:46-60`), `.with_state(state)`
    (`:60`) — **add** `.route("/api/events/subscribe", get(subscribe_events))`.
  - Handler convention: `async fn name(State(state): State<AppState>, …) ->
    <IntoResponse>` (e.g. `get_image_pixels` returns `Response`, `:229`) — the
    new handler returns `Sse<impl Stream<Item = Result<Event, Infallible>>>`.
- `services/rp/src/lib.rs`
  - `EventBus` construction (`:71`: `Arc::new(EventBus::from_config(&config.plugins))`).
  - server start + graceful shutdown (`:217-231`: `axum::serve(...).with_graceful_shutdown(shutdown)`).
  - auth layer application (`:160`).
- `services/rp/src/main.rs:145` — `ServiceRunner::new("rp").run(move |shutdown:
  Shutdown| …)`; source of the `CancellationToken` for D7.
- `crates/bdd-infra/src/rp_harness/` — `world.rs:204` `rp_url()`,
  `launcher.rs:48` `wait_for_rp_healthy` (reqwest poll pattern),
  `webhook.rs:73` `WebhookReceiver` (the test-server precedent the SSE test
  helper mirrors).
- `Cargo.toml` — `axum` 0.8 (`:90`, `response::sse` available, no feature
  flag); `reqwest` 0.13.4 `["form","json"]` (`:92`) — **needs the `stream`
  feature** for `bytes_stream()` in the SSE test helper.

## Work breakdown (test-first)

### Step 0 — Design-doc reconciliation (before code)
- `docs/services/rp.md` §Real-Time Stream / §Event System: flip
  `/api/events/subscribe` from "documented, no handler" to as-built —
  document the SSE framing (`id`=`event_seq`, `event`=type, `data`=envelope),
  `Last-Event-ID` replay, the 512-event ring, `stream_gap`, the 15 s
  keep-alive, and the lag-disconnect contract.
- Parent [`predictive-deadlines-and-watchdog.md`](predictive-deadlines-and-watchdog.md)
  §Phase 3: add a short "as-built" note (mirroring the Phase 1.1 note),
  recording the `async-stream` dep and the `subscribe_with_history` handoff.

### Step 1 — History ring + handoff (`events.rs`) + unit tests
- Add `history` field + `HISTORY_CAPACITY`; push/evict in `dispatch` under the
  lock; add `events_since(last_seq) -> (Vec<EventEnvelope>, oldest_seq)` and
  `subscribe_with_history` (D2). Unit-test: eviction at cap; `events_since`
  boundary; the replay→live handoff delivers each of N emissions exactly once
  with no gap/dup across the subscribe boundary.

### Step 2 — SSE handler + router wiring (`routes.rs`)
- `subscribe_events(State, HeaderMap, Query) -> Sse<…>`: read `Last-Event-ID`
  (D6), call `state.mcp.event_bus.subscribe_with_history(last_seq)`, build the
  `async_stream::stream!` body that yields replay then live (D3), map each
  envelope to `Event::default().id(seq).event(name).json_data(&env)`, wrap in
  `Sse::new(stream).keep_alive(15 s)` (D8). Register the route at `:60`.

### Step 3 — Replay / gap / lag semantics
- `stream_gap` when `last_seq` precedes the ring (D5); `Lagged(n)` → `warn!`,
  final `stream_gap`, end body (D4). Covered by the BDD scenarios in Step 5.

### Step 4 — Graceful-shutdown cooperation (D7)
- Thread the lifecycle `CancellationToken` into `AppState`; `select!` on
  `token.cancelled()` inside the `stream!` so shutdown ends in-flight streams
  and `with_graceful_shutdown` can complete. Trace the `Shutdown`→`AppState`
  seam in `lib.rs`/`main.rs` first.

### Step 5 — Tests
- **Unit** (`events.rs` tests): Step 1 ring + handoff tests; `tokio::test`.
- **SSE test helper** (`crates/bdd-infra/src/rp_harness/`): a small
  `SseClient` mirroring `WebhookReceiver` — `reqwest` (+`stream` feature)
  `GET …/api/events/subscribe`, `.bytes_stream()`, parse the `id:`/`event:`/`data:`
  line protocol into `ReceivedEvent`s.
- **BDD** (`services/rp/tests/features/event_subscribe.feature` + `sse_steps.rs`):
  (1) subscribe → trigger an operation → assert the live `*_started`/`*_complete`
  frames arrive with correct `id`/`event`/`data`; (2) disconnect, reconnect
  with `Last-Event-ID` → assert buffered events replay in order; (3) a slow
  consumer that lags past capacity → assert the stream closes (server-initiated
  end-of-body) rather than backing up the bus. Keep `operation_events.feature`
  and `event_delivery.feature` (webhook path) green — the SSE path is additive.

### Step 6 — Gate
- Add `async-stream` to `services/rp/Cargo.toml` and the `stream` reqwest
  feature where the test helper lives; then `CARGO_BAZEL_REPIN=1 bazel mod tidy
  && bazel mod tidy` (CLAUDE.md rule 10). `cargo fmt` + `cargo rail run
  --profile commit -q`; final `cargo build`.

## Acceptance → parent §3.3

1. `GET /api/events/subscribe` streams every emitted envelope as SSE with
   `id`=`event_seq`, `event`=type, `data`=envelope JSON; an integration test
   opens the stream, triggers a tool, and asserts delivery. → Steps 2, 5.
2. Reconnect with `Last-Event-ID` replays buffered events (and `stream_gap`s
   when history was evicted past the cursor). → Steps 1, 3, 5.
3. A slow consumer is disconnected rather than backing up the broadcast
   channel. → Steps 3, 5.
4. rp shutting down ends in-flight SSE streams cleanly (graceful shutdown
   completes). → Step 4.
5. Existing webhook delivery and the Phase 1/2 BDD remain green; the endpoint
   is additive and reuses the existing auth/TLS layer. → Steps 5, D9.

## Risks

- **Replay/live race** (D2) — the one correctness-critical seam; mitigated by
  doing snapshot+subscribe and push+send under the single `history` lock, with
  a dedicated exactly-once unit test.
- **Shutdown wiring** (D7) — the only non-additive change; the `Shutdown`→
  `AppState` token seam must be traced before coding. Fallback if it proves
  invasive: accept that streams end on client disconnect / process exit and
  document the graceful-shutdown caveat — but plumbing the token is preferred.
- **Dep churn** (D3) — one small crate (`async-stream`) + a reqwest feature
  for tests; corrects the parent's "no dep churn" note. Remember the Bazel
  repin (rule 10).
- **Lag tuning** — `BROADCAST_CAPACITY` (256) and `HISTORY_CAPACITY` (512) set
  how far a consumer can lag before a gap; 512 ≫ any realistic in-flight
  operation count, but document both so Phase 4 sizing is explicit.

## Footprint

~6–9 files: `events.rs` (ring + handoff), `routes.rs` (handler + state +
route), `lib.rs`/`main.rs` (shutdown-token thread-through),
`services/rp/Cargo.toml` (`async-stream`), a new SSE test helper in
`crates/bdd-infra/src/rp_harness/`, a new `event_subscribe.feature` +
`sse_steps.rs`, and `docs/services/rp.md`. No change to the `EventEnvelope`
wire shape — Phase 1 already reserved everything Phase 3 streams.
