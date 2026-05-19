//! Refcounted multi-client lifecycle scaffolding for duplex transports.
//!
//! This crate factors out the connect-handshake-share-teardown pattern that
//! every ASCOM service in this workspace had grown independently
//! (`qhy-focuser`, `ppba-driver`, `pa-falcon-rotator`,
//! `star-adventurer-gti`). See `docs/plans/shared-transport-extraction.md`
//! for the design rationale and the three bug classes the abstraction
//! dissolves structurally.
//!
//! # The shape
//!
//! ```text
//! Service Manager  ───►  Arc<SharedTransport<C>>
//!                              │  acquire()
//!                              ▼
//!                          Session<C>  ──► request(cmd) → C::Response
//!                              │  close().await   ◄── primary teardown
//!                              │  Drop            ◄── detached fallback
//! ```
//!
//! [`SharedTransport`] holds the refcount, the slot, and the open-state
//! lock. [`Session`] is the handle a service hands to its ASCOM device
//! types; one device = one session. The first `acquire` runs the
//! handshake; the last drop runs teardown. A `while_open` task (e.g. a
//! poll loop) can be configured via [`Hooks`] — its lifetime tracks the
//! transport's, not any individual session's.
//!
//! Codec authors implement [`Codec`] to translate between protocol
//! commands and on-wire frames. Frame I/O (terminator framing for serial,
//! datagram boundaries for UDP) is the responsibility of the
//! [`FrameTransport`] implementation, not the codec.

pub mod codec;
pub mod connection;
pub mod error;
pub mod session;
pub mod shared;
pub mod transport;

pub use codec::Codec;
pub use connection::Connection;
pub use error::{SessionError, TransportError};
pub use session::{Hooks, Session, WhileOpen};
pub use shared::SharedTransport;
pub use transport::{FrameTransport, SerialFrameTransport, TransportFactory, UdpFrameTransport};

/// Pinned, heap-allocated, Send-able future used by [`Hooks`] closures.
///
/// Equivalent to `futures::future::BoxFuture`, redefined here so the crate
/// has no `futures` dependency.
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;
