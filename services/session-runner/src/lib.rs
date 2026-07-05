//! `session-runner` — generic imaging-workflow orchestrator (an `rp`
//! orchestrator plugin).
//!
//! Design: `docs/services/session-runner.md`; delivery plan:
//! `docs/plans/workflow-dsl.md`. This crate currently ships the Phase B
//! expression layer ([`expr`]) and the Phase C document layer
//! ([`document`]: model, validation, parameter binding); the engine and
//! service binary follow later in Phase C.

pub mod document;
pub mod expr;
