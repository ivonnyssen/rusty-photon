# Shared Transport Extraction Plan (issue #257)

## Status

**Phase C in flight.** Phase A landed via PR #269 (the
`crates/rusty-photon-shared-transport/` crate; 31 tests). Phase B
migrated `ppba-driver` via PR #276; both ASCOM devices
(`PpbaSwitchDevice`, `PpbaObservingConditionsDevice`) now hold an
`Option<Session<PpbaCodec>>` and `ppba-driver/src/serial_manager.rs`
was deleted in favour of `PpbaManager` + `Hooks { handshake,
while_open, … }`. Issue #251 closed structurally with that migration.
Phase C migrates `qhy-focuser` to the shared crate on
`feature/phase-c-qhy-focuser-shared-transport`: `QhyCodec` carries the
JSON encode/decode + `cmd_id↔idx` matching (with `max_skip = 5` for
unsolicited position frames), `FocuserManager` wraps
`SharedTransport<QhyCodec>` + the cached state, and
`QhyFocuserDevice::set_connected` is the
`RwLock<Option<Session<QhyCodec>>>` shape. The legacy
`serial_manager.rs` (~1063 lines) and `io.rs` are deleted. Issue #258
closes structurally with this migration. All 110 unit + integration
tests + 37 BDD scenarios green; rail commit profile clean. Phases
D–E (pa-falcon-rotator, star-adventurer-gti) follow per the rollout
below.

## Motivation

Four services (`qhy-focuser`, `ppba-driver`, `pa-falcon-rotator`,
`star-adventurer-gti`) have grown independently to similar shapes for a
problem they all share: a single duplex transport, multiple in-process
clients (ASCOM devices + a background poll loop + slew/park watchers), a
connect-use-disconnect lifecycle. The shapes converged through
copy-paste and copy-paste-with-fix, not through a shared abstraction.

That convergence has cost us:

