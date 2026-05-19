# Shared Transport Extraction Plan (issue #257)

## Status

**Active — design.** Phase 1 (this plan) lands first. Implementation is
sequenced in five phases (A–E), one PR each, per the rollout below.

## Motivation

Four services (`qhy-focuser`, `ppba-driver`, `pa-falcon-rotator`,
`star-adventurer-gti`) have grown independently to similar shapes for a
problem they all share: a single duplex transport, multiple in-process
clients (ASCOM devices + a background poll loop + slew/park watchers), a
connect-use-disconnect lifecycle. The shapes converged through
copy-paste and copy-paste-with-fix, not through a shared abstraction.

That convergence has cost us:

* **Lock-holding race in `set_connected` (issue #257).** Fixed three
  times — `pa-falcon-rotator` (PR #241 commit `8cd6e16`),
  `qhy-focuser` (PR #256), `ppba-driver` (PR #255, in flight). Each fix
  is structurally identical: hold the `requested_connection` write lock
  across the entire check-and-modify. `ppba-driver` still has the
  defect on `main`.
* **Refcount + reader/writer leak on partial-connect failure
  (issue #258).** `qhy-focuser`'s `connect()` bumps the refcount and
  installs reader/writer *before* the handshake; any handshake error
  leaves the manager wedged until process restart. `ppba-driver` has
  the same defect. PR #260 fixes it for `qhy-focuser`;
  `pa-falcon-rotator` and `star-adventurer-gti` already roll back
  correctly. This is the more dangerous bug class — a single-client
  wedge on any handshake timeout, not a multi-client race.
* **Polling-task teardown leak** is the next bug class waiting to bite,
  same shape: each service spawns a poll task on connect and is
  responsible for stopping it on disconnect, with no shared mechanism
  to enforce that the task's lifetime tracks the transport's.

The four `SerialManager` implementations now total roughly 1200 lines
of lifecycle scaffolding that says the same thing. Three of those four
were written by the same person to the same template; the fourth
(`star-adventurer-gti::TransportManager`) explicitly cites the others
as precedent. What looks like a shared pattern is actually parallel
implementations of the same pattern.

## Goals

* Extract a workspace crate `crates/shared-transport/` that owns the
  pieces that are genuinely shared:
    1. Byte-level transport (factory trait + `AsyncRead`/`AsyncWrite`
       handles, with a UDP duplex adapter for Star Adventurer GTi).
    2. Codec framing (encode + decode + terminator, with an optional
       stale-frame predicate for protocols that need it).
    3. Request arbitration (today's `command_lock`).
    4. Refcounted lifecycle (today's `connection_count` +
       `serial_available` + slot).
    5. A `Session<C>` Drop-safe handle that is the source of truth for
       "this client is connected."
    6. A `Hooks` surface for service-specific handshake, teardown,
       and *while-open* work (the natural home for poll tasks).
* Make all three bug classes structurally impossible across the four
  services after migration.
* Keep per-service protocol code and per-service business state where
  they are today.

## Non-goals

* A generic `Protocol` trait that absorbs handshake command sequences,
  response parsing, polling cadence, or cached-state shape.
  These differ enough that a single trait would either bloat or force
  each service into a procrustean shape.
* A solution for multi-process / cross-Alpaca-client races (per-device
  `requested_connection` handles that; ASCOM's `Connected` flag is
  per-session by design).
* Folding `star-adventurer-gti`'s `PollPauseGuard` into the shared
  crate. It lives inside that service's `while_open` task, never
  crosses the crate boundary.
* Replacing the per-service mock state machines. Each service keeps
  its own in-memory mock — only the factory trait moves to the shared
  crate.

## Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│  Per-service device (FocuserDevice, PpbaSwitchDevice, MountDevice…)  │
│   - Arc<Manager<C>>                                                  │
│   - session: RwLock<Option<Session<C>>>   ← flag = resource          │
└──────────────────────────────────────────────────────────────────────┘
                                  │
┌──────────────────────────────────────────────────────────────────────┐
│  Per-service Manager<C>  (thin)                                      │
│   - Arc<SharedTransport<C>>                                          │
│   - protocol-specific public API (move_absolute, halt, …)            │
│   - service-specific cached state (CachedState, MountSnapshot, …)    │
│   - constructs Hooks { handshake, teardown, while_open }             │
└──────────────────────────────────────────────────────────────────────┘
                                  │
╔══════════════════ crates/shared-transport ═══════════════════════════╗
║ SharedTransport<C: Codec>                                            ║
║  - refcount + slot + open-state lock                                 ║
║  - acquire() → Session<C>  (Drop = release; last out closes)         ║
║  - 0→1: open via factory → handshake → spawn while_open              ║
║  - 1→0: cancel while_open → teardown → close                         ║
║                                                                      ║
║ Session<C: Codec>                                                    ║
║  - request(cmd) → C::Response   (write+read+decode, serialized)      ║
║  - send(cmd) → ()               (write only; protocol must promise   ║
║                                  no reply)                           ║
║  - impl Drop                                                         ║
║                                                                      ║
║ Connection<C: Codec>   (internal)                                    ║
║  - owns boxed duplex transport + codec                               ║
║  - holds the request-arbitration lock                                ║
║                                                                      ║
║ trait Codec      { encode, decode, terminator, [matches, retries] }  ║
║ trait TransportFactory { open() → Box<dyn DuplexTransport> }         ║
║ trait DuplexTransport: AsyncRead + AsyncWrite + Unpin + Send         ║
╚══════════════════════════════════════════════════════════════════════╝
```

### What lifts to the shared crate

* `TransportFactory` trait + `DuplexTransport` blanket impl.
* `Codec` trait.
* `Connection<C>` — request arbitration, framed read, decode.
* `SharedTransport<C>` — refcount, slot, open/close, while-open task
  lifecycle.
* `Session<C>` — request/send API, Drop-release.
* `Hooks<C>` — handshake / teardown / while_open closures.

### What stays per-service

* Concrete codec implementation (JSON for qhy; ASCII-LF echo for ppba
  and pa-falcon; `:`-prefixed for SAG-GTI).
* `Command` / `Response` enums and their parsers.
* The `Manager<C>` wrapper with the service's public protocol API
  (`move_absolute`, `set_reverse`, `read_status`, etc.).
* Cached business state (`CachedState`, `MountSnapshot`,
  `MountParameters`, sensor `SensorMean` windows, etc.) and the poll
  loop body that updates it.
* Per-service mock factory implementing `TransportFactory` and the
  in-memory mock state machine.
* `PollPauseGuard` (SAG-GTI only) — internal to its `while_open`
  task.

## API design

### `Codec`

```rust
pub trait Codec: Send + Sync + 'static {
    type Command: Send + Sync;
    type Response: Send;
    type Error: Send + Sync + 'static;

    /// Encode a command for the wire. Includes any framing the wire
    /// expects on the request side (terminator, prefix, etc).
    fn encode(&self, cmd: &Self::Command) -> Vec<u8>;

    /// Decode one response frame. The bytes passed do NOT include the
    /// terminator (the connection strips it before calling).
    fn decode(&self, bytes: &[u8]) -> Result<Self::Response, Self::Error>;

    /// Byte that ends one response frame on the read side. Default
    /// is LF (`b'\n'`) which covers ppba and pa-falcon. Star Adventurer
    /// GTi overrides to CR (`b'\r'`); qhy-focuser overrides to `b'}'`.
    fn terminator(&self) -> u8 { b'\n' }

    /// Return true iff `resp` is the response to `cmd`. Default is
    /// always-true (matches the immediately preceding request).
    /// qhy-focuser overrides this to compare cmd_id↔idx, so that
    /// unsolicited mid-move frames don't satisfy a foreground request.
    fn matches(&self, cmd: &Self::Command, resp: &Self::Response) -> bool {
        let _ = (cmd, resp);
        true
    }

    /// Maximum number of non-matching frames the read side will skip
    /// before erroring. Default 0 (any non-matching frame is an error).
    /// qhy-focuser overrides to 5 to absorb unsolicited position
    /// updates emitted during a move.
    fn max_skip(&self) -> usize { 0 }
}
```

### `TransportFactory` + `DuplexTransport`

```rust
#[async_trait]
pub trait TransportFactory: Send + Sync + 'static {
    async fn open(&self) -> Result<Box<dyn DuplexTransport>, TransportError>;
}

pub trait DuplexTransport: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> DuplexTransport for T {}
```

`tokio_serial::SerialStream` already implements both. The crate
provides `UdpDuplex` — a thin adapter that wraps
`tokio::net::UdpSocket` as `AsyncRead + AsyncWrite` so SAG-GTI's
network factory can return one.

### `Session<C>`

```rust
pub struct Session<C: Codec> { /* Arc<SharedTransport<C>> + slot id */ }

impl<C: Codec> Session<C> {
    pub async fn request(&self, cmd: C::Command) -> Result<C::Response, C::Error>;
    pub async fn send(&self, cmd: C::Command) -> Result<(), C::Error>;
}

impl<C: Codec> Drop for Session<C> {
    fn drop(&mut self) {
        // Decrement; if we were last, spawn an async cleanup task
        // (acquire_lock-guarded) that cancels while_open, runs
        // teardown, closes the transport.
    }
}
```

`request` writes the encoded command, reads frames until `decode`
yields one for which `matches()` returns true (or `max_skip` is
reached), and returns it. It holds the connection's command lock for
the whole transaction.

`send` writes and flushes, returns. The protocol must promise no
response — otherwise the unread bytes desync the next `request`.
Today every wire request has a wire response; `send` is forward-compat
plumbing.

### `SharedTransport<C>` + `Hooks<C>`

```rust
pub struct Hooks<C: Codec> {
    /// Runs after open, before any external Session escapes.
    /// On error: roll back (close, decrement), propagate.
    pub handshake: Box<
        dyn Fn(&Connection<C>) -> BoxFuture<'_, Result<(), C::Error>>
            + Send + Sync,
    >,

    /// Runs on the 1→0 transition with the connection still open.
    /// Best-effort: errors are logged at warn!, not propagated.
    pub teardown: Box<
        dyn Fn(&Connection<C>) -> BoxFuture<'_, ()>
            + Send + Sync,
    >,

    /// Optional: spawned after handshake succeeds; receives its own
    /// Session + a CancellationToken. The task must drop its Session
    /// when the token fires. Awaited (with a bounded timeout) during
    /// the 1→0 transition before teardown runs.
    pub while_open: Option<Box<
        dyn Fn(Session<C>, CancellationToken) -> BoxFuture<'static, ()>
            + Send + Sync,
    >>,
}

pub struct SharedTransport<C: Codec> { /* ... */ }

impl<C: Codec> SharedTransport<C> {
    pub fn new(
        factory: Arc<dyn TransportFactory>,
        codec: C,
        hooks: Hooks<C>,
    ) -> Arc<Self>;

    pub async fn acquire(self: &Arc<Self>) -> Result<Session<C>, C::Error>;

    /// Cheap, non-blocking. True between successful handshake and the
    /// start of teardown.
    pub fn is_available(&self) -> bool;
}
```

### Lifecycle internals (acquire / release)

`SharedTransport` holds:
* `count: AtomicU32` — external session refcount.
* `slot: Mutex<Option<Arc<Connection<C>>>>` — Some between open and
  close.
* `available: AtomicBool` — true between handshake-success and
  teardown-start. Distinct from `count > 0` because a connect-in-flight
  has `count == 1` but `available == false`.
* `acquire_lock: Mutex<()>` — serializes `acquire()` and the async
  cleanup that runs after the last `Session` drops. Acquires are
  rare; this is not a hot path.
* `while_open_state: Mutex<Option<(JoinHandle<()>, CancellationToken)>>`
  — present iff a while-open task is running.

`acquire()`:
1. Take `acquire_lock`.
2. `let prev = count.fetch_add(1, SeqCst);`
3. If `prev == 0`:
    1. `factory.open()` → boxed duplex transport.
    2. Wrap in `Connection<C>` (codec, command-lock).
    3. Run `hooks.handshake(&conn)`. On `Err`: clear slot,
       `count.fetch_sub(1)`, return error. (Connection drops, transport
       closes.)
    4. Store `Arc::new(conn)` in `slot`.
    5. `available.store(true, SeqCst)`.
    6. If `hooks.while_open` is `Some`: acquire an *internal* session
       (does **not** affect `count`), spawn the closure, store handle
       + cancellation token in `while_open_state`.
4. Return `Session<C>` (whose `Drop` is wired to this `SharedTransport`).

`Session::drop`:
1. `let prev = count.fetch_sub(1, SeqCst);`
2. If `prev == 1`: schedule async cleanup via `tokio::spawn`. (Drop is
   sync; the spawn is fire-and-forget on the current runtime.)

Async cleanup:
1. Take `acquire_lock`.
2. Re-check `count`: if another `acquire()` already raced and took
   `count` back to ≥1, abort cleanup (the new owner is the new
   opener; this cleanup is stale).
3. `available.store(false, SeqCst)`.
4. If `while_open_state` is `Some`: fire the cancellation token,
   await the join handle with a bounded timeout (5s), then drop the
   internal session.
5. Take the connection out of `slot`. Run `hooks.teardown(&conn)`.
6. Drop the connection — transport closes.

The `acquire_lock` makes the open/close transitions atomic with
respect to each other. The fast path (acquire when `count > 0`) takes
the lock, increments, and returns — microseconds.

### Error model

`shared-transport` defines a small `TransportError` enum for
`TransportFactory::open` failures and for connection-layer I/O. Codec
errors are the codec's own `C::Error` type — `SharedTransport` is
generic over it and propagates without wrapping. Each service maps its
codec error + `TransportError` to its existing service-level
`ASCOMResult` mapping (no change to current ASCOM error semantics).

## How the bug classes dissolve

### Race (issue #257)

Today: two concurrent `set_connected(true)` on the same device can
both observe `requested_connection == false`, both call
`SerialManager::connect()` (double-increment the refcount), and both
set the per-device flag — leaving the refcount permanently elevated by
one.

After: the device's "I am connected" state is `RwLock<Option<Session<C>>>`.
Two concurrent `set_connected(true)` serialize on the device's own
write lock. The flag and the resource are the same value — there is
no second source to desync.

```rust
async fn set_connected(&self, on: bool) -> ASCOMResult<()> {
    let mut s = self.session.write().await;
    match (on, s.is_some()) {
        (true, false) => *s = Some(self.transport.acquire().await
                                       .map_err(Self::to_ascom)?),
        (false, true) => *s = None,        // Drop releases
        _ => {}
    }
    Ok(())
}
```

### Refcount + reader/writer leak on partial-connect failure (issue #258)

Today: `SerialManager::connect()` increments the refcount and installs
reader/writer *before* the handshake. If any of the handshake commands
fails, both are left in place; the next `connect()` short-circuits on
the elevated refcount and the device stays wedged.

After: `SharedTransport::acquire()` only returns `Session<C>` on full
success. The 0→1 path is structurally atomic — if `handshake` errors,
the slot is cleared, the count is decremented, the transport is
dropped (closing the underlying port), and the caller gets `Err`. No
half-state can escape because there is no path from inside the
handshake-error branch to a `Session` value.

### Polling task teardown leak (next bug class)

Today: each service's `disconnect()` is responsible for sending
`shutdown_tx.send(true)` and awaiting the poll task. A future
refactor that forgets either step would leak the task and the
transport.

After: the `while_open` task is owned by `SharedTransport`. The 1→0
transition fires the cancellation token and awaits the join handle.
Services can't forget — there is no per-service code to forget.

## Per-service migration sketches

### `qhy-focuser`

Before (current):

```rust
// services/qhy-focuser/src/serial_manager.rs — ~600 lines
//   connection_count: AtomicU32
//   serial_available: AtomicBool
//   reader/writer Mutexes
//   command_lock: Mutex<()>
//   shutdown_tx: watch::Sender<bool>
//   poll_handle: Mutex<Option<JoinHandle<()>>>
//   cached_state: Arc<RwLock<CachedState>>
//   + connect/disconnect/handshake/start_polling/stop_polling/poll_position/
//     poll_temperature/send_command_internal/read_response_for/…

// services/qhy-focuser/src/focuser_device.rs — `set_connected` body
//   ~25 lines holding requested_connection.write().await across check-modify
```

After:

```rust
// services/qhy-focuser/src/codec.rs — new
pub struct QhyCodec;
impl Codec for QhyCodec {
    type Command = QhyCommand;     // existing
    type Response = QhyResponse;   // existing
    type Error = QhyFocuserError;
    fn encode(&self, cmd: &QhyCommand) -> Vec<u8> { /* serde_json */ }
    fn decode(&self, bytes: &[u8]) -> Result<QhyResponse, _> { /* serde_json */ }
    fn terminator(&self) -> u8 { b'}' }
    fn matches(&self, cmd: &QhyCommand, resp: &QhyResponse) -> bool {
        cmd.cmd_id() == resp.idx()
    }
    fn max_skip(&self) -> usize { 5 }    // unsolicited frames during move
}

// services/qhy-focuser/src/manager.rs — thin
pub struct FocuserManager {
    transport: Arc<SharedTransport<QhyCodec>>,
    cached_state: Arc<RwLock<CachedState>>,
    config: Config,
}

impl FocuserManager {
    pub fn new(config: Config, factory: Arc<dyn TransportFactory>) -> Arc<Self> {
        let cached_state = Arc::new(RwLock::new(CachedState::default()));
        let speed = config.speed;
        let cs_for_hs = cached_state.clone();
        let cs_for_poll = cached_state.clone();
        let poll_interval = config.polling_interval;

        let hooks = Hooks {
            handshake: Box::new(move |conn| {
                let cs = cs_for_hs.clone();
                Box::pin(async move {
                    let v = conn.request(QhyCommand::GetVersion).await?;
                    conn.request(QhyCommand::SetSpeed { speed }).await?;
                    let p = conn.request(QhyCommand::GetPosition).await?;
                    let t = conn.request(QhyCommand::ReadTemperature).await?;
                    let mut state = cs.write().await;
                    state.firmware_version = v.firmware;
                    state.position = Some(p.position);
                    state.outer_temp = Some(t.outer);
                    state.chip_temp = Some(t.chip);
                    state.voltage = t.voltage;
                    Ok(())
                })
            }),
            teardown: Box::new(|_conn| Box::pin(async {})),
            while_open: Some(Box::new(move |session, cancel| {
                let cs = cs_for_poll.clone();
                Box::pin(poll_loop(session, cancel, cs, poll_interval))
            })),
        };

        Arc::new(Self {
            transport: SharedTransport::new(factory, QhyCodec, hooks),
            cached_state,
            config,
        })
    }

    pub async fn move_absolute(&self, target: i64) -> Result<(), QhyFocuserError> { /* … */ }
    pub async fn abort(&self) -> Result<(), QhyFocuserError> { /* … */ }
    pub fn snapshot(&self) -> Arc<RwLock<CachedState>> { self.cached_state.clone() }
    pub fn transport(&self) -> &Arc<SharedTransport<QhyCodec>> { &self.transport }
}

// services/qhy-focuser/src/focuser_device.rs — `set_connected` body
async fn set_connected(&self, on: bool) -> ASCOMResult<()> {
    let mut s = self.session.write().await;
    match (on, s.is_some()) {
        (true, false) => *s = Some(
            self.manager.transport().acquire().await
                .map_err(QhyFocuserError::into_ascom)?
        ),
        (false, true) => *s = None,
        _ => {}
    }
    Ok(())
}
```

`poll_loop` (per-service) accepts the `Session`, the
`CancellationToken`, and the `cached_state` Arc. It does the same
`GetPosition` + `ReadTemperature` calls the current
`poll_position`/`poll_temperature` do, wrapped in
`tokio::select! { _ = cancel.cancelled() => break, _ = interval.tick() => {} }`.
The stale-frame retry budget is now part of the codec — `poll_loop`
just calls `session.request(...).await`.

Net effect: `serial_manager.rs` shrinks from ~600 lines to a ~120-line
`manager.rs` with the manager API; lifecycle scaffolding (~250 lines)
moves into the shared crate.

### `ppba-driver`

Two devices on one transport. After migration both
`PpbaSwitchDevice` and `PpbaObservingConditionsDevice` hold an
`Arc<PpbaManager>` and their own `session: RwLock<Option<Session<PpbaCodec>>>`.
Their `set_connected` bodies are identical to qhy-focuser's. The
`while_open` task runs both polls (`refresh_status` + `refresh_power_stats`)
into a `cached_state: Arc<RwLock<CachedState>>` that both devices
read.

Closes issue #251 inherently — the buggy `set_connected` body on
`main` ceases to exist (the new body takes its place). PR #255 is
superseded; we close it in favor of this migration if Phase A lands
in time, or we land PR #255 first and let Phase B simply replace its
fix with the new shape.

### `pa-falcon-rotator`

`pa-falcon-rotator` is the only service with no polling task (by
design — every property read is a fresh wire round-trip). After
migration its `Hooks` is `while_open: None`. The five-command
handshake (`F#`, `FV`, `DR:0`, `FA`, `VS`) becomes the handshake
closure. The two devices (`FalconRotatorDevice` and
`FalconStatusSwitchDevice`) each hold their own session.

Coordination: PR #241 is still `@wip` and not merged. The migration
applies cleanly once #241 lands, since #241 already has the canonical
lock-held shape. The migration PR for `pa-falcon-rotator` either
rebases onto post-#241 main, or — if scheduling demands — we ship
the migration of the three already-merged services first and migrate
`pa-falcon-rotator` after #241 lands.

### `star-adventurer-gti`

Two structural differences worth treating carefully:

* **Dual transport (USB serial + UDP).** The crate's
  `TransportFactory` abstraction was designed for this. SAG-GTI's
  `transport/serial.rs` and `transport/udp.rs` become
  `TransportFactory` impls; selection at `ServerBuilder::build()` time
  picks one based on `config.transport`.
* **Fallible post-acquire work in `set_connected`.** Currently
  `seed_home_pose_after_connect` and `load_park_target_after_connect`
  run after `transport.connect()` succeeds; each has an explicit
  `transport.disconnect()` rollback on failure. After migration:

```rust
async fn set_connected(&self, on: bool) -> ASCOMResult<()> {
    let mut s = self.session.write().await;
    match (on, s.is_some()) {
        (true, false) => {
            let session = self.manager.transport().acquire().await
                              .map_err(Self::ascom)?;
            self.seed_home_pose_after_connect(&session).await
                .map_err(Self::ascom)?;
            self.load_park_target_after_connect(&session).await
                .map_err(Self::ascom)?;
            *s = Some(session);
        }
        (false, true) => {
            *s = None;
            self.clear_session_state().await;
        }
        _ => {}
    }
    Ok(())
}
```

The explicit rollback code disappears: if either post-acquire call
errors, `session` falls out of scope, `Drop` releases the transport,
the underlying port closes. No more `tracing::warn!(...)` log noise
for failed-rollback-disconnect either, because the rollback is
structural now.

The `PollPauseGuard` pattern stays inside SAG-GTI's `while_open`
closure. The closure captures the guard's atomic; the poll body
`tokio::select!`s between cancellation, the interval tick, and the
pause-depth check, exactly as it does today.

The two `seed_*_position` helpers
(`TransportManager::seed_ra_position` / `seed_dec_position`) — which
let `Sync` and home-pose seeding update the cached snapshot
immediately rather than waiting for the next poll tick — become
methods on the service's `MountManager` (not on `SharedTransport`).
They mutate the manager's `snapshot: Arc<RwLock<MountSnapshot>>`
directly; `SharedTransport` doesn't know they exist.

## Phased rollout

Each phase is one PR, landing on `main` in order. Each PR is
independently `cargo rail run --profile commit -q` green.

### Phase A — Land `crates/shared-transport/`

* `crates/shared-transport/Cargo.toml`, `src/lib.rs`,
  `src/codec.rs`, `src/transport.rs`, `src/connection.rs`,
  `src/session.rs`, `src/shared.rs`, `src/error.rs`.
* `crates/shared-transport/tests/race.rs` — concurrent acquire
  test using an `AtomicU32`-backed stub `TransportFactory` and a
  trivial `Codec`. Two concurrent `acquire()` → exactly one call
  to factory.open().
* `crates/shared-transport/tests/rollback.rs` — factory errors,
  handshake errors, and post-handshake panics all leave count=0
  and `is_available()==false`. Subsequent `acquire()` re-runs the
  full open path (proved by the factory's call counter).
* `crates/shared-transport/tests/while_open.rs` — `while_open`
  task starts on first acquire, cancels on last release; the
  spawned task can issue requests through its session and is
  fully torn down before the next `acquire()` opens a fresh
  transport.
* `crates/shared-transport/tests/idempotent.rs` — two sequential
  `acquire()`s share one open; first drop keeps it open, second
  drop closes it.
* No service migrations in this PR — purely additive.

Verification: `cargo nextest run -p shared-transport --all-features --locked`
green; `cargo clippy -p shared-transport --all-features --all-targets -- -D warnings`
clean.

### Phase B — Migrate `ppba-driver`

`ppba-driver` first because:

1. It has the buggy `set_connected` shape on `main` (PR #255 fix is
   in flight). Phase B replaces the inline body with the new shape,
   closing #251 structurally.
2. It exercises the multi-device-on-one-transport case end to end.
3. It's the simplest of the four protocols (3-command handshake,
   ASCII LF, command-echo validation).

Removes:
* `services/ppba-driver/src/serial_manager.rs` — most of it.
* `services/ppba-driver/src/io.rs` — replaced by shared
  `TransportFactory`.

Adds:
* `services/ppba-driver/src/codec.rs` — `PpbaCodec` impl.
* Trims `serial_manager.rs` to a `PpbaManager` wrapper around
  `Arc<SharedTransport<PpbaCodec>>` + `cached_state` +
  protocol-specific public methods.
* `mock.rs` factory adopts shared `TransportFactory`.

Tests: existing unit + BDD scenarios stay green. Inline tests in
`serial_manager.rs` that probed `connection_count` directly get
rewritten to use the shared crate's `is_available()` (or get deleted
where they overlap with the shared crate's race/rollback tests).

Coordination with PR #255: if PR #255 lands first, Phase B simply
replaces its fix with the migrated body. If Phase A lands before
PR #255 ships, we close PR #255 and let Phase B be the fix for #251.

### Phase C — Migrate `qhy-focuser`

Second because:

1. Its codec is the most demanding — JSON framing + cmd_id↔idx
   match + stale-frame skip. Migrating it validates that the
   `Codec::matches` + `max_skip` design generalizes.
2. PR #260 (rollback fix for #258) should land first. Phase C then
   deletes the rollback code PR #260 adds, since
   `SharedTransport::acquire()` handles rollback structurally.

Removes:
* `services/qhy-focuser/src/serial_manager.rs` — most of it.
* `services/qhy-focuser/src/io.rs` — replaced.

Adds:
* `services/qhy-focuser/src/codec.rs`.
* Trimmed `manager.rs`.

Tests: existing BDD green. Inline `MockReader`-based unit tests for
`read_response_for` in `serial_manager.rs` are subsumed by Phase A's
codec tests; remove the redundant ones.

### Phase D — Migrate `pa-falcon-rotator`

Third because PR #241 has not yet merged. Phase D rebases onto
post-#241 main and applies the same pattern (Hooks with
`while_open: None`).

If PR #241 has not merged by the time Phase C lands, Phase D blocks
on #241; the other phases ship independently.

Removes / Adds: mirror Phases B and C structure.

Coordination: the migration of the two devices
(`FalconRotatorDevice` + `FalconStatusSwitchDevice`) follows the
ppba-driver shape.

### Phase E — Migrate `star-adventurer-gti`

Last because:

1. Largest service (~3700 lines in `mount_device.rs` alone).
2. Most BDD scenarios (~54).
3. Dual transport (USB + UDP) needs the `UdpDuplex` adapter exercised.
4. Post-acquire fallible work needs the structural-rollback story
   verified end to end.

Removes:
* `services/star-adventurer-gti/src/transport_manager.rs` — most of it.
* The explicit rollback-on-error branches in `set_connected`
  (`mount_device.rs:1083-1094`) — auto-released by Drop.

Adds:
* `services/star-adventurer-gti/src/codec.rs` — `SkywatcherCodec`
  wrapping the existing `skywatcher-motor-protocol` crate.
* `services/star-adventurer-gti/src/transport/udp.rs` adapts to
  the shared `TransportFactory` and uses `UdpDuplex` from
  `shared-transport`.
* `services/star-adventurer-gti/src/manager.rs` — `MountManager`
  with `seed_*_position`, `parameters`, `snapshot`, `poll_axes_now`,
  `pause_background_polling`.

Tests:
* Existing BDD (54 scenarios) green. Most are protocol-level and
  don't touch the transport-manager API.
* The `/debug/v1/mock-state` HTTP router stays — it's per-service
  test plumbing, untouched by this migration.
* `transport_manager.rs` unit tests that probed internals
  (`connection_count`, `serial_available`) get rewritten or
  deleted.

## Test strategy

### In the shared crate

The race, rollback, while-open lifecycle, and idempotent-acquire
tests in Phase A are written once with a generic `Codec` + an
`AtomicU32`-backed stub `TransportFactory`. They prove:

* Two concurrent `acquire()` → exactly one `factory.open()` (race).
* Factory error → count stays 0, `is_available()` stays false,
  next acquire re-opens.
* Handshake error → same as factory error; transport closes.
* Panic in handshake → same as error (catch_unwind in handshake
  driver, or `AbortOnDrop` semantics — TBD in Phase A).
* `while_open` task spawns on first acquire, cancels on last
  release, fully torn down before the next acquire reopens.
* Sequential `acquire()` then drop, then acquire — one open,
  one close, one re-open.

A `loom`-based test of the lock interleaving is **out of scope** for
Phase A but listed as a follow-up if regressions appear.

### Per-service

* Existing BDD scenarios stay green through every migration phase.
  Each phase's PR description must include a BDD pass log.
* Inline unit tests that test lifecycle invariants (e.g.
  `test_connect_factory_error`, `test_connect_bad_ping`) are
  redundant after migration — they exercise behavior the shared
  crate already tests once. Each migration deletes the redundant
  ones; if any test asserts service-specific failure-recovery
  behavior, it gets rewritten against the new manager API.
* `cargo rail run --profile commit -q` green at every commit.

## Risks & mitigations

| Risk | Severity | Mitigation |
|---|---|---|
| Closure storage in `Hooks` is unfamiliar Rust; trait objects with `BoxFuture` add complexity. | Med | Concrete example in shared-crate tests; keep `Hooks` impl Send + Sync trivial; document patterns in `crates/shared-transport/README.md`. |
| Async drop spawning a cleanup task requires a running tokio runtime. | Med | All `Session` drops happen in async context today (every `set_connected(false)` is `async fn`). Document the assumption in `Session::drop` rustdoc; panic loudly if no runtime. |
| Stale cleanup races a new acquire (count goes 1 → 0 → 1 quickly). | Low | `acquire_lock` serializes cleanup against new acquire; cleanup re-checks `count` after taking the lock and bails if a new owner is already in. |
| qhy-focuser's `matches`/`max_skip` codec hooks add cost (one extra method call per response) to every service. | Low | Default impls are no-ops; hot path is identical to today. Bench in Phase A. |
| Mock factory rewrites in Phases B–E touch many tests. | Med | The per-service `MockState` machinery stays; only the factory trait it implements changes. Migration is a rename + signature tweak per service. |
| Phase E (SAG-GTI) is large; risk of subtle behavior change. | Med | Phase E lands last so the shared crate is well-exercised by the other three services first. Hardware smoke-test against the real mount before merging Phase E. |
| `pa-falcon-rotator` PR #241 may not merge in time. | Low | Phase D depends on #241; the other phases ship independently. |

## Open questions

1. **Naming.** `shared-transport` vs `device-transport` vs `transport`?
   `shared-transport` is the working name in this plan; bikeshed in
   the Phase A PR if needed.
2. **`Session::send` policy when the codec's protocol does have a
   response.** Current proposal: `send` writes and returns; the
   protocol is responsible for guaranteeing no reply. Alternative:
   the codec exposes a per-command `expects_response()` method and
   `send` requires it to return `false`. The alternative adds
   type-system enforcement at the cost of a per-command method.
   Defer to Phase A.
3. **Whether `Hooks::teardown` should be allowed to fail.** Today
   `star-adventurer-gti::TransportManager::disconnect()` returns
   `Result<()>` because of the best-effort `:L1`/`:L2`/`:K1` halt
   sequence; the proposed `teardown` is infallible-by-design. The
   current code logs at `warn!` on failure and continues — i.e. the
   `Result<()>` was already best-effort. Treating teardown as
   infallible at the signature level matches that intent. Confirm
   in Phase A.
4. **Whether to land `pa-falcon-rotator` adoption inside PR #241
   itself** (so the service is born with the new shape) or after
   PR #241 merges as a separate migration PR. The author of #241
   decides; either way works.

## References

* Issue #257 — Extract shared connection-lifecycle helper for
  serial-based ASCOM services (this work).
* Issue #251 — `ppba-driver` `set_connected` race
  (closed structurally by Phase B).
* Issue #258 — `qhy-focuser` refcount leak on partial-connect
  failure (closed structurally by Phase C; PR #260 lands first as
  the interim inline fix).
* PR #241 — `pa-falcon-rotator` Phase 2 (introduces the canonical
  fix shape; Phase D depends on this merging).
* PR #255 — `ppba-driver` inline fix for #251 (superseded by
  Phase B if Phase A lands in time).
* PR #256 — `qhy-focuser` `set_connected` race fix
  (canonical lock-held shape, merged).
* PR #260 — `qhy-focuser` rollback fix for #258
  (interim; Phase C deletes the rollback code).
* `docs/services/qhy-focuser.md`,
  `docs/services/ppba-driver.md`,
  `docs/services/falcon-rotator.md`,
  `docs/services/star-adventurer-gti.md` —
  per-service design docs. Each gets a small update in its
  migration phase noting the move to `shared-transport`.
