//! Workflow execution: the interpreter that runs a validated
//! [`Document`]'s procedure tree against `rp`'s tool catalog and the
//! session blackboard.
//!
//! The normative execution contract — instruction semantics, `result`
//! scoping, error propagation, the re-entrancy contract, safety behavior —
//! is `docs/services/session-runner.md`; this module implements it against
//! two seams so unit tests need no `rp`: a [`ToolClient`] (the real MCP
//! client arrives with the Phase C service wiring) and a [`Clock`].
//!
//! Phase boundary (`docs/plans/workflow-dsl.md`): this is the Phase C
//! engine core. Trigger evaluation — including `wait` `until_event` and
//! the `event.*` namespace — lands in Phase D; until then a document's
//! declared triggers do not fire (warned at run start) and an
//! `until_event` wait raises a workflow error.

mod exec;
mod io;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod exec_tests;

use serde_json::{json, Value};
use tracing::{debug, info, warn};

pub use io::{Clock, SystemClock, ToolCallError, ToolClient};

use crate::blackboard::Blackboard;
use crate::document::Document;

/// A workflow error: raised by a failed tool call (after retries), an
/// expression evaluation error, a `fail` instruction, a `wait` timeout, or
/// a blackboard write failure; propagates outward through enclosing `try`
/// instructions.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct WorkflowError {
    pub message: String,
    /// The raising instruction's own `id`, when it declares one.
    pub instruction_id: Option<String>,
    /// The tool name when the error came from a tool call; `None`
    /// otherwise.
    pub tool: Option<String>,
}

impl WorkflowError {
    /// The `error.*` namespace value visible in `catch`/`finally`.
    fn to_value(&self) -> Value {
        json!({
            "message": self.message,
            "instruction_id": self.instruction_id,
            "tool": self.tool,
        })
    }
}

/// How a run ended.
#[derive(Debug, PartialEq, Eq)]
pub enum RunOutcome {
    /// The procedure tree ran to completion — post `outcome: "complete"`.
    Completed,
    /// An uncaught workflow error — post `outcome: "failed"` with the
    /// error message.
    Failed(WorkflowError),
    /// `rp` terminated the MCP session (safety). The blackboard is
    /// current (write-on-mutation invariant); the caller exits **without**
    /// posting a completion and awaits re-invocation with recovery
    /// context.
    Terminated,
}

/// Execute `doc`'s procedure tree to completion.
///
/// `params` is the bound parameter object from
/// [`crate::document::bind_parameters`]; `blackboard` is empty for a fresh
/// session or reloaded for a recovery invocation — re-execution from the
/// root against the persisted blackboard *is* the resume model (design
/// § Re-entrancy Contract).
pub async fn run<T, C>(
    doc: &Document,
    params: &Value,
    blackboard: &mut Blackboard,
    tools: &T,
    clock: &C,
) -> RunOutcome
where
    T: ToolClient + Sync,
    C: Clock + Sync,
{
    if !doc.triggers.is_empty() {
        warn!(
            document = %doc.name,
            triggers = doc.triggers.len(),
            "document declares triggers, which this engine does not evaluate yet \
             (workflow-dsl plan, Phase D) — they will not fire"
        );
    }
    let mut exec = exec::Exec::new(params, blackboard, tools, clock);
    match exec.exec_block(std::slice::from_ref(&doc.root)).await {
        Ok(()) => {
            debug!(document = %doc.name, "workflow completed");
            RunOutcome::Completed
        }
        Err(exec::Interrupt::Error(error)) => {
            debug!(document = %doc.name, %error, "workflow failed");
            RunOutcome::Failed(error)
        }
        Err(exec::Interrupt::Terminated) => {
            info!(
                document = %doc.name,
                "MCP session terminated by rp; exiting without completion and awaiting \
                 re-invocation"
            );
            RunOutcome::Terminated
        }
    }
}
