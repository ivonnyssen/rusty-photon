//! End-to-end harness for the operation watchdog (Sentinel) + the predictive
//! deadlines / real-time event stream (rp).
//!
//! This crate carries **no library code** — it exists only to host the
//! `tests/bdd.rs` cucumber suite, which spawns a real `rp` binary and a real
//! `sentinel` binary (plus OmniSim and an in-process plate-solver stub) and
//! drives the watchdog through wedge → escalation → corrective ladder. The
//! per-service BDD suites (`services/rp/tests`, `services/sentinel/tests`)
//! cover each half against stubs; this suite is the only place the two real
//! binaries run the full two-loop structure together.
//!
//! See `docs/services/sentinel.md` §Operation Watchdog and the archived plan
//! `docs/plans/archive/predictive-deadlines-and-watchdog.md`.
