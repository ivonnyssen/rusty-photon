//! Phase 0b: reconnect supervisor and connection-cell swap.
//!
//! These tests verify the supervisor's transport-recovery contract:
//!
//! - `reconnect_now()` opens a fresh transport and runs the handshake
//!   against it, then atomically swaps it into the cell.
//! - Live `Session<C>` references handed out by `acquire()` automatically
//!   route through the new transport on their next `request()` call —
//!   no client-visible Session recreation is needed (the
//!   live-session-survival contract from the transport-lifecycle plan).
//! - `is_reconnecting()` flips false again once recovery succeeds; the
//!   supervisor task is wired so external observers can poll the state.
//! - `shutdown()` cancels the supervisor cleanly.
//!
//! Notify-driven reconnect (Connection::request firing the signal on a
//! real transport error) lands the moment a real service's mock factory
//! exercises a mid-stream drop in Phases 1-5; this file pins the
//! supervisor / cell / `reconnect_now` mechanics in isolation.
//!
//! What deliberately is **not** here yet (Phase 0b follow-up):
//!
//! - On-acquire eager reconnect (an `acquire()` mid-reconnect that
//!   triggers a synchronous attempt with `reconnect_acquire_timeout`).
//! - Codec-error filtering verification (needs a codec that can be
//!   poked into emitting `SessionError::Codec`; `EchoCodec` always
//!   round-trips successfully).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]

mod common;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use common::{build_with_factory_and_hooks, CountingHooks, FactoryConfig, ProgrammableFactory};
use rusty_photon_shared_transport::TransportFactory;

#[tokio::test]
async fn reconnect_now_opens_fresh_transport_and_runs_handshake() {
    let cfg = FactoryConfig::default();
    let factory: Arc<dyn TransportFactory> = Arc::new(ProgrammableFactory::new(cfg.clone()));
    let counting = CountingHooks::default();
    let st = build_with_factory_and_hooks(factory, counting.hooks());

    st.start().await.unwrap();
    assert_eq!(cfg.opens(), 1);
    assert_eq!(counting.handshake_calls.load(Ordering::SeqCst), 1);

    st.reconnect_now().await.unwrap();
    assert_eq!(
        cfg.opens(),
        2,
        "reconnect_now must open a fresh transport via the factory"
    );
    assert_eq!(
        counting.handshake_calls.load(Ordering::SeqCst),
        2,
        "reconnect_now must run the handshake against the new transport"
    );
    assert!(
        !st.is_reconnecting(),
        "reconnecting flag must clear on successful reconnect"
    );
    assert!(
        st.is_available(),
        "available flag must be true after successful reconnect"
    );
}

#[tokio::test]
async fn live_session_survives_reconnect_via_cell_swap() {
    // The headline Phase 0b contract: a Session acquired before a
    // reconnect transparently picks up the new transport on its next
    // request, because the supervisor swaps the inner Arc<Connection>
    // inside the cell that both SharedTransport and Sessions share.
    let cfg = FactoryConfig::default();
    let factory: Arc<dyn TransportFactory> = Arc::new(ProgrammableFactory::new(cfg.clone()));
    let counting = CountingHooks::default();
    let st = build_with_factory_and_hooks(factory, counting.hooks());

    st.start().await.unwrap();
    let session = st.acquire().await.unwrap();

    let r1 = session.request(b"ping".to_vec()).await.unwrap();
    assert_eq!(r1, b"ping", "request must work before reconnect");

    st.reconnect_now().await.unwrap();
    assert_eq!(cfg.opens(), 2);

    // Critical assertion: the same Session reference, no recreation,
    // routes through the new transport.
    let r2 = session.request(b"pong".to_vec()).await.unwrap();
    assert_eq!(
        r2, b"pong",
        "live session must survive a reconnect via cell swap"
    );

    session.close().await.unwrap();
}