* **Lock-holding race in `set_connected`** — tracked per-service:
  issue #250 (`qhy-focuser`, fixed by PR #256), issue #251
  (`ppba-driver`, fix in flight as PR #255), and the
  `pa-falcon-rotator` Copilot review on PR #241 commit `8cd6e16`.
  Each fix is structurally identical: hold the `requested_connection`
  write lock across the entire check-and-modify. `ppba-driver` still
  has the defect on `main`. Issue #257 (this work) is the umbrella
  "extract a shared helper so this can't recur" issue.
* **Refcount + reader/writer leak on partial-connect failure
  (issue #258).** `qhy-focuser`'s `connect()` bumps the refcount and
  installs reader/writer *before* the handshake; any handshake error
  leaves the manager wedged until process restart. `ppba-driver` has
  the same defect. PR #260 fixes it for `qhy-focuser`;
  `pa-falcon-rotator` and `star-adventurer-gti` already roll back
  correctly. This is the more dangerous bug class — a single-client
  wedge on any handshake timeout, not a multi-client race.
* **Polling-task teardown leak** — speculative, not observed in
  production, but the same copy-paste shape that produced the first
  two classes. Each service today spawns a poll task on connect and
  is responsible for stopping it on disconnect, with no shared
  mechanism enforcing that the task's lifetime tracks the transport's.
  Listed here as preventive design pressure rather than as a defect
  we can cite.

The four `SerialManager` implementations now total roughly 1200 lines
of lifecycle scaffolding that says the same thing. Three of those four
were written by the same person to the same template; the fourth
(`star-adventurer-gti::TransportManager`) explicitly cites the others
as precedent. What looks like a shared pattern is actually parallel
implementations of the same pattern.

## Goals

* Extract a workspace crate `crates/rusty-photon-shared-transport/` that owns the
  pieces that are genuinely shared:
    1. A frame-oriented transport trait (`FrameTransport` with
       `send_frame` / `recv_frame`) so serial and UDP transports
       share an abstraction without losing datagram boundaries on
       UDP. Framing decisions (read-until-terminator, fixed-length,
       balanced-brace, one-datagram-per-frame) live inside each
       per-transport implementation, not on the codec.
    2. A `Codec` trait that operates on whole frames (encode command
       → bytes; decode bytes → response), with an optional
       stale-frame predicate for protocols that need it
       (`qhy-focuser`).
    3. Request arbitration (today's `command_lock`).
    4. Refcounted lifecycle (today's `connection_count` +
       `serial_available` + slot).
    5. A `Session<C>` handle whose `close().await` is the documented
       primary teardown path (synchronous from the caller's
       perspective); `Drop` is a best-effort safety net.
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
╔════════════ crates/rusty-photon-shared-transport ════════════════════╗
║ SharedTransport<C: Codec>                                            ║
║  - refcount + slot + open-state lock                                 ║
║  - acquire() → Session<C>  (close().await = primary release;         ║
║                              Drop = safety-net detached cleanup)     ║
║  - 0→1: open via factory → handshake → spawn while_open              ║
║  - 1→0: cancel while_open → teardown → close                         ║
║                                                                      ║
║ Session<C: Codec>                                                    ║
║  - request(cmd) → Result<Response, SessionError<C::Error>>           ║
║  - close(self).await → Result<(), TransportError>   (primary)        ║
║  - impl Drop  → detached best-effort cleanup (fallback)              ║
║                                                                      ║
║ WhileOpen<C: Codec>   (passed to while_open hook; not a Session)     ║
║  - request / cancelled()                                             ║
║  - does NOT participate in the external refcount                     ║
║                                                                      ║
║ Connection<C: Codec>   (internal, frame-level)                       ║
║  - owns Box<dyn FrameTransport> + codec                              ║
║  - holds the request-arbitration lock                                ║
║                                                                      ║
║ trait Codec          { encode, decode, [matches, max_skip] }         ║
║ trait FrameTransport { send_frame, recv_frame }                      ║
║ trait TransportFactory { open() → Box<dyn FrameTransport> }          ║
║                                                                      ║
║ enum SessionError<E> { Transport(TransportError), Codec(E) }         ║
╚══════════════════════════════════════════════════════════════════════╝
```

### What lifts to the shared crate

* `FrameTransport` trait — `send_frame` / `recv_frame`, frame-oriented
  so UDP and serial share an abstraction without losing datagram
  boundaries.
* `TransportFactory` trait — opens a boxed `FrameTransport`.
* `Codec` trait — frame-level encode / decode, plus optional
  stale-frame predicate.
* `Connection<C>` — request arbitration; calls `send_frame` /
  `recv_frame` and encode / decode in lockstep.
* `SharedTransport<C>` — refcount, slot, open/close, while-open task
  lifecycle, explicit `close()` path with `Drop` fallback.
* `Session<C>` — request / close API.
* `WhileOpen<C>` — non-refcounted context passed to the while_open
  hook; same request API as `Session`.
* `Hooks<C>` — handshake / teardown / while_open closures.
* `SessionError<E>` — discriminates transport vs codec failures.

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
pub trait Codec: Send + Sync + Clone + 'static {
    type Command: Send + Sync;
    type Response: Send;
    type Error: std::error::Error + Send + Sync + 'static;

    /// Encode a command into one whole frame's worth of bytes,
    /// including any in-frame terminator the wire protocol requires
    /// (qhy's `}`, the Sky-Watcher `\r`). The transport reads
    /// frames in chunks defined by that same terminator and returns
    /// the bytes verbatim — the codec sees the same bytes that
    /// flowed on the wire.
    fn encode(&self, cmd: &Self::Command) -> Vec<u8>;

    /// Decode one whole response frame's bytes into a typed response.
    fn decode(&self, bytes: &[u8]) -> Result<Self::Response, Self::Error>;

    /// Return true iff `resp` is the response to `cmd`. Default is
    /// always-true (matches the immediately preceding request).
    /// qhy-focuser overrides this to compare cmd_id↔idx so that
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

### `FrameTransport` + `TransportFactory`

```rust
#[async_trait]
pub trait FrameTransport: Send {
    /// Send one complete frame. The bytes are written verbatim — the
    /// caller (i.e. the [`Codec`]) includes any in-frame terminator
    /// the wire protocol requires. For UDP: one `send` call (one
    /// datagram).
    async fn send_frame(&mut self, bytes: &[u8]) -> Result<(), TransportError>;

    /// Receive one complete frame's bytes into `buf` (overwriting it).
    /// For serial: read-until-terminator; the terminator is
    /// **included** in the result (qhy's `}`, the Sky-Watcher `\r`
    /// are wire-protocol content, not pure framing delimiters, so
    /// every codec we ship today wants to see them).
    /// For UDP: one `recv` call (one datagram); the whole datagram is
    /// the frame.
    async fn recv_frame(&mut self, buf: &mut Vec<u8>) -> Result<(), TransportError>;
}

#[async_trait]
pub trait TransportFactory: Send + Sync + 'static {
    async fn open(&self) -> Result<Box<dyn FrameTransport>, TransportError>;
}
```

Per-transport framing decisions live on the transport implementation:

* `crates/rusty-photon-shared-transport` provides a `SerialFrameTransport` helper
  that wraps any `AsyncRead + AsyncWrite + Unpin + Send` (e.g.
  `tokio_serial::SerialStream`) plus a configurable terminator byte
  and a max-frame-size guard. ppba/pa-falcon use LF; SAG-GTI's serial
  factory uses CR; qhy-focuser uses `b'}'` (qhy responses are flat
  JSON objects — that assumption is documented at the qhy-focuser
  layer, not lifted into the shared crate). If a future codec needs
  balanced-brace or length-prefix framing, it ships its own
  `FrameTransport` impl without changing anything cross-cutting.

* `crates/rusty-photon-shared-transport` provides a `UdpFrameTransport` that wraps
  `tokio::net::UdpSocket` with `connect()` set to the peer address.
  `send_frame` is one `send` call; `recv_frame` is one `recv` call
  into the supplied buffer. Datagram boundaries are preserved by
  construction.

### `SessionError<E>`

```rust
pub enum SessionError<E: std::error::Error + Send + Sync + 'static> {
    /// Wire-level I/O failure: factory open, broken pipe, read
    /// timeout, EOF, framing error before reaching `Codec::decode`.
    Transport(TransportError),

    /// Codec-level failure: malformed response, mismatched checksum,
    /// or a frame the codec couldn't translate into a typed response.
    Codec(E),

    /// Read `Codec::max_skip` + 1 frames without one passing
    /// `Codec::matches`. The device fell out of sync or the codec's
    /// `matches` predicate is wrong.
    SkipExhausted(usize),
}

