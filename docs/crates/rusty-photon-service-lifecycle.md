# `rusty-photon-service-lifecycle` Crate Design

Unified service lifecycle for every long-running Rusty Photon binary:
owns the tokio runtime, installs OS signal handlers (or dispatches to
the Windows Service Control Manager), and exposes a single cooperative
shutdown handle backed by [`tokio_util::sync::CancellationToken`].

This is a workspace library, not a service. Every service binary in
`services/` consumes it directly. For the migration plan that moves
existing services onto it, see
[`docs/plans/archive/service-lifecycle-unification.md`](../plans/archive/service-lifecycle-unification.md);
this doc covers the crate's own design.

## Scope

- Build the tokio runtime; invoke a user closure on it with a
  [`Shutdown`] handle.
- Install OS signal handlers (Ctrl+C + SIGTERM on Unix; Ctrl+C on
  Windows console) without panicking on install failure.
- Optional: install a SIGHUP-driven reload notifier
  ([`ReloadSignal`]).
- Optional (cargo feature `scm`, Windows-only): dispatch to the
  Windows Service Control Manager and translate
  `ServiceControl::Stop` / `ParamChange` into the same `Shutdown` +
  `ReloadSignal` types the console path produces.
- Cooperative-shutdown propagation: the handle is a
  `CancellationToken`-backed clone, so spawned subtasks observe the
  same cancellation as the main loop.
- Install the shared tracing subscriber ([`init_tracing`]): logs go to
  stderr (stdout is reserved for the `bound_addr=` handshake the BDD
  harness parses), filtered by `RUST_LOG` when set, otherwise at a
  caller-supplied fallback level. Idempotent. Every service binary calls
  this once at startup so logging is configured identically everywhere.

Out of scope:

- Owning CLI argument parsing. Services keep `clap`; the runner takes
  a service name and a closure. (Services still own their `--log-level`
  flag and pass its value to [`init_tracing`] as the fallback level.)
- Defining what graceful shutdown means for any particular server.
  Services compose `shutdown.cancelled()` into their own server stop
  (a `tokio::select!` race, `axum::with_graceful_shutdown(...)`, or
  a passed-in `CancellationToken`).
- Replacing `#[tokio::main]` as a macro. The runner is a plain
  builder; the user closure is plain `async`.
- Windows Service installation artifacts (msi, winget, `sc create`
  scripts). The crate produces a *binary capable of running under
  SCM*; packaging is a separate concern.

## Public API

Three types and one free function ([`init_tracing`]).
[`Shutdown`] is constructed only by the runner.
[`ReloadSignal`] has a public constructor so integration tests can
drive a service's run loop with synthetic reload events, and so
non-signal-driven reload sources (e.g. a file-watcher) can share the
same primitive — but the canonical producer remains the runner.

```text
ServiceRunner::new(name)                       -> ServiceRunner
ServiceRunner::with_reload(self)               -> ServiceRunner
ServiceRunner::scm_mode(self, enable)          -> ServiceRunner   // no-op unless cfg(all(windows, feature = "scm"))
ServiceRunner::run(self, |Shutdown| async)              -> Result<(), Box<dyn Error>>
ServiceRunner::run_with_reload(self, |Shutdown, ReloadSignal| async) -> Result<(), Box<dyn Error>>

Shutdown::token()        -> CancellationToken
Shutdown::cancelled()    -> impl Future<Output = ()> + Send + 'static
Shutdown::is_cancelled() -> bool

ReloadSignal::new()      -> ReloadSignal      // for tests / alt sources
ReloadSignal::notify(&self)                   // wake one waiter
ReloadSignal::recv(&self) -> impl Future<Output = ()> + '_

init_tracing(default_level: tracing::Level)   // stderr + RUST_LOG/EnvFilter subscriber; idempotent
```

The closure passed to `run` / `run_with_reload` is `FnOnce(...) -> Fut`
where `Fut: Future<Output = Result<(), Box<dyn Error>>> + 'static`
and the closure itself is `Send + 'static`. Bounds explained:

* `F: Send + 'static` — the SCM dispatch path stashes the closure in
  a `OnceLock` and re-enters it from the `extern "system" fn` service
  entry point, which requires both bounds. The console path inherits
  them for API uniformity.