#[tokio::test]
async fn reconnect_now_does_not_change_refcount() {
    // Reconnect is a transport-level concern; the external client
    // refcount must not move. A session acquired before and a session
    // acquired after see the same fast path.
    let cfg = FactoryConfig::default();
    let factory: Arc<dyn TransportFactory> = Arc::new(ProgrammableFactory::new(cfg.clone()));
    let counting = CountingHooks::default();
    let st = build_with_factory_and_hooks(factory, counting.hooks());

    st.start().await.unwrap();
    let s1 = st.acquire().await.unwrap();
    assert_eq!(cfg.opens(), 1);

    st.reconnect_now().await.unwrap();
    assert_eq!(cfg.opens(), 2);
    assert_eq!(
        counting.handshake_calls.load(Ordering::SeqCst),
        2,
        "exactly one handshake per open"
    );

    // Acquire another after the reconnect — still the fast path
    // (slot's cell holds the new connection).
    let s2 = st.acquire().await.unwrap();
    assert_eq!(
        cfg.opens(),
        2,
        "post-reconnect acquire reuses the new connection — no third open"
    );

    s1.close().await.unwrap();
    s2.close().await.unwrap();
}

#[tokio::test]
async fn reconnect_now_failure_leaves_supervisor_in_reconnecting() {
    // factory.open() fails: reconnect_now returns Err and the
    // supervisor stays in Reconnecting until a successful retry.
    let cfg = FactoryConfig::default();
    let factory: Arc<dyn TransportFactory> = Arc::new(ProgrammableFactory::new(cfg.clone()));
    let counting = CountingHooks::default();
    let st = build_with_factory_and_hooks(factory, counting.hooks());

    st.start().await.unwrap();

    // Flip the factory to fail mode; reconnect_now must surface the
    // failure and the state must reflect ongoing recovery.
    cfg.set_fail(true);
    let err = st.reconnect_now().await.unwrap_err();
    let display = format!("{err}");
    assert!(
        display.contains("EOF") || display.contains("eof") || display.contains("transport"),
        "expected a transport-loss-shaped error from a failing factory.open, got: {display}"
    );
    assert!(
        st.is_reconnecting(),
        "transport must stay in Reconnecting until a successful retry"
    );
    assert!(
        !st.is_available(),
        "available must be false during Reconnecting"
    );

    // Recovery: factory succeeds again, kick another attempt manually.
    cfg.set_fail(false);
    st.reconnect_now().await.unwrap();
    assert!(!st.is_reconnecting());
    assert!(st.is_available());
}

#[tokio::test]
async fn session_request_short_circuits_to_reconnecting_during_failure() {
    // While the supervisor is in Reconnecting (a failed reconnect
    // attempt leaves us there), Session::request must short-circuit
    // with TransportError::Reconnecting rather than waiting on the
    // dying transport's command lock.
    let cfg = FactoryConfig::default();
    let factory: Arc<dyn TransportFactory> = Arc::new(ProgrammableFactory::new(cfg.clone()));
    let counting = CountingHooks::default();
    let st = build_with_factory_and_hooks(factory, counting.hooks());

    st.start().await.unwrap();
    let session = st.acquire().await.unwrap();

    // Induce a Reconnecting state via a failed reconnect_now.
    cfg.set_fail(true);
    let _ = st.reconnect_now().await; // expected to fail
    assert!(st.is_reconnecting());

    let err = session.request(b"x".to_vec()).await.unwrap_err();
    let display = format!("{err}");
    assert!(
        display.contains("reconnecting"),
        "expected TransportError::Reconnecting, got: {display}"
    );

    // Cleanup: recover, then close.
    cfg.set_fail(false);
    st.reconnect_now().await.unwrap();
    session.close().await.unwrap();
}

#[tokio::test]
async fn shutdown_cancels_supervisor_cleanly() {
    // After shutdown, no further reconnect attempts should happen —
    // the supervisor is cancelled. A subsequent acquire returns the
    // shut-down error.
    let cfg = FactoryConfig::default();
    let factory: Arc<dyn TransportFactory> = Arc::new(ProgrammableFactory::new(cfg.clone()));
    let counting = CountingHooks::default();
    let st = build_with_factory_and_hooks(factory, counting.hooks());

    st.start().await.unwrap();
    st.shutdown().await.unwrap();

    // The supervisor would normally react to factory.open() succeeding
    // again, but it's been cancelled — so additional opens shouldn't
    // happen on a periodic-timer tick.
    common::yield_briefly().await;
    assert_eq!(
        cfg.opens(),
        1,
        "no more opens after shutdown — supervisor must be cancelled"
    );

    // is_reconnecting cleared by shutdown.
    assert!(!st.is_reconnecting());
}
