//! Phase 0a: `SharedTransport::start` / `shutdown` and the
//! `LazyAcquire`-vs-`ServiceLifetime` mode split.
//!
//! These tests verify the two new lifecycle modes coexist correctly:
//!
//! - In `LazyAcquire` (default, no `start()` called), the port opens on
//!   the 0→1 `acquire()` and closes on the 1→0 `Session::close()`.
//!   `Hooks::on_last_disconnect` runs on 1→0; `Hooks::shutdown` is
//!   never invoked.
//! - In `ServiceLifetime` (after `start()`), the port opens at `start()`
//!   and stays open until `shutdown()`. `Hooks::on_last_disconnect` runs
//!   on every 1→0 and the port stays open. `Hooks::shutdown` runs once
//!   from `shutdown()`.
//!
//! Reconnect-supervisor tests land in Phase 0b alongside that machinery.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]

mod common;

use std::sync::atomic::Ordering;

use common::{
    build_with_factory_and_hooks, CountingHooks, FactoryConfig, ProgrammableFactory, WhileOpenHooks,
};
use rusty_photon_shared_transport::TransportFactory;

// ---------------------------------------------------------------------------
// start()
// ---------------------------------------------------------------------------

#[tokio::test]
async fn start_opens_transport_and_runs_handshake() {
    let cfg = FactoryConfig::default();
    let factory: std::sync::Arc<dyn TransportFactory> =
        std::sync::Arc::new(ProgrammableFactory::new(cfg.clone()));
    let counting = CountingHooks::default();
    let st = build_with_factory_and_hooks(factory, counting.hooks());

    st.start().await.unwrap();

    assert_eq!(
        cfg.opens(),
        1,
        "start() must open the transport exactly once"
    );
    assert_eq!(
        counting.handshake_calls.load(Ordering::SeqCst),
        1,
        "start() must run the handshake exactly once"
    );
    assert!(st.is_available());
}

#[tokio::test]
async fn start_is_idempotent_when_already_started() {
    let cfg = FactoryConfig::default();
    let factory: std::sync::Arc<dyn TransportFactory> =
        std::sync::Arc::new(ProgrammableFactory::new(cfg.clone()));
    let counting = CountingHooks::default();
    let st = build_with_factory_and_hooks(factory, counting.hooks());

    st.start().await.unwrap();
    st.start().await.unwrap();
    st.start().await.unwrap();

    assert_eq!(
        cfg.opens(),
        1,
        "repeated start() must not re-open the transport"
    );
    assert_eq!(
        counting.handshake_calls.load(Ordering::SeqCst),
        1,
        "repeated start() must not re-run handshake"
    );
}

#[tokio::test]
async fn start_promotes_an_already_lazy_opened_transport() {
    // A client called acquire() first (LazyAcquire 0→1 open); then the
    // service binary called start(). The flag flips to ServiceLifetime
    // without re-opening or re-handshaking.
    let cfg = FactoryConfig::default();
    let factory: std::sync::Arc<dyn TransportFactory> =
        std::sync::Arc::new(ProgrammableFactory::new(cfg.clone()));
    let counting = CountingHooks::default();
    let st = build_with_factory_and_hooks(factory, counting.hooks());

    let session = st.acquire().await.unwrap();
    assert_eq!(cfg.opens(), 1);
    assert_eq!(counting.handshake_calls.load(Ordering::SeqCst), 1);

    st.start().await.unwrap();
    assert_eq!(
        cfg.opens(),
        1,
        "start() on an already-open transport must not re-open"
    );
    assert_eq!(counting.handshake_calls.load(Ordering::SeqCst), 1);

    // Now in ServiceLifetime mode: closing the session must not close
    // the port.
    session.close().await.unwrap();
    assert!(
        st.is_available(),
        "transport must stay open across 1→0 after start() promoted it"
    );
}

// ---------------------------------------------------------------------------
// acquire() in ServiceLifetime mode
// ---------------------------------------------------------------------------

