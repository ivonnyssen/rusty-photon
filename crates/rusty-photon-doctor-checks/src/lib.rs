//! rusty-photon-doctor-checks: the no-SDK hardware facts and predicates
//! behind doctor's `hardware.*` check family (docs/services/doctor.md
//! §Hardware). Central doctor consumes it today; the D5 per-service
//! `doctor` subcommands will call the same helpers — ADR-016 decision 6:
//! the similarity between hardware-touching doctors lives in a shared
//! library, not a shared binary.
//!
//! Everything here is read-only: `stat`, directory listings, and inventory
//! queries. Nothing ever opens a device — a running service holds its
//! hardware, and doctor must never contend for it (ADR-016 decision 5).

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

pub mod access;
pub mod facts;
pub mod udev;

pub use access::Identity;
pub use facts::{gather, HardwareFacts, PathFacts, PathKind, ProbeRequest, UsbDevice, UserFacts};
