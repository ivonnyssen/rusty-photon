//! `session-runner` — generic imaging-workflow orchestrator (an `rp`
//! orchestrator plugin).
//!
//! Design: `docs/services/session-runner.md`; delivery plan:
//! `docs/plans/workflow-dsl.md`. This crate currently ships the Phase B
//! expression layer ([`expr`]), the Phase C document layer ([`document`]:
//! model, validation, parameter binding), and the Phase C engine core
//! ([`engine`] + [`blackboard`]: tree execution against the `ToolClient`
//! seam, atomic blackboard persistence); the real MCP client, the HTTP
//! routes, and the service binary follow later in Phase C.

pub mod blackboard;
pub mod document;
pub mod engine;
pub mod expr;