#[tokio::test]
async fn acquire_after_start_is_fast_path_no_reopen() {
    let cfg = FactoryConfig::default();
    let factory: std::sync::Arc<dyn TransportFactory> =
        std::sync::Arc::new(ProgrammableFactory::new(cfg.clone()));
    let counting = CountingHooks::default();
    let st = build_with_factory_and_hooks(factory, counting.hooks());

    st.start().await.unwrap();
    let _s1 = st.acquire().await.unwrap();
    let _s2 = st.acquire().await.unwrap();
    let _s3 = st.acquire().await.unwrap();

    assert_eq!(
        cfg.opens(),
        1,
        "acquire() in ServiceLifetime mode must reuse the slot connection"
    );
    assert_eq!(
        counting.handshake_calls.load(Ordering::SeqCst),
        1,
        "handshake belongs to start(); per-client acquire() must not re-run it"
    );
}

// ---------------------------------------------------------------------------
// Session::close() in ServiceLifetime mode
// ---------------------------------------------------------------------------

#[tokio::test]
async fn close_in_service_lifetime_runs_on_last_disconnect_and_keeps_port_open() {
    let cfg = FactoryConfig::default();
    let factory: std::sync::Arc<dyn TransportFactory> =
        std::sync::Arc::new(ProgrammableFactory::new(cfg.clone()));
    let counting = CountingHooks::default();
    let st = build_with_factory_and_hooks(factory, counting.hooks());

    st.start().await.unwrap();
    let session = st.acquire().await.unwrap();
    session.close().await.unwrap();

    assert_eq!(
        counting.teardown_calls.load(Ordering::SeqCst),
        1,
        "on_last_disconnect must fire on the 1→0 transition"
    );
    assert_eq!(
        counting.shutdown_calls.load(Ordering::SeqCst),
        0,
        "shutdown must NOT fire on a client disconnect — only from SharedTransport::shutdown()"
    );
    assert!(
        st.is_available(),
        "port must stay open across 1→0 in ServiceLifetime mode"
    );
    assert_eq!(
        cfg.dropped_count().await,
        0,
        "FrameTransport must not have dropped"
    );
}