impl<E> From<TransportError> for SessionError<E> { /* … */ }
```

`SkipExhausted` is the third variant the implementation needed: when
the connection-layer skip budget is exhausted, that's neither a
transport failure (frames flowed fine) nor a codec failure (each
decode succeeded). The variant carries the number of frames that
were read.

Each service maps `SessionError<C::Error>` into its existing
service-wide error enum at the `Manager` public-API boundary (e.g.
`QhyFocuserError::from(SessionError<QhyCodecError>)`). The shared
crate's tests can assert on the variant directly to distinguish
transport vs codec failures without parsing strings.

### `Session<C>` and `WhileOpen<C>`

```rust
pub struct Session<C: Codec> { /* Arc<SharedTransport<C>> + slot id */ }

impl<C: Codec> Session<C> {
    pub async fn request(&self, cmd: C::Command)
        -> Result<C::Response, SessionError<C::Error>>;

    /// Primary teardown path. If this is the last live session,
    /// awaits while_open cancellation, runs `hooks.teardown`, closes
    /// the transport — all before returning. Callers see today's
    /// observable behavior (rollback complete before the call
    /// returns).
    pub async fn close(self) -> Result<(), TransportError>;
}

impl<C: Codec> Drop for Session<C> {
    /// Fallback only. Decrements the refcount synchronously; if this
    /// is the last session, spawns an `acquire_lock`-guarded detached
    /// cleanup task on the current tokio runtime. Best-effort: if the
    /// runtime is shutting down or no runtime is available, teardown
    /// commands (e.g. SAG-GTI's halt sequence) may not run. Document
    /// in rustdoc: "for graceful teardown, call `close().await`."
    fn drop(&mut self) { /* … */ }
}

/// Non-refcounted context handed to the while_open hook. Same
/// request API as Session, but its drop does NOT decrement the
/// shared transport's external refcount — it's infrastructure, not a
/// client. The Connection it wraps is shared via `Arc` with the
/// primary `acquire()` path; both go through the same request
/// arbitration lock.
pub struct WhileOpen<C: Codec> { /* Arc<Connection<C>> + CancellationToken */ }

impl<C: Codec> WhileOpen<C> {
    pub async fn request(&self, cmd: C::Command)
        -> Result<C::Response, SessionError<C::Error>>;

