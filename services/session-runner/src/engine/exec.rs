//! The procedure-tree walk: executes a validated [`Instruction`] tree
//! against the blackboard, the tool client, and the clock.
//!
//! Semantics implemented here are pinned in
//! `docs/services/session-runner.md` — § Instructions, § `result` scoping,
//! § Re-entrancy Contract (`once` markers), and § Safety Behavior (the
//! terminated-session path). Two interrupts propagate outward: a workflow
//! error (catchable by `try`) and a session termination (never caught;
//! `finally` blocks still run best-effort).

use std::future::Future;
use std::pin::Pin;

use serde_json::{json, Map, Value};
use tracing::{debug, info};

use crate::blackboard::Blackboard;
use crate::document::{
    ArgValue, Bound, Instruction, InstructionKind, Log, LogLevel, Repeat, RepeatMode, SetEntry,
    ToolCall, Wait,
};
use crate::expr::{EvalContext, Expression};

use super::{Clock, ToolCallError, ToolClient, WorkflowError};

/// Why execution stopped early.
#[derive(Debug)]
pub(super) enum Interrupt {
    /// A workflow error: propagates outward through enclosing `try`s.
    Error(WorkflowError),
    /// `rp` terminated the MCP session (safety): skips `catch` blocks,
    /// runs `finally` blocks best-effort, and ends the run without a
    /// completion.
    Terminated,
}

type ExecResult = Result<(), Interrupt>;

/// One run's execution state.
pub(super) struct Exec<'a, T, C> {
    params: &'a Value,
    blackboard: &'a mut Blackboard,
    tools: &'a T,
    clock: &'a C,
    /// The `result` namespace: the structured result of the most recent
    /// result-producing instruction on the current path (`null` at
    /// session start).
    result: Value,
    /// The `error.*` namespace value while a `catch` or an error-path
    /// `finally` runs; `None` elsewhere (expressions read `null`).
    error: Option<Value>,
}