#[tokio::test]
async fn close_in_service_lifetime_fires_on_last_disconnect_each_cycle() {
    // Three full connect/disconnect cycles → on_last_disconnect fires
    // three times; the port stays open across all of them.
    let cfg = FactoryConfig::default();
    let factory: std::sync::Arc<dyn TransportFactory> =
        std::sync::Arc::new(ProgrammableFactory::new(cfg.clone()));
    let counting = CountingHooks::default();
    let st = build_with_factory_and_hooks(factory, counting.hooks());

    st.start().await.unwrap();
    for _ in 0..3 {
        let s = st.acquire().await.unwrap();
        s.close().await.unwrap();
    }

    assert_eq!(cfg.opens(), 1);
    assert_eq!(counting.handshake_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        counting.teardown_calls.load(Ordering::SeqCst),
        3,
        "on_last_disconnect must fire on every 1→0 transition"
    );
    assert_eq!(counting.shutdown_calls.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------
// shutdown()
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shutdown_runs_shutdown_hook_and_closes_port() {
    let cfg = FactoryConfig::default();
    let factory: std::sync::Arc<dyn TransportFactory> =
        std::sync::Arc::new(ProgrammableFactory::new(cfg.clone()));
    let counting = CountingHooks::default();
    let st = build_with_factory_and_hooks(factory, counting.hooks());

    st.start().await.unwrap();
    st.shutdown().await.unwrap();

    assert_eq!(
        counting.shutdown_calls.load(Ordering::SeqCst),
        1,
        "shutdown hook must fire exactly once"
    );
    assert_eq!(
        counting.teardown_calls.load(Ordering::SeqCst),
        0,
        "on_last_disconnect must NOT fire from shutdown() — it's only for client refcount 1→0"
    );
    assert!(!st.is_available());
    assert_eq!(
        cfg.dropped_count().await,
        1,
        "FrameTransport must drop when shutdown closes the port"
    );
}

#[tokio::test]
async fn shutdown_is_noop_in_lazy_acquire_mode() {
    let cfg = FactoryConfig::default();
    let factory: std::sync::Arc<dyn TransportFactory> =
        std::sync::Arc::new(ProgrammableFactory::new(cfg.clone()));
    let counting = CountingHooks::default();
    let st = build_with_factory_and_hooks(factory, counting.hooks());

    // No start() called — LazyAcquire mode.
    st.shutdown().await.unwrap();
    assert_eq!(
        counting.shutdown_calls.load(Ordering::SeqCst),
        0,
        "shutdown is a no-op when service_lifetime was never set"
    );

    // LazyAcquire still works normally afterwards.
    let s = st.acquire().await.unwrap();
    s.close().await.unwrap();
    assert_eq!(cfg.opens(), 1);
    assert_eq!(
        counting.teardown_calls.load(Ordering::SeqCst),
        1,
        "LazyAcquire 1→0 still runs on_last_disconnect after a no-op shutdown call"
    );
}

#[tokio::test]
async fn acquire_after_shutdown_returns_shut_down_error() {
    let cfg = FactoryConfig::default();
    let factory: std::sync::Arc<dyn TransportFactory> =
        std::sync::Arc::new(ProgrammableFactory::new(cfg.clone()));
    let counting = CountingHooks::default();
    let st = build_with_factory_and_hooks(factory, counting.hooks());

    st.start().await.unwrap();
    st.shutdown().await.unwrap();

    let err = st.acquire().await.unwrap_err();
    let display = format!("{err}");
    assert!(
        display.contains("shut down"),
        "expected shut-down error, got: {display}"
    );
}

#[tokio::test]
async fn start_after_shutdown_reopens_cleanly() {
    let cfg = FactoryConfig::default();
    let factory: std::sync::Arc<dyn TransportFactory> =
        std::sync::Arc::new(ProgrammableFactory::new(cfg.clone()));
    let counting = CountingHooks::default();
    let st = build_with_factory_and_hooks(factory, counting.hooks());

    st.start().await.unwrap();
    st.shutdown().await.unwrap();

    // Second cycle: full cold-start again.
    st.start().await.unwrap();
    let s = st.acquire().await.unwrap();
    s.close().await.unwrap();
    st.shutdown().await.unwrap();

    assert_eq!(
        cfg.opens(),
        2,
        "each start() after a shutdown() must re-open the port"
    );
    assert_eq!(
        counting.handshake_calls.load(Ordering::SeqCst),
        2,
        "each start() must re-run handshake"
    );
    assert_eq!(counting.shutdown_calls.load(Ordering::SeqCst), 2);
}

// ---------------------------------------------------------------------------
// LazyAcquire mode preservation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lazy_acquire_close_still_closes_port_and_does_not_call_shutdown() {
    // No start() / shutdown() calls at all — verifies the pre-Phase-0a
    // behavior is preserved for unmigrated services.
    let cfg = FactoryConfig::default();
    let factory: std::sync::Arc<dyn TransportFactory> =
        std::sync::Arc::new(ProgrammableFactory::new(cfg.clone()));
    let counting = CountingHooks::default();
    let st = build_with_factory_and_hooks(factory, counting.hooks());

    let s = st.acquire().await.unwrap();
    s.close().await.unwrap();

    assert_eq!(
        counting.teardown_calls.load(Ordering::SeqCst),
        1,
        "LazyAcquire 1→0 runs on_last_disconnect"
    );
    assert_eq!(
        counting.shutdown_calls.load(Ordering::SeqCst),
        0,
        "shutdown hook is never invoked in LazyAcquire mode"
    );
    assert!(!st.is_available());
    assert_eq!(
        cfg.dropped_count().await,
        1,
        "LazyAcquire 1→0 still drops the FrameTransport"
    );
}

// ---------------------------------------------------------------------------
// while_open task lifecycle under ServiceLifetime
// ---------------------------------------------------------------------------

#[tokio::test]
async fn while_open_task_survives_client_disconnect_in_service_lifetime() {
    // The while_open task is tied to transport-open, not client refcount.
    // In ServiceLifetime mode it keeps running through a full
    // connect/disconnect cycle.
    let cfg = FactoryConfig::default();
    let factory: std::sync::Arc<dyn TransportFactory> =
        std::sync::Arc::new(ProgrammableFactory::new(cfg.clone()));
    let wo = WhileOpenHooks::default();
    let st = build_with_factory_and_hooks(factory, wo.cooperative_hooks());

    st.start().await.unwrap();
    common::yield_briefly().await;
    assert!(
        wo.started.load(Ordering::SeqCst),
        "while_open should start as soon as start() returns"
    );

    let s = st.acquire().await.unwrap();
    s.close().await.unwrap();
    common::yield_briefly().await;
    assert!(
        !wo.exited.load(Ordering::SeqCst),
        "while_open must NOT exit on client 1→0 in ServiceLifetime mode"
    );

    st.shutdown().await.unwrap();
    common::yield_briefly().await;
    assert!(
        wo.exited.load(Ordering::SeqCst),
        "while_open exits on shutdown()'s cancellation"
    );
}
