//! `session-runner` — generic imaging-workflow orchestrator (an `rp`
//! orchestrator plugin).
//!
//! Design: `docs/services/session-runner.md`; delivery plan:
//! `docs/plans/workflow-dsl.md`. This crate currently ships the Phase B
//! expression layer ([`expr`]); the document model, engine, and service
//! binary follow in Phase C.

pub mod expr;