* `Fut: 'static` — the SCM path type-erases the future into
  `Pin<Box<dyn Future<Output = ServiceResult>>>`, which is implicitly
  `+ 'static`. Most async fn bodies satisfy this naturally because
  they own their captures via `move` semantics; the only futures that
  fail are those that borrow non-`'static` data.
* `Fut: Send` is **not** required. `Runtime::block_on` polls the
  future on the calling thread, so error types and intermediate state
  inside the closure body stay non-`Send`-friendly without extra
  bounds.

## Behavior

### Signal install (no-panic)

The runner spawns a single signal-watcher task that races the OS
signals and cancels the underlying [`CancellationToken`] on first
fire. Install failures are logged via `tracing::warn!` and replaced
with a never-resolving future, so a misconfigured environment that
cannot install (e.g., already-stolen signal) degrades to "the other
signal source still works" rather than "the service panics during
startup":

```rust
async fn watch_signals(token: CancellationToken) {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!("failed to wait for Ctrl+C: {e}");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => { sig.recv().await; }
            Err(e) => {
                tracing::warn!("failed to install SIGTERM handler: {e}");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::debug!("received Ctrl+C, shutting down"),
        () = terminate => tracing::debug!("received SIGTERM, shutting down"),
    }
    token.cancel();
}
```

This matches the no-panic pattern established workspace-wide by
PR #289. One implementation; one log-message style; one place to
fix bugs.

### Reload (opt-in via `with_reload`)

When [`ServiceRunner::with_reload`] is set, the runner additionally
spawns a SIGHUP-watcher (Unix) — or, in SCM mode, wires the
`ServiceControl::ParamChange` event — to a [`ReloadSignal`] that the
user closure receives via `run_with_reload`. Each event wakes one
waiter via `Notify::notify_one`. The signal is *notification only*:
re-reading config and rebuilding state is the caller's job.

On Windows console mode there is no SIGHUP equivalent;
`ReloadSignal::recv` returns a never-resolving future.

### Shutdown propagation

The runner constructs one [`CancellationToken`] per `run` invocation.
The signal-watcher task cancels it on first fire; SCM mode cancels it
when the control handler receives `ServiceControl::Stop`. The user
closure receives a [`Shutdown`] wrapping this token; `.token()` hands
out clones for spawned subtasks. All clones observe the same
cancellation — propagation is `O(1)`, not `O(N)` per-subtask wakes.

This is the existing `sentinel` pattern, generalized: sentinel
already takes a `CancellationToken` through its engine and cancels it
from a hand-rolled signal task. After the migration, that task goes
away and sentinel's engine receives `shutdown.token()` directly.