    /// Future that resolves when the surrounding SharedTransport
    /// starts teardown. Poll loops `tokio::select!` between this and
    /// their interval tick.
    pub fn cancelled(&self) -> impl Future<Output = ()> + '_;
}
```

`request` writes one frame via `Connection::send_frame` (which calls
`Codec::encode` + `FrameTransport::send_frame`), then reads frames
via `recv_frame` + `Codec::decode` until one passes `matches()` or
`max_skip` is exhausted. The connection's command lock is held for
the whole transaction. Transport errors surface as
`SessionError::Transport`; codec errors as `SessionError::Codec`.

Today every wire request has a wire response, so there is no
fire-and-forget `send` on `Session` or `WhileOpen`. If a future
protocol needs one, it gets designed against that protocol's
real semantics (including whether `Codec::expects_response(&cmd)` is
worth carrying for type-system enforcement).

`close(self).await` is the documented primary teardown path. If you
hold the last session, awaiting close means rollback completes before
your call returns — matching today's behavior. Drop is only the
"oops I forgot" safety net.

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
    /// Signature is infallible at the type level because no current
    /// service propagates teardown errors (SAG-GTI's `Result<()>` is
    /// log-and-continue today).
    pub teardown: Box<
        dyn Fn(&Connection<C>) -> BoxFuture<'_, ()>
            + Send + Sync,
    >,

    /// Optional: spawned after handshake succeeds; receives a
    /// `WhileOpen<C>` context (NOT a Session — see API design above).
    /// The closure cooperates with `WhileOpen::cancelled()` by
    /// returning promptly when the token fires. Awaited with a
    /// bounded timeout (5s) during the 1→0 transition; on timeout
    /// the JoinHandle is `abort()`-ed and teardown proceeds
    /// regardless. A panicking task is treated the same way (the
    /// JoinHandle resolves to Err; teardown runs).
    pub while_open: Option<Box<
        dyn Fn(WhileOpen<C>) -> BoxFuture<'static, ()>
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

    pub async fn acquire(self: &Arc<Self>)
        -> Result<Session<C>, SessionError<C::Error>>;

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
* `acquire_lock: Mutex<()>` — serialises every open/close transition.
  Held across the entire 0→1 path in `acquire`, and across the entire
  release path in both `Session::close` and `Session::drop`'s spawned
  cleanup. This is intentional: if release took `fetch_sub` *before*
  the lock, a new `acquire` racing in could see `count == 0` and
  start opening a fresh transport while the old one's slot was still
  populated, leading to a double-open of the underlying serial port.
  Acquires are rare; this is not a hot path.
* `while_open_state: Mutex<Option<(JoinHandle<()>, CancellationToken)>>`
  — present iff a while-open task is running.

`acquire()`:
1. Take `acquire_lock`.
2. `let prev = count.fetch_add(1, SeqCst);`
3. If `prev == 0` (0→1 transition under a `RollbackGuard` that fires
   on any `?` early-return *or* panic in steps 3.1–3.3):
    1. `factory.open()` → boxed `FrameTransport`. (Guard active.)
    2. Wrap in `Arc::new(Connection::new(transport, codec.clone()))`.
       (Guard active.)
    3. Run `hooks.handshake(&conn).await`. On `Err`: guard's `Drop`
       fires `count.fetch_sub(1)`, the Arc<Connection> drops at
       function exit, transport closes, return `SessionError::Codec`.
       (Guard active.)
    4. Store `connection.clone()` in `slot`. Disarm rollback guard.
    5. `available.store(true, SeqCst)`.
    6. If `hooks.while_open` is `Some`: construct a `WhileOpen<C>`
       wrapping `Arc<Connection<C>>` + a fresh `CancellationToken`
       (does **not** touch `count`), spawn the closure with it,
       store the `JoinHandle` + token in `while_open_state`.
4. Else (`prev > 0`, reuse path): clone the slot's `Arc<Connection<C>>`.
5. Return `Session<C>`. `acquire_lock` releases at end of function.

`Session::close(self).await` (**primary teardown path**):
1. Drop the `Session`'s inner `Arc<Connection>` clone so the slot
   isn't sharing with stale references.
2. Call `transport.release_inline().await`:
    1. Take `acquire_lock`. (Held for the entire body — see above.)
    2. `let prev = count.fetch_sub(1, SeqCst);`
    3. If `prev > 1`: release lock, return `Ok(())` (another session
       keeps the transport open).
    4. Otherwise (`prev == 1`, run cleanup):
        1. `available.store(false, SeqCst)`.
        2. If `while_open_state` is `Some`: fire the cancellation
           token, await the `JoinHandle` with a bounded 5s timeout
           (`Duration::from_secs(5)`); on timeout call
           `handle.abort()`. Panicking task is the same path
           (JoinHandle resolves to Err immediately; warning logged).
        3. Take the connection out of `slot`. Run `hooks.teardown(&conn).await`.
        4. Drop the connection — `FrameTransport` drops, the OS-level
           conduit closes.
        5. Return `Ok(())` (or `Err(TransportError)` if a future
           hook surfaces one).

`Session::drop` (**fallback / safety net**):
1. If the `Session`'s `transport` field is still `Some` (i.e. `close`
   wasn't called), `take()` it and call `release_detached(self)` on
   the moved `Arc<SharedTransport<C>>`.
2. `release_detached` spawns `release_inline()` on the current tokio
   runtime as a fire-and-forget task. (`release_inline` is the same
   body as for `close`, so the decrement + cleanup is identical.)
3. Documented limitation: if no tokio runtime is current
   (`Handle::try_current()` returns `Err`), the refcount is **not**
   decremented and teardown is **not** run; a `warn!` is logged.
   Service migrations call `close().await` on the disconnect path;
   Drop catches programmer error, not the steady-state path.

The `acquire_lock` makes the open/close transitions atomic with
respect to each other. The fast path (acquire when `count > 0`) takes
the lock, increments, and returns — microseconds. The lock is NOT
held by `Session` during `request` — only by the
acquire/release boundary.

### Error model

`rusty-photon-shared-transport` defines a small `TransportError` enum for
`TransportFactory::open` failures and for `FrameTransport::send_frame`
/ `recv_frame` failures (broken pipe, read timeout, EOF, framing
error before reaching `Codec::decode`). `Session::request` returns
`Result<_, SessionError<C::Error>>` where `SessionError`
discriminates `Transport(TransportError)` from `Codec(C::Error)`. No
implicit `From<TransportError>` bound is required on `C::Error`; the
shared crate exposes the discriminated union and each service
flattens it into its existing service-wide error enum at the
`Manager` public-API boundary.

`Session::close` returns `Result<(), TransportError>` — codec errors
are not in scope for teardown (teardown is logging-only by hook
contract).

## How the bug classes dissolve

### Race (issues #250 and #251)

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
        (true, false) => {
            *s = Some(
                self.transport.acquire().await
                    .map_err(Self::to_ascom)?
            );
        }
        (false, true) => {
            if let Some(session) = s.take() {
                session.close().await.map_err(Self::to_ascom)?;
            }
        }
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
    type Error = QhyCodecError;    // new: parse/JSON errors only
    fn encode(&self, cmd: &QhyCommand) -> Vec<u8> { /* serde_json */ }
    fn decode(&self, bytes: &[u8]) -> Result<QhyResponse, _> { /* serde_json */ }
    fn matches(&self, cmd: &QhyCommand, resp: &QhyResponse) -> bool {
        cmd.cmd_id() == resp.idx()
    }
    fn max_skip(&self) -> usize { 5 }    // unsolicited frames during move
}

// services/qhy-focuser/src/transport.rs — new
// Builds a SerialFrameTransport with terminator b'}' (qhy responses
// are flat JSON objects; documented assumption at this layer).
// Implements TransportFactory.
pub struct QhyTransportFactory { /* … */ }
impl TransportFactory for QhyTransportFactory { /* … */ }

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
        (false, true) => {
            if let Some(session) = s.take() {
                session.close().await
                    .map_err(QhyFocuserError::into_ascom)?;
            }
        }
        _ => {}
    }
    Ok(())
}
```