impl<'a, T, C> Exec<'a, T, C>
where
    T: ToolClient + Sync,
    C: Clock + Sync,
{
    pub(super) fn new(
        params: &'a Value,
        blackboard: &'a mut Blackboard,
        tools: &'a T,
        clock: &'a C,
    ) -> Self {
        Self {
            params,
            blackboard,
            tools,
            clock,
            result: Value::Null,
            error: None,
        }
    }

    fn ctx(&self) -> EvalContext<'_> {
        EvalContext {
            params: Some(self.params),
            session: Some(self.blackboard.value()),
            result: Some(&self.result),
            // Trigger `do` blocks (which carry `event.*`) land in Phase D.
            event: None,
            error: self.error.as_ref(),
            now: self.clock.now(),
        }
    }

    /// Run a block of instructions in order.
    pub(super) async fn exec_block(&mut self, block: &'a [Instruction]) -> ExecResult {
        for ins in block {
            self.exec_boxed(ins).await?;
        }
        Ok(())
    }

    /// The boxed recursion point: `exec_instruction`'s future embeds
    /// container execution, which re-enters here — the box breaks the
    /// otherwise-infinite future type. Depth is bounded by the document
    /// layer's 128-container nesting gate.
    fn exec_boxed<'s>(
        &'s mut self,
        ins: &'a Instruction,
    ) -> Pin<Box<dyn Future<Output = ExecResult> + Send + 's>> {
        Box::pin(self.exec_instruction(ins))
    }

    async fn exec_instruction(&mut self, ins: &'a Instruction) -> ExecResult {
        if let Some(key) = &ins.once {
            if self.blackboard.once_done(key) {
                // A skipped instruction produces nothing: `result` is left
                // unchanged, exactly as if the instruction were absent.
                debug!(once = %key, "skipping instruction: `once` marker already recorded");
                return Ok(());
            }
        }
        match &ins.kind {
            InstructionKind::Tool(call) => self.exec_tool(ins, call).await?,
            InstructionKind::Sequence(body) => self.exec_block(body).await?,
            InstructionKind::Repeat(repeat) => self.exec_repeat(ins, repeat).await?,
            InstructionKind::If {
                condition,
                then,
                otherwise,
            } => {
                if self.condition(ins, condition, "`if` condition")? {
                    self.exec_block(then).await?;
                } else if let Some(otherwise) = otherwise {
                    self.exec_block(otherwise).await?;
                }
            }
            InstructionKind::Set(entries) => self.exec_set(ins, entries).await?,
            InstructionKind::Try {
                body,
                catch,
                finally,
            } => {
                self.exec_try(body, catch.as_deref(), finally.as_deref())
                    .await?;
            }
            InstructionKind::Fail { message } => return Err(self.exec_fail(ins, message)),
            InstructionKind::Wait(wait) => self.exec_wait(ins, wait).await?,
            InstructionKind::Log(log) => self.exec_log(ins, log)?,
        }
        if let Some(key) = &ins.once {
            // Recorded only on successful completion — a failed
            // instruction re-runs on resume.
            debug!(once = %key, "recording `once` completion marker");
            self.blackboard
                .mark_once(key)
                .await
                .map_err(|e| self.error_here(ins, e.to_string()))?;
        }
        Ok(())
    }

    /// Build a workflow error raised at this instruction.
    fn error_here(&self, ins: &Instruction, message: String) -> Interrupt {
        Interrupt::Error(WorkflowError {
            message,
            instruction_id: ins.id.clone(),
            tool: None,
        })
    }

    /// Evaluate an expression; an evaluation error becomes a workflow
    /// error at this instruction, naming the expression's role.
    fn eval(&self, ins: &Instruction, expr: &Expression, role: &str) -> Result<Value, Interrupt> {
        expr.eval(&self.ctx())
            .map_err(|e| self.error_here(ins, format!("{role} `{}` failed: {e}", expr.source())))
    }

    /// Evaluate an expression that must yield a boolean — no truthiness,
    /// matching the expression language's own logic operators.
    fn condition(
        &self,
        ins: &Instruction,
        expr: &Expression,
        role: &str,
    ) -> Result<bool, Interrupt> {
        match self.eval(ins, expr, role)? {
            Value::Bool(b) => Ok(b),
            other => Err(self.error_here(
                ins,
                format!(
                    "{role} `{}` must yield a boolean, got {other}",
                    expr.source()
                ),
            )),
        }
    }

    async fn exec_tool(&mut self, ins: &'a Instruction, call: &'a ToolCall) -> ExecResult {
        let mut args = Map::new();
        for (name, arg) in &call.args {
            let value = match arg {
                ArgValue::Literal(v) => v.clone(),
                ArgValue::Expr(expr) => self.eval(ins, expr, &format!("tool argument `{name}`"))?,
            };
            args.insert(name.clone(), value);
        }
        let max_attempts = call.retry.as_ref().map_or(1, |r| r.max_attempts);
        let mut attempt: u64 = 1;
        loop {
            debug!(tool = %call.tool, attempt, "calling tool");
            match self.tools.call(&call.tool, args.clone()).await {
                Ok(result) => {
                    self.result = result;
                    return Ok(());
                }
                Err(ToolCallError::SessionTerminated(message)) => {
                    debug!(tool = %call.tool, %message, "MCP session terminated during tool call");
                    return Err(Interrupt::Terminated);
                }
                Err(ToolCallError::Failed(message)) if attempt < max_attempts => {
                    debug!(
                        tool = %call.tool,
                        attempt,
                        max_attempts,
                        %message,
                        "tool call failed; retrying after backoff"
                    );
                    if let Some(retry) = &call.retry {
                        self.clock.sleep(retry.backoff).await;
                    }
                    attempt += 1;
                }
                Err(ToolCallError::Failed(message)) => {
                    let message = if max_attempts > 1 {
                        format!(
                            "tool `{}` failed after {max_attempts} attempts: {message}",
                            call.tool
                        )
                    } else {
                        format!("tool `{}` failed: {message}", call.tool)
                    };
                    return Err(Interrupt::Error(WorkflowError {
                        message,
                        instruction_id: ins.id.clone(),
                        tool: Some(call.tool.clone()),
                    }));
                }
            }
        }
    }

    /// Evaluate a loop bound. Expressions must yield an integer-valued
    /// number; `minimum` is 1 for `max_iterations` (an exhausted budget
    /// must represent at least one pass) and 0 for `count`.
    fn bound(
        &self,
        ins: &Instruction,
        bound: &Bound,
        role: &str,
        minimum: u64,
    ) -> Result<u64, Interrupt> {
        let n = match bound {
            Bound::Literal(n) => *n,
            Bound::Expr(expr) => {
                let value = self.eval(ins, expr, role)?;
                integer_value(&value).ok_or_else(|| {
                    self.error_here(
                        ins,
                        format!(
                            "{role} `{}` must yield a non-negative integer, got {value}",
                            expr.source()
                        ),
                    )
                })?
            }
        };
        if n < minimum {
            return Err(self.error_here(ins, format!("{role} must be at least {minimum}, got {n}")));
        }
        Ok(n)
    }

    async fn exec_repeat(&mut self, ins: &'a Instruction, repeat: &'a Repeat) -> ExecResult {
        match &repeat.mode {
            RepeatMode::Until {
                condition,
                max_iterations,
            } => {
                let max = self.bound(ins, max_iterations, "`max_iterations`", 1)?;
                let mut iterations: u64 = 0;
                let mut converged = false;
                while iterations < max {
                    self.exec_block(&repeat.body).await?;
                    iterations += 1;
                    // Checked after each pass, with the `result` that pass
                    // left in scope.
                    if self.condition(ins, condition, "`repeat` `until` condition")? {
                        converged = true;
                        break;
                    }
                }
                self.result = json!({ "iterations": iterations, "converged": converged });
            }
            RepeatMode::While {
                condition,
                max_iterations,
            } => {
                let max = self.bound(ins, max_iterations, "`max_iterations`", 1)?;
                let mut iterations: u64 = 0;
                let converged;
                loop {
                    // Checked before each potential pass — including after
                    // the final permitted one, so a condition that turns
                    // false exactly at the budget still converges.
                    if !self.condition(ins, condition, "`repeat` `while` condition")? {
                        converged = true;
                        break;
                    }
                    if iterations == max {
                        converged = false;
                        break;
                    }
                    self.exec_block(&repeat.body).await?;
                    iterations += 1;
                }
                self.result = json!({ "iterations": iterations, "converged": converged });
            }
            RepeatMode::Count {
                count,
                max_iterations,
            } => {
                let n = self.bound(ins, count, "`count`", 0)?;
                if let Some(guard) = max_iterations {
                    let max = self.bound(ins, guard, "`max_iterations`", 1)?;
                    if n > max {
                        // The guard exists to bound a runaway `$expr`
                        // count: trip it loudly at loop entry rather than
                        // silently truncating the pass count.
                        return Err(self.error_here(
                            ins,
                            format!("`count` ({n}) exceeds `max_iterations` ({max})"),
                        ));
                    }
                }
                for _ in 0..n {
                    self.exec_block(&repeat.body).await?;
                }
                // Count loops report no `converged` — there is no
                // condition to converge on.
                self.result = json!({ "iterations": n });
            }
        }
        Ok(())
    }

    async fn exec_set(&mut self, ins: &'a Instruction, entries: &'a [SetEntry]) -> ExecResult {
        // All values evaluate against the pre-write state — a `set`
        // cannot read its own writes.
        let mut writes = Vec::with_capacity(entries.len());
        for entry in entries {
            let role = format!("`set` value for `{}`", entry.key());
            writes.push(self.eval(ins, &entry.value, &role)?);
        }
        for (entry, value) in entries.iter().zip(writes) {
            self.blackboard
                .set_path(&entry.path, value)
                .map_err(|e| self.error_here(ins, e.to_string()))?;
        }
        // One atomic persist per `set` instruction, before the next
        // instruction runs (the write-on-mutation invariant).
        self.blackboard
            .persist()
            .await
            .map_err(|e| self.error_here(ins, e.to_string()))
    }

    async fn exec_try(
        &mut self,
        body: &'a [Instruction],
        catch: Option<&'a [Instruction]>,
        finally: Option<&'a [Instruction]>,
    ) -> ExecResult {
        let body_outcome = self.exec_block(body).await;
        let after_catch = match body_outcome {
            Err(Interrupt::Error(error)) => {
                if let Some(catch) = catch {
                    debug!(error = %error, "workflow error caught; running `catch`");
                    self.run_with_error_scope(catch, error.to_value()).await
                } else {
                    Err(Interrupt::Error(error))
                }
            }
            // A safety termination skips `catch` — the session is over
            // and `rp` has already secured the equipment.
            other => other,
        };
        let Some(finally) = finally else {
            return after_catch;
        };
        match after_catch {
            Ok(()) => {
                // Success path: the enclosing `error.*` scope (if any)
                // stays visible, and a `finally` failure is a real
                // workflow error.
                self.exec_block(finally).await
            }
            Err(interrupt) => {
                // Error path (or termination): best-effort — run, log
                // failures, never let them mask the original interrupt.
                let outcome = if let Interrupt::Error(error) = &interrupt {
                    self.run_with_error_scope(finally, error.to_value()).await
                } else {
                    self.exec_block(finally).await
                };
                match outcome {
                    Ok(()) => {}
                    // A termination mid-`finally` supersedes everything.
                    Err(Interrupt::Terminated) => return Err(Interrupt::Terminated),
                    Err(Interrupt::Error(finally_error)) => {
                        debug!(
                            error = %finally_error,
                            "`finally` block failed; propagating the original error"
                        );
                    }
                }
                Err(interrupt)
            }
        }
    }

    /// Run a `catch` or error-path `finally` block with `error.*` bound to
    /// the given value, restoring the enclosing scope's value afterwards.
    async fn run_with_error_scope(&mut self, block: &'a [Instruction], error: Value) -> ExecResult {
        let saved = self.error.replace(error);
        let outcome = self.exec_block(block).await;
        self.error = saved;
        outcome
    }

    fn exec_fail(&self, ins: &'a Instruction, message: &'a Expression) -> Interrupt {
        let message = match self.eval(ins, message, "`fail` message") {
            Ok(Value::String(s)) => s,
            // A non-string message is rendered as compact JSON — an error
            // message is terminal output, not data.
            Ok(other) => other.to_string(),
            Err(interrupt) => return interrupt,
        };
        self.error_here(ins, message)
    }

    async fn exec_wait(&mut self, ins: &'a Instruction, wait: &'a Wait) -> ExecResult {
        match wait {
            Wait::Duration(duration) => {
                debug!(duration = %humantime::format_duration(*duration), "waiting");
                self.clock.sleep(*duration).await;
                Ok(())
            }
            Wait::UntilEvent { event, timeout: _ } => Err(self.error_here(
                ins,
                format!(
                    "`wait` `until_event` (`{event}`) is not implemented yet — event \
                     subscriptions land with the trigger engine (workflow-dsl plan, Phase D)"
                ),
            )),
            Wait::Until {
                condition,
                poll_interval,
                timeout,
            } => {
                // The timeout budget is tracked by accumulating the
                // durations actually slept — monotonic by construction
                // (production `sleep` is tokio's monotonic timer), so a
                // wall-clock step (NTP) can neither extend the wait nor
                // fire the timeout early. `clock.now()` (wall time) is
                // only for `seconds_until()` inside the condition, where
                // calendar time is the point.
                let mut elapsed = std::time::Duration::ZERO;
                loop {
                    // Evaluated on entry, after each interval, and once
                    // more when the timeout expires (the last sleep is
                    // clamped to the remaining budget).
                    if self.condition(ins, condition, "`wait` `until` condition")? {
                        return Ok(());
                    }
                    if elapsed >= *timeout {
                        return Err(self.error_here(
                            ins,
                            format!(
                                "`wait` `until` condition `{}` did not become true within {}",
                                condition.source(),
                                humantime::format_duration(*timeout)
                            ),
                        ));
                    }
                    let sleep_for = (*poll_interval).min(*timeout - elapsed);
                    self.clock.sleep(sleep_for).await;
                    elapsed += sleep_for;
                }
            }
        }
    }

    fn exec_log(&self, ins: &'a Instruction, log: &'a Log) -> ExecResult {
        let mut rendered = Map::new();
        for (key, expr) in &log.values {
            let role = format!("`log` value `{key}`");
            rendered.insert(key.clone(), self.eval(ins, expr, &role)?);
        }
        let values = Value::Object(rendered);
        match log.level {
            LogLevel::Debug => {
                debug!(id = ins.id.as_deref(), values = %values, "{}", log.message);
            }
            LogLevel::Info => {
                info!(id = ins.id.as_deref(), values = %values, "{}", log.message);
            }
        }
        Ok(())
    }
}

/// A `Value` as a `u64` loop bound: any JSON number whose value is a
/// non-negative integer within f64's exact-integer range.
fn integer_value(value: &Value) -> Option<u64> {
    let n = value.as_f64()?;
    if n.fract() != 0.0 || n < 0.0 || n > (1u64 << 53) as f64 {
        return None;
    }
    Some(n as u64)
}