**Do not install a second signal handler downstream.** If a server
inside your service already takes a shutdown future (e.g. axum's
`with_graceful_shutdown`, or `BoundServer::start` after the #294
migration), pass `shutdown.cancelled()` *into* it rather than racing
it externally with `tokio::select!`. Racing two independent signal
sources lets one drop the other mid-flight — that's the bug
[#287](https://github.com/ivonnyssen/rusty-photon/issues/287) and
the underlying motivation for funneling everything through a single
`Shutdown` handle.

### SCM mode (`#[cfg(all(windows, feature = "scm"))]`)

When `scm_mode(true)` is set on Windows with the `scm` feature
enabled, `run` / `run_with_reload` take a different path:

1. Stash the runner config + closure in a `OnceLock` (the SCM
   dispatch entry point is a no-arg `extern "system" fn`, so the
   closure must be reachable from a static).
2. Call `windows_service::service_dispatcher::start(name, ffi_main)`.
3. Inside `ffi_main`, register a control handler translating:
   - `ServiceControl::Stop` → `token.cancel()`
   - `ServiceControl::ParamChange` (only when `with_reload`) →
     `reload.notify()`
   - `ServiceControl::Interrogate` → `NoError`
   - everything else → `NotImplemented`
4. Report `ServiceState::Running` to SCM.
5. Build the tokio runtime, invoke the user closure with the same
   [`Shutdown`] (and optional [`ReloadSignal`]) types as the console
   path. **The user closure does not know whether it is running under
   SCM or under console-mode signals** — that is the whole point of
   the abstraction.
6. On the closure's return, report `ServiceState::Stopped`. The
   `exit_code` field reflects the closure's outcome: `Win32(0)` on
   `Ok(())`, `ServiceSpecific(1)` on `Err`. Reporting `0` on every
   stop would let crashes look like clean shutdowns in `services.msc`
   and any supervisor reading SCM's stop record.

When `scm_mode(false)` (or the `scm` feature is off, or the target
is not Windows), the runner falls through to the OS-signal path
unchanged.

### Why the runner owns the runtime

`ServiceRunner::run` is a synchronous `fn`, not an `async fn`. It
builds a `tokio::runtime::Runtime` internally and `block_on`s the
user closure on it. This choice is forced by the SCM dispatch path:
`service_dispatcher::start` is synchronous and must be called from a
context that owns the entry point — there is no tokio runtime when
`ffi_main` is invoked, so SCM and console paths cannot share a single
entry point unless the runner owns runtime construction. The same
shape works for console mode without any compromise.

Practically, this means services move from `#[tokio::main] async fn
main() -> Result<...>` to `fn main() -> Result<...> { runner.run(|s|
async move { ... }) }`. The visible diff is small, and there's only
one place in main where `async` enters.

## Usage

### ASCOM Alpaca driver, console only

The common case (10 of 12 services after migration). The server
consumes `shutdown.cancelled()` directly — there is no outer
`tokio::select!`. This avoids the double-installation race described
in [issue #287](https://github.com/ivonnyssen/rusty-photon/issues/287)
by making axum's `with_graceful_shutdown` (inside `BoundServer::start`)
fire on the same source as the OS-signal watcher.

```rust
use rusty_photon_service_lifecycle::ServiceRunner;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    init_tracing(args.log_level);

    ServiceRunner::new("dsd-fp2").run(|shutdown| async move {
        let bound = ServerBuilder::new()
            .with_config(args.config)
            .build()
            .await?;

        // `BoundServer::start` accepts the shutdown future and threads
        // it into axum's `with_graceful_shutdown`. When the signal fires,
        // axum drains in-flight requests before returning.
        bound.start(shutdown.cancelled()).await?;
        Ok(())
    })
}
```

For services where the server still does its own
`tokio::select!` internally (e.g. the engine in `sentinel`),
pass `shutdown.token()` instead — see the next example.

### Axum service (graceful shutdown drains in-flight requests)

```rust
ServiceRunner::new("calibrator-flats").run(|shutdown| async move {
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown.cancelled())
        .await?;
    Ok(())
})
```

`with_graceful_shutdown` is axum's idiomatic stop signal; the runner's
`Shutdown` plugs in directly with no `tokio::select!` glue.

### Service with spawned worker tasks (sentinel-style)

When you spawn workers that need to know about shutdown, hand them
clones of the token:

```rust
ServiceRunner::new("sentinel").run(|shutdown| async move {
    let token = shutdown.token();

    let dashboard = tokio::spawn(run_dashboard(token.clone()));
    let engine = tokio::spawn(run_engine(token.clone()));

    let _ = tokio::join!(dashboard, engine);
    Ok(())
})
```

The token is cloned at no cost; cancellation propagates to every
clone, so each worker can `tokio::select! { _ = token.cancelled() => ... }`
in its own loop.

### Service with reload (filemonitor)

Enable reload, optionally enable SCM mode (gated on a CLI flag the
SCM control manager passes via `binPath`), and use
`run_with_reload`:

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    init_tracing(args.log_level);

    ServiceRunner::new("filemonitor")
        .with_reload()
        .scm_mode(args.service)       // requires `features = ["scm"]`
        .run_with_reload(|shutdown, reload| async move {
            run_server_loop(&args.config, shutdown.token(), reload).await
        })
}
```

`run_server_loop` then races its config-rebuild loop against
`shutdown.token().cancelled()` and `reload.recv()` in a single
`tokio::select!`. The same closure works under SCM (driven by
`Stop` / `ParamChange`) and console (driven by `SIGTERM` /
`SIGHUP`); the service body sees identical types.

### Enabling SCM in a new service

Two changes:

```toml
# services/<name>/Cargo.toml
rusty-photon-service-lifecycle = { workspace = true, features = ["scm"] }
```

```rust
// services/<name>/src/main.rs
#[derive(Parser)]
struct Args {
    // ... existing args ...

    /// Run as a Windows service (used by the service control manager)
    #[cfg(windows)]
    #[arg(long, hide = true)]
    service: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    ServiceRunner::new("my-service")
        .scm_mode(args.service)   // no-op on non-Windows targets
        .run(|shutdown| async move {
            // ...
        })
}
```

That's the whole adoption cost. The SCM control-handler glue lives
in the crate; the service binary just opts in.

## Module Layout

```
crates/rusty-photon-service-lifecycle/src/
├── lib.rs       # crate root: //! docs, module decls, re-exports
├── logging.rs   # init_tracing — shared stderr + EnvFilter subscriber
├── shutdown.rs  # Shutdown — thin wrapper over CancellationToken
├── reload.rs    # ReloadSignal — thin wrapper over Arc<Notify>
└── runner.rs    # ServiceRunner — builder + run/run_with_reload
```

There is intentionally no `scm.rs` module today; the SCM dispatch
code lives inline in `runner.rs` behind `#[cfg(feature = "scm")]`.
If the SCM glue grows past ~100 LOC it gets its own module.

## Testing

- **Unit tests** live next to the code under
  `#[cfg(test)] mod tests`. The cross-cutting primitives ([`Shutdown`]
  and [`ReloadSignal`]) are trivial enough that their tests focus on
  contract — `cancelled()` resolves when any clone cancels, `recv()`
  resolves on `notify()`, `is_cancelled()` flips after `cancel()`.
- **Signal-install integration tests** drive the full
  `ServiceRunner::run` / `run_with_reload` path on Unix: spawn a
  task that `libc::raise`s `SIGTERM` (or `SIGHUP` followed by
  `SIGTERM`) after a brief delay, then assert that
  `shutdown.cancelled()` resolves (and that `reload.recv()` fires
  at least once for the SIGHUP variant). Tokio's signal handlers
  are reference-counted, but raising signals to self affects the
  whole process, so the test module serializes runs via a
  `std::sync::Mutex<()>`. `libc` is a `cfg(unix)` dev-dependency
  to avoid pulling in `nix` solely for these tests.
- **Runner contract** — `ServiceRunner::run` invokes the closure
  exactly once, propagates its `Result`, and returns `Ok(())` when
  the closure returns `Ok(())`. `run_with_reload` without a prior
  `.with_reload()` returns a descriptive error rather than running
  the closure. The runner builds and tears down a fresh runtime
  per call.
- **SCM mode** — `scm_mode(false)` is a no-op and falls through to
  the OS-signal path on Windows. The full SCM dispatch is *not*
  exercised in CI: `windows_service::service_dispatcher::start` only
  succeeds when invoked by the actual SCM (it returns
  `ERROR_FAILED_SERVICE_CONTROLLER_CONNECT` otherwise). SCM mode is
  manually validated via filemonitor's existing Windows install/run
  procedure on each release.
- **No `coverage(off)` exclusions.** Per workspace policy, production
  code is covered or it isn't shipped. The dead-code arms in the
  no-panic signal-install path (`std::future::pending` fallbacks)
  are deliberately impossible to provoke in a unit test — they fire
  only when the OS refuses to install a handler — and we accept the
  coverage gap rather than write a `#[cfg(test)]` injection seam
  for a single branch.

## Dependencies

| Crate | Purpose |
|---|---|
| `tokio` | runtime, signal handling, `sync::Notify` |
| `tokio-util` | `CancellationToken` |
| `tracing` | warn/debug logging on signal events |
| `windows-service` (opt, feature `scm`) | Windows Service Control Manager dispatch + control handler |

Workspace-pinned versions live in the workspace `Cargo.toml`. The
`scm` feature is opt-in per consumer; non-adopters get zero
`windows-service` content in their dep tree.

`windows-service` is `windows-service = "0.8"` — the Mullvad-
maintained Rust wrapper around the Windows Service Control API.
On non-Windows targets the crate's library is essentially empty, so
even if a consumer enables `scm` and is built on Linux/macOS the
runtime cost is zero.