`poll_loop` (per-service) accepts the `WhileOpen<QhyCodec>` context.
It does the same `GetPosition` + `ReadTemperature` calls the current
`poll_position`/`poll_temperature` do, wrapped in
`tokio::select! { _ = ctx.cancelled() => break, _ = interval.tick() => {} }`.
The stale-frame retry budget is part of the codec — `poll_loop`
just calls `ctx.request(...).await`.

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

Coordination: PR #241 (Phase 2 scaffold, `@wip`) lands as-is with
the canonical lock-held shape; pa-falcon-rotator Phase 3a-3e then
ships the protocol implementation against `serial_manager.rs` in the
existing per-service shape. Phase D (below) then deletes
`serial_manager.rs` and rewrites `set_connected` against
`rusty-photon-shared-transport`. This trades some throwaway lifecycle code in
Phase 3 for unblocking PR #241's timeline and keeping the Phase 3
PRs focused on protocol correctness rather than lifecycle plumbing.

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
            // Post-acquire fallible work. On error, close the
            // session synchronously before propagating, preserving
            // today's behavior where rollback completes before the
            // ASCOM call returns.
            if let Err(e) = self.seed_home_pose_after_connect(&session).await {
                session.close().await.map_err(Self::ascom)?;
                return Err(Self::ascom(e));
            }
            if let Err(e) = self.load_park_target_after_connect(&session).await {
                session.close().await.map_err(Self::ascom)?;
                return Err(Self::ascom(e));
            }
            *s = Some(session);
        }
        (false, true) => {
            if let Some(session) = s.take() {
                session.close().await.map_err(Self::ascom)?;
            }
            self.clear_session_state().await;
        }
        _ => {}
    }
    Ok(())
}
```

The post-acquire rollback path stays explicit but becomes uniform:
on any failure between `acquire()` and the final `*s = Some(...)`,
call `session.close().await` synchronously and propagate. If the
caller forgets and just lets `session` drop, the Drop fallback fires
detached cleanup — best-effort, log-level halt commands only. The
`tracing::warn!(...)` for failed-rollback-disconnect that exists
today (`mount_device.rs:1085, 1091`) goes away because `close()`
surfaces the error to the caller instead of swallowing it.

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

### Phase A — Land `crates/rusty-photon-shared-transport/`

Status: **implemented in PR #269**, awaiting review.

* `crates/rusty-photon-shared-transport/Cargo.toml`, `BUILD.bazel`, `src/lib.rs`,
  `src/codec.rs`, `src/transport.rs`, `src/connection.rs`,
  `src/session.rs`, `src/shared.rs`, `src/error.rs`.
* `crates/rusty-photon-shared-transport/tests/common/mod.rs` — shared `EchoCodec`,
  `ProgrammableFactory`, `EchoTransport`, `CountingHooks`, and
  `WhileOpenHooks` (cooperative + stubborn variants) used by all
  four integration test files.
* `crates/rusty-photon-shared-transport/tests/race.rs` — concurrent acquire
  test (3 tests): two and many concurrent `acquire()` → exactly
  one factory.open() + exactly one handshake invocation.
* `crates/rusty-photon-shared-transport/tests/rollback.rs` — 5 tests covering
  factory error, recovered factory, handshake error, handshake
  panic (`RollbackGuard` covers the unwind), and an alternating
  failure/success sequence. All paths leave count=0 and
  `is_available()==false`; subsequent `acquire()` re-runs the full
  open path (proved by `cfg.opens()` and `cfg.dropped_count()`).
* `crates/rusty-photon-shared-transport/tests/while_open.rs` — 5 tests: task
  starts after handshake; exits when the last session closes;
  persists across multiple sessions sharing one transport;
  respawns after a close/reopen cycle; stubborn task is `abort()`-ed
  after the 5s timeout (under `tokio::test(start_paused = true)`).
* `crates/rusty-photon-shared-transport/tests/idempotent.rs` — 5 tests: two
  sequential acquires share one open; close-then-reacquire runs
  handshake again; `Drop` fallback decrements + runs teardown;
  non-last `close()` returns fast (< 100ms) without teardown;
  `is_available()` tracks lifecycle.
* Workspace edits: `tokio-util` lifted to `workspace.dependencies`
  (now shared by `sentinel` and `rusty-photon-shared-transport`);
  `rusty-photon-shared-transport` added to workspace members and to
  `workspace.dependencies` as a path dep.
* No service migrations in this PR — purely additive.

Implementation choices not in the original plan but locked in
during Phase A:

* `Codec: Clone` added to the trait bound; `SharedTransport` stamps
  a fresh codec onto each `Connection` on every 0→1 transition.
  Cheap to satisfy (codecs are typically ZST or hold small config).
* `Codec::Error` requires `std::error::Error + Send + Sync + 'static`
  so it composes cleanly with `thiserror`-derived service error
  enums.
* `SessionError::SkipExhausted(usize)` added as a third variant —
  the connection-layer skip budget is neither a transport failure
  (frames flowed) nor a codec failure (each decode succeeded), and
  the carried `usize` is the number of frames read before the
  budget tripped.
* `SerialFrameTransport::recv_frame` returns frames with the
  terminator byte **included** — every codec we ship today wants
  to see it (qhy's `}` is JSON-content, Sky-Watcher's `\r` is
  validated by the existing `validate_response_frame` helper).
  `send_frame` writes bytes verbatim; the codec is responsible for
  including the terminator in encoded output. Symmetric and
  matches UDP's "the datagram is the frame" model.
* `acquire_lock` is held across the **full** `release_inline`
  path (not just the cleanup body), so a racing new `acquire()`
  cannot start opening a fresh transport while the old transport's
  slot is still populated. The plan originally split this; the
  implementation found the split allowed a double-open race.
* Panic rollback uses a `RollbackGuard` whose `Drop` fires
  `count.fetch_sub(1)` on any unwind through the 0→1 path —
  covers handshake panics structurally, no `catch_unwind`.

Verification:
* `cargo nextest run -p rusty-photon-shared-transport --all-features --all-targets --locked`:
  31/31 green.
* `cargo clippy -p rusty-photon-shared-transport --all-features --all-targets --locked -- -D warnings`:
  clean.
* `cargo doc -p rusty-photon-shared-transport --all-features --no-deps`: clean.
* `cargo fmt --all --check`: clean.
* `cargo rail run --profile commit -q`: clean (rail's --merge-base
  detection sees only `sentinel` as a "changed" existing crate;
  `rusty-photon-shared-transport` runs explicitly).

### Phase B — Migrate `ppba-driver`

Status: **implemented** on `feature/phase-b-ppba-shared-transport`.

`ppba-driver` migrated first because:

1. It had the buggy `set_connected` shape on `main` (lock-held check-and-modify
   was the in-flight fix tracked by issue #251). Phase B replaces that
   inline body with the new "session-is-the-resource" shape, closing
   #251 structurally — there is no separate "requested" bool that can
   desync from the transport refcount.
2. It exercises the multi-device-on-one-transport case end to end
   (Switch + ObservingConditions both share one `Arc<PpbaManager>` and
   each hold their own `Option<Session<PpbaCodec>>`).
3. It's the simplest of the four protocols (3-command handshake,
   ASCII LF, command-echo validation).

Removed:
* `services/ppba-driver/src/serial_manager.rs` — entirely.
* `services/ppba-driver/src/io.rs` — replaced by shared
  `TransportFactory`.

Added:
* `services/ppba-driver/src/codec.rs` — `PpbaCodec` with `PpbaResponse`
  (`PingOk` / `Status(PpbaStatus)` / `PowerStats(PpbaPowerStats)` /
  `Echo(String)`) and `PpbaCodecError`. `max_skip` defaults to 0
  (PPBA does not emit unsolicited frames).
* `services/ppba-driver/src/manager.rs` — `PpbaManager` wraps
  `Arc<SharedTransport<PpbaCodec>>` + `Arc<RwLock<CachedState>>` and
  exposes session-borrowing helpers (`send_command`,
  `refresh_status`, `refresh_power_stats`) plus cache mutators
  (`set_averaging_period`, `set_usb_hub_state`).
* `services/ppba-driver/src/serial.rs` — `PpbaTransportFactory`
  building a `SerialFrameTransport` over `tokio-serial`.
* `services/ppba-driver/src/mock.rs` — `MockPpbaTransportFactory`
  implementing `TransportFactory` directly (no more `SerialReader` /
  `SerialWriter` split).

Tests: 117 unit + 145 BDD scenarios pass. Race / rollback / while-open
invariants are tested once in `rusty-photon-shared-transport`'s own
test suite; per-service duplicates were dropped per the original plan.

### Phase C — Migrate `qhy-focuser`

Status: **implemented** on `feature/phase-c-qhy-focuser-shared-transport`.

`qhy-focuser` migrated second because:

1. Its codec is the most demanding — JSON framing + cmd_id↔idx
   match + stale-frame skip. Migrating it validates that the
   `Codec::matches` + `max_skip` design generalizes.
2. PR #260 (rollback fix for #258) landed first; Phase C deletes
   the lifecycle rollback code PR #260 added, since
   `SharedTransport::acquire()` handles rollback structurally.

Removed:
* `services/qhy-focuser/src/serial_manager.rs` — entirely (~1063 lines).
* `services/qhy-focuser/src/io.rs` — replaced by shared
  `TransportFactory`.

Added:
* `services/qhy-focuser/src/codec.rs` — `QhyCodec` with `QhyResponse`
  (`Version` / `Position` / `Temperature` / `Ack { idx }`) and
  `QhyCodecError`. `max_skip = 5` (the legacy
  `MAX_RESPONSE_RETRIES`); `matches` compares `cmd.cmd_id() ==
  resp.idx()`.
* `services/qhy-focuser/src/manager.rs` — `FocuserManager` wraps
  `Arc<SharedTransport<QhyCodec>>` + `Arc<RwLock<CachedState>>` and
  exposes session-borrowing helpers (`move_absolute`, `abort`,
  `refresh_position`) plus the `Hooks { handshake, teardown,
  while_open }` constructor.
* `services/qhy-focuser/src/serial.rs` rewritten as
  `QhyTransportFactory` over `tokio-serial`, returning a
  `SerialFrameTransport` with `b'}'` as the frame terminator.
* `services/qhy-focuser/src/mock.rs` — `MockQhyTransportFactory`
  implementing `TransportFactory` directly (no more
  `SerialReader`/`SerialWriter` split); now compiled under
  `cfg(any(feature = "mock", test))` so unit tests can drive the
  canonical mock.

Tests: 110 unit + integration tests + 37 BDD scenarios pass. Race /
rollback / while-open invariants are tested once in
`rusty-photon-shared-transport`'s own test suite; per-service inline
duplicates were dropped per the original plan.

### Phase D — Migrate `pa-falcon-rotator`

Third because PR #241 (Phase 2 scaffold) and pa-falcon-rotator
Phase 3a-3e (protocol implementation) need to land first. Phase D
rebases onto post-Phase-3 main and applies the same pattern (Hooks
with `while_open: None`), deleting the canonical-lock-shape
`set_connected` and `serial_manager.rs` that Phase 3 ships.

If Phase 3 has not landed by the time Phase C ships, Phase D blocks
on it; the other phases ship independently.

Removes / Adds: mirror Phases B and C structure.

Coordination: the migration of the two devices
(`FalconRotatorDevice` + `FalconStatusSwitchDevice`) follows the
ppba-driver shape.

### Phase E — Migrate `star-adventurer-gti`

Last because:

1. Largest service (~3700 lines in `mount_device.rs` alone).
2. Most BDD scenarios (~54).
3. Dual transport (USB + UDP) needs the `UdpFrameTransport` adapter exercised.
4. Post-acquire fallible work needs the structural-rollback story
   verified end to end.

Removes:
* `services/star-adventurer-gti/src/transport_manager.rs` — most of it.
* The `tracing::warn!(...)` log-on-rollback-disconnect-failure
  branches in `set_connected` (`mount_device.rs:1085, 1091`).
  Rollback now calls `session.close().await` and propagates errors;
  no swallowed warnings.

Adds:
* `services/star-adventurer-gti/src/codec.rs` — `SkywatcherCodec`
  wrapping the existing `skywatcher-motor-protocol` crate.
* `services/star-adventurer-gti/src/transport/udp.rs` adapts to
  the shared `TransportFactory` and uses `UdpFrameTransport` from
  `rusty-photon-shared-transport` (one `recv` per frame; datagram boundaries
  preserved).
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
tests in Phase A are written once over a generic `Codec` (`EchoCodec`,
treats `Vec<u8>` as both command and response) and an `AtomicU32`-backed
stub `ProgrammableFactory` (controllable success/failure, per-transport
drop-flag tracking). The integration tests are split across four files
in `crates/rusty-photon-shared-transport/tests/`:

* `race.rs` (3 tests) — two and many concurrent `acquire()` → exactly
  one `factory.open()`; handshake also runs exactly once.
* `rollback.rs` (6 tests) — factory error, recovery after factory
  error, handshake error (transport drops), handshake panic
  (`RollbackGuard` covers the unwind path), while-open closure panic
  (rollback fires before slot/`available` publish), alternating
  failure/success.
* `while_open.rs` (5 tests) — task starts after handshake, exits on
  last close, persists across multiple sessions, respawns after a
  full close/reopen cycle, stubborn task gets `abort()`-ed after the
  bounded 5s timeout (under `tokio::test(start_paused = true)`).
* `idempotent.rs` (5 tests) — sequential reuse shares one open;
  close-then-reacquire runs handshake again; `Drop` fallback path
  decrements + closes; non-last close returns fast; `is_available()`
  tracks lifecycle.

Total: 19 integration tests + 12 inline tests in `transport.rs` and
`error.rs` (framing/timeout/error-display behaviour on
`SerialFrameTransport` and `UdpFrameTransport`).

Panic-in-handshake is handled by a `RollbackGuard` whose `Drop` runs
the count rollback whether the function exits via `?` *or* unwinds —
not by `std::panic::catch_unwind`. The unwind continues past
`acquire`, the spawning task sees the panic (test reads
`JoinHandle::await` and asserts `Err`), and `count == 0`,
`is_available() == false` are observable by subsequent acquires
through the same `SharedTransport`.

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
| Closure storage in `Hooks` is unfamiliar Rust; trait objects with `BoxFuture` add complexity. | Med | Concrete example in shared-crate tests; keep `Hooks` impl Send + Sync trivial; document patterns in the crate-level rustdoc (`crates/rusty-photon-shared-transport/src/lib.rs` and the `Hooks` doc comment in `session.rs`). |
| `Session::drop` fallback spawns a cleanup task and needs a tokio runtime; if the runtime is shutting down, teardown commands (e.g. SAG-GTI's halt sequence) may not run. | Med | Service migrations always call `close().await` on the disconnect path. Drop is for "caller forgot." Document the limitation in `Session::drop` rustdoc; the explicit-close-is-primary design (vs Drop-is-primary) makes this an "oops" path, not the steady-state path. |
| Misbehaving `while_open` task ignores cancellation and times out; teardown then aborts the JoinHandle and proceeds while the task may still be running. | Low | Connection's request-arbitration lock is released when the connection drops, so an abort()-stuck task can't deadlock teardown. Tests in Phase A cover the panic/timeout paths explicitly. |
| Stale cleanup races a new acquire (count goes 1 → 0 → 1 quickly). | Low | `acquire_lock` serializes cleanup against new acquire. The Phase A implementation takes the lock **before** `count.fetch_sub` (not after, as originally sketched) so a new acquire can't observe `count == 0` while the old slot is still populated — closing the only opening the test bench found for the race. |
| qhy-focuser's `matches`/`max_skip` codec hooks add cost (one extra method call per response) to every service. | Low | Default impls are no-ops; hot path is identical to today. Bench in Phase A. |
| Mock factory rewrites in Phases B–E touch many tests. | Med | The per-service `MockState` machinery stays; only the factory trait it implements changes (`SerialPortFactory` → `TransportFactory`, returning `Box<dyn FrameTransport>`). Migration is a rename + signature tweak per service. |
| Phase E (SAG-GTI) is large; risk of subtle behavior change. | Med | Phase E lands last so the shared crate is well-exercised by the other three services first. Hardware smoke-test against the real mount before merging Phase E. |
| `pa-falcon-rotator` PR #241 may not merge in time. | Low | Phase D depends on #241; the other phases ship independently. |

## Open questions

None remaining.

Resolved during the Copilot review on PR #265:

* **`Hooks::teardown` fallibility** — settled as infallible at the
  type level (`-> BoxFuture<()>`), matching today's log-and-continue
  behavior in every service. Errors that need to reach the caller
  surface through `Session::close()`'s `Result<(), TransportError>`,
  not through the teardown hook.
* **Error model** — settled as explicit
  `SessionError<E> { Transport(TransportError), Codec(E) }`. No
  implicit `From<TransportError>` bound on `C::Error`.
* **Framing on `Codec` vs transport** — settled: framing lives on
  the per-transport `FrameTransport` impl, not on `Codec`. Codec
  operates on whole frames.
* **`AsyncRead + AsyncWrite` vs frame-oriented transport** — settled
  as `FrameTransport { send_frame, recv_frame }` to preserve UDP
  datagram boundaries.
* **Drop-only vs explicit-close teardown** — settled as explicit
  `Session::close().await` primary; Drop is the safety net.

Resolved 2026-05-18 (the three questions originally deferred to
Phase A):

* **Crate naming.** `rusty-photon-shared-transport`. The
  `rusty-photon-*` prefix matches the convention already in use for
  workspace-wide infrastructure crates (`rusty-photon-i18n`,
  `rusty-photon-i18n-derive`) — the `rp-*` prefix is reserved for
  crates supporting the `rp` service specifically, and the
  unprefixed slots (`bdd-infra`, `skywatcher-motor-protocol`) are
  test infrastructure / external-protocol encodings. "Shared"
  captures the refcounted multi-client value-add that distinguishes
  the crate from a thin `tokio_serial` / `UdpSocket` wrapper.
  `serial-transport` and unprefixed `shared-transport` were both
  considered and rejected — the former misleads about UDP support
  (`UdpFrameTransport` is a first-class adapter, exercised by
  `star-adventurer-gti`'s `transport/udp.rs` today); the latter
  breaks the existing workspace prefix convention.
* **`Session::send` policy.** Dropped from the Phase A API surface
  entirely. Today every wire request has a wire response, so a
  fire-and-forget `send` would be forward-compat plumbing for a
  use case that does not exist. When the first protocol with
  response-less commands shows up, design `send` against that
  protocol's real semantics — including whether
  `Codec::expects_response(&cmd)` is worth carrying for
  type-system enforcement.
* **pa-falcon-rotator timing.** PR #241 (Phase 2 scaffold,
  `@wip`) lands as-is. pa-falcon-rotator Phase 3a-3e ships the
  protocol implementation against canonical-lock-shape
  `serial_manager.rs`. Phase D then replaces the lifecycle
  scaffolding with `rusty-photon-shared-transport`. Trades some throwaway
  Phase 3 lifecycle code for unblocking PR #241's timeline and
  keeping the Phase 3 PRs focused on protocol correctness.

## References

* Issue #257 — Extract shared connection-lifecycle helper for
  serial-based ASCOM services (umbrella, this work).
* Issue #250 — `qhy-focuser` `set_connected` race (closed by PR
  #256; matches the bug class this plan generalizes).
* Issue #251 — `ppba-driver` `set_connected` race (closed
  structurally by Phase B).
* Issue #258 — `qhy-focuser` refcount leak on partial-connect
  failure (closed structurally by Phase C; PR #260 lands first as
  the interim inline fix).
* PR #241 — `pa-falcon-rotator` Phase 2 (introduces the canonical
  lock-held fix shape in commit `8cd6e16`; Phase D depends on this
  merging).
* PR #255 — `ppba-driver` inline fix for #251 (superseded by
  Phase B if Phase A lands in time).
* PR #256 — `qhy-focuser` `set_connected` race fix for #250
  (canonical lock-held shape, merged).
* PR #260 — `qhy-focuser` rollback fix for #258
  (interim; Phase C deletes the rollback code).
* PR #265 — this plan; review thread settles five of the original
  open questions (see "Resolved" list above).
* `docs/services/qhy-focuser.md`,
  `docs/services/ppba-driver.md`,
  `docs/services/falcon-rotator.md`,
  `docs/services/star-adventurer-gti.md` —
  per-service design docs. Each gets a small update in its
  migration phase noting the move to `rusty-photon-shared-transport`.
