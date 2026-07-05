//! Engine semantics tests — the design doc's § Testing Strategy battery,
//! driven against a scripted [`ToolClient`] and a mock [`Clock`]:
//! sequencing, `result` scoping, `set` ordering and persistence,
//! `try`/`catch`/`finally` paths (including finally-does-not-mask),
//! `retry`, loop bounds and `result.converged`, `once` bookkeeping,
//! waits, and the terminated-session (safety) path.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use serde_json::{json, Map, Value};

use super::{run, Clock, RunOutcome, ToolCallError, ToolClient};
use crate::blackboard::Blackboard;
use crate::document::{bind_parameters, Document};

// --- test doubles ---------------------------------------------------------

type Responder =
    Box<dyn Fn(usize, &str, &Map<String, Value>) -> Result<Value, ToolCallError> + Send + Sync>;

/// A `ToolClient` driven by a closure; records every call in order.
struct MockTools {
    responder: Responder,
    calls: Mutex<Vec<(String, Value)>>,
}

impl MockTools {
    fn new(
        responder: impl Fn(usize, &str, &Map<String, Value>) -> Result<Value, ToolCallError>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        Self {
            responder: Box::new(responder),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Every call succeeds with `result`.
    fn ok(result: Value) -> Self {
        Self::new(move |_, _, _| Ok(result.clone()))
    }

    /// Responses consumed in call order; panics when exhausted.
    fn scripted(results: Vec<Result<Value, ToolCallError>>) -> Self {
        let queue = Mutex::new(VecDeque::from(results));
        Self::new(move |_, tool, _| {
            queue
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| panic!("unexpected tool call `{tool}`: script exhausted"))
        })
    }

    /// Panics on any call.
    fn none() -> Self {
        Self::new(|_, tool, _| panic!("unexpected tool call `{tool}`"))
    }

    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }

    fn call_names(&self) -> Vec<String> {
        self.calls().into_iter().map(|(name, _)| name).collect()
    }
}

impl ToolClient for MockTools {
    async fn call(&self, tool: &str, args: Map<String, Value>) -> Result<Value, ToolCallError> {
        let index = {
            let mut calls = self.calls.lock().unwrap();
            calls.push((tool.to_owned(), Value::Object(args.clone())));
            calls.len() - 1
        };
        (self.responder)(index, tool, &args)
    }
}

/// A deterministic clock: `sleep` advances `now` instantly and records the
/// requested duration.
struct MockClock {
    now: Mutex<DateTime<Utc>>,
    sleeps: Mutex<Vec<Duration>>,
}

impl MockClock {
    fn new() -> Self {
        Self {
            now: Mutex::new(Utc.with_ymd_and_hms(2026, 7, 1, 22, 0, 0).unwrap()),
            sleeps: Mutex::new(Vec::new()),
        }
    }

    fn sleeps(&self) -> Vec<Duration> {
        self.sleeps.lock().unwrap().clone()
    }
}

impl Clock for MockClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().unwrap()
    }

    async fn sleep(&self, duration: Duration) {
        let delta = chrono::Duration::from_std(duration).unwrap();
        {
            let mut now = self.now.lock().unwrap();
            *now += delta;
        }
        self.sleeps.lock().unwrap().push(duration);
    }
}

// --- harness --------------------------------------------------------------

fn make_doc(document: Value) -> Document {
    Document::from_value(&document)
        .unwrap_or_else(|issues| panic!("test document invalid: {issues:#?}"))
}

fn doc_with_root(root: Value) -> Document {
    make_doc(json!({ "version": 1, "name": "test", "root": root }))
}

/// Run `doc` against a blackboard file in `dir` (loaded, so repeated runs
/// share persisted state) and return the outcome plus the final session
/// value.
async fn run_in(
    dir: &tempfile::TempDir,
    doc: &Document,
    params: &Value,
    tools: &MockTools,
    clock: &(impl Clock + Sync),
) -> (RunOutcome, Value) {
    let mut blackboard = Blackboard::load(dir.path().join("session.json"))
        .await
        .unwrap();
    let outcome = run(doc, params, &mut blackboard, tools, clock).await;
    let session = blackboard.value().clone();
    (outcome, session)
}

/// One-shot run of a parameterless document.
async fn run_root(root: Value, tools: &MockTools) -> (RunOutcome, Value) {
    let dir = tempfile::tempdir().unwrap();
    let doc = doc_with_root(root);
    run_in(&dir, &doc, &json!({}), tools, &MockClock::new()).await
}

#[track_caller]
fn failure(outcome: RunOutcome) -> super::WorkflowError {
    match outcome {
        RunOutcome::Failed(error) => error,
        other => panic!("expected RunOutcome::Failed, got {other:?}"),
    }
}

// --- sequencing and tool calls --------------------------------------------

#[tokio::test]
async fn test_sequence_calls_tools_in_order_with_literal_and_expr_args() {
    let tools = MockTools::ok(json!({}));
    let doc = make_doc(json!({
        "version": 1, "name": "t",
        "parameters": { "cam": { "type": "string", "required": true } },
        "root": { "sequence": [
            { "tool": "a", "args": { "x": 1, "s": "literal" } },
            { "tool": "b", "args": { "camera_id": { "$expr": "params.cam" } } },
            { "tool": "c" }
        ] }
    }));
    let params = bind_parameters(&doc.parameters, Some(&json!({"cam": "main"}))).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (outcome, _) = run_in(&dir, &doc, &params, &tools, &MockClock::new()).await;

    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(
        tools.calls(),
        vec![
            ("a".to_owned(), json!({"x": 1, "s": "literal"})),
            ("b".to_owned(), json!({"camera_id": "main"})),
            ("c".to_owned(), json!({})),
        ]
    );
}

#[tokio::test]
async fn test_result_scoping_across_instruction_kinds() {
    // `tool` produces a result; `set`, `log`, and `wait` leave it
    // unchanged; an `if` reads it without consuming it; after a container
    // the last inner result is still in scope.
    let tools = MockTools::new(|index, _, _| Ok(json!({ "v": index + 1 })));
    let root = json!({ "sequence": [
        { "tool": "a" },
        { "set": { "session.first": "result.v" } },
        { "log": { "message": "still scoped" } },
        { "wait": { "duration": "1s" } },
        { "set": { "session.after_noops": "result.v" } },
        { "tool": "b" },
        { "if": "result.v == 2",
          "then": [ { "set": { "session.in_branch": "result.v" } } ] },
        { "set": { "session.after_if": "result.v" } }
    ] });
    let (outcome, session) = run_root(root, &tools).await;

    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(session["first"], json!(1));
    assert_eq!(session["after_noops"], json!(1));
    assert_eq!(session["in_branch"], json!(2));
    assert_eq!(session["after_if"], json!(2));
}

#[tokio::test]
async fn test_result_is_null_at_session_start() {
    let tools = MockTools::none();
    let root = json!({ "set": { "session.initial": "result == null" } });
    let (outcome, session) = run_root(root, &tools).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(session["initial"], json!(true));
}

// --- set ------------------------------------------------------------------

#[tokio::test]
async fn test_set_evaluates_all_values_before_writing_any_key() {
    let tools = MockTools::none();
    let root = json!({ "sequence": [
        { "set": { "session.a": "1", "session.b": "2" } },
        // A swap only works if both reads happen before either write.
        { "set": { "session.a": "session.b", "session.b": "session.a" } }
    ] });
    let (outcome, session) = run_root(root, &tools).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(session["a"], json!(2.0));
    assert_eq!(session["b"], json!(1.0));
}

#[tokio::test]
async fn test_set_persists_before_the_next_instruction_runs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.json");
    let probe_path = path.clone();
    // The tool responder reads the blackboard file from disk: whatever the
    // preceding `set` wrote must already be persisted.
    let tools = MockTools::new(move |_, _, _| {
        let bytes = std::fs::read(&probe_path).unwrap();
        Ok(json!({ "on_disk": serde_json::from_slice::<Value>(&bytes).unwrap() }))
    });
    let doc = doc_with_root(json!({ "sequence": [
        { "set": { "session.x": "42" } },
        { "tool": "probe" },
        { "set": { "session.probed": "result.on_disk.x" } }
    ] }));
    let (outcome, session) = run_in(&dir, &doc, &json!({}), &tools, &MockClock::new()).await;

    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(session["probed"], json!(42.0));
}

#[tokio::test]
async fn test_set_through_non_object_intermediate_fails_loud() {
    let tools = MockTools::none();
    let root = json!({ "sequence": [
        { "set": { "session.a": "1" } },
        { "id": "bad-write", "set": { "session.a.b": "2" } }
    ] });
    let (outcome, _) = run_root(root, &tools).await;
    let error = failure(outcome);
    assert_eq!(
        error.message,
        "cannot set `session.a.b`: `session.a` is not an object"
    );
    assert_eq!(error.instruction_id.as_deref(), Some("bad-write"));
}

#[tokio::test]
async fn test_set_value_evaluation_error_is_a_workflow_error() {
    let tools = MockTools::none();
    let root = json!({ "id": "s1", "set": { "session.x": "session.missing + 1" } });
    let (outcome, _) = run_root(root, &tools).await;
    let error = failure(outcome);
    assert!(
        error
            .message
            .starts_with("`set` value for `session.x` `session.missing + 1` failed:"),
        "{}",
        error.message
    );
    assert!(error.message.contains("(at "), "{}", error.message);
    assert_eq!(error.instruction_id.as_deref(), Some("s1"));
    assert_eq!(error.tool, None);
}

// --- try / catch / finally -------------------------------------------------

#[tokio::test]
async fn test_try_success_skips_catch_and_runs_finally() {
    let tools = MockTools::ok(json!({}));
    let root = json!({
        "try": [ { "tool": "work" } ],
        "catch": [ { "tool": "never" } ],
        "finally": [ { "tool": "cleanup" } ]
    });
    let (outcome, _) = run_root(root, &tools).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(tools.call_names(), vec!["work", "cleanup"]);
}

#[tokio::test]
async fn test_catch_sees_error_namespace_and_handles_the_error() {
    let tools = MockTools::new(|_, tool, _| match tool {
        "explode" => Err(ToolCallError::Failed("kaput".to_owned())),
        _ => Ok(json!({})),
    });
    let root = json!({
        "try": [ { "id": "boom", "tool": "explode" } ],
        "catch": [ { "set": {
            "session.msg": "error.message",
            "session.iid": "error.instruction_id",
            "session.tool": "error.tool"
        } } ],
        "finally": [ { "tool": "cleanup" } ]
    });
    let (outcome, session) = run_root(root, &tools).await;

    assert_eq!(outcome, RunOutcome::Completed, "catch handles the error");
    assert_eq!(session["msg"], json!("tool `explode` failed: kaput"));
    assert_eq!(session["iid"], json!("boom"));
    assert_eq!(session["tool"], json!("explode"));
    assert_eq!(tools.call_names(), vec!["explode", "cleanup"]);
}

#[tokio::test]
async fn test_error_namespace_for_a_fail_raised_error_has_null_tool() {
    let tools = MockTools::none();
    let root = json!({
        "try": [ { "fail": { "message": "'deliberate'" } } ],
        "catch": [ { "set": {
            "session.tool_is_null": "error.tool == null",
            "session.iid_is_null": "error.instruction_id == null"
        } } ]
    });
    let (outcome, session) = run_root(root, &tools).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(session["tool_is_null"], json!(true));
    assert_eq!(session["iid_is_null"], json!(true));
}

#[tokio::test]
async fn test_catch_reraise_via_fail_propagates_and_finally_still_runs() {
    let tools = MockTools::new(|_, tool, _| match tool {
        "explode" => Err(ToolCallError::Failed("kaput".to_owned())),
        _ => Ok(json!({})),
    });
    let root = json!({
        "try": [ { "tool": "explode" } ],
        "catch": [ { "fail": { "message": "error.message" } } ],
        "finally": [ { "tool": "cleanup" } ]
    });
    let (outcome, _) = run_root(root, &tools).await;
    let error = failure(outcome);
    assert_eq!(error.message, "tool `explode` failed: kaput");
    assert_eq!(tools.call_names(), vec!["explode", "cleanup"]);
}

#[tokio::test]
async fn test_finally_failure_on_the_success_path_is_a_workflow_error() {
    let tools = MockTools::new(|_, tool, _| match tool {
        "cleanup" => Err(ToolCallError::Failed("jammed".to_owned())),
        _ => Ok(json!({})),
    });
    let root = json!({
        "try": [ { "tool": "work" } ],
        "finally": [ { "tool": "cleanup" } ]
    });
    let (outcome, _) = run_root(root, &tools).await;
    assert_eq!(failure(outcome).message, "tool `cleanup` failed: jammed");
}

#[tokio::test]
async fn test_finally_failure_never_masks_the_original_error() {
    let tools = MockTools::new(|_, tool, _| Err(ToolCallError::Failed(format!("{tool} broke"))));
    let root = json!({
        "try": [ { "tool": "work" } ],
        "finally": [ { "tool": "cleanup" } ]
    });
    let (outcome, _) = run_root(root, &tools).await;
    assert_eq!(failure(outcome).message, "tool `work` failed: work broke");
    assert_eq!(tools.call_names(), vec!["work", "cleanup"]);
}

#[tokio::test]
async fn test_nested_try_restores_the_enclosing_error_scope() {
    let tools = MockTools::new(|_, tool, _| match tool {
        "outer_boom" => Err(ToolCallError::Failed("outer".to_owned())),
        "inner_boom" => Err(ToolCallError::Failed("inner".to_owned())),
        _ => Ok(json!({})),
    });
    let root = json!({
        "try": [ { "tool": "outer_boom" } ],
        "catch": [
            { "try": [ { "tool": "inner_boom" } ],
              "catch": [ { "set": { "session.inner": "error.message" } } ] },
            { "set": { "session.outer": "error.message" } }
        ]
    });
    let (outcome, session) = run_root(root, &tools).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(session["inner"], json!("tool `inner_boom` failed: inner"));
    assert_eq!(session["outer"], json!("tool `outer_boom` failed: outer"));
}

#[tokio::test]
async fn test_success_path_finally_keeps_the_enclosing_error_scope_visible() {
    let tools = MockTools::new(|_, tool, _| match tool {
        "explode" => Err(ToolCallError::Failed("kaput".to_owned())),
        _ => Ok(json!({})),
    });
    // The inner try succeeds, so its `finally` runs on the success path —
    // the outer catch's `error.*` (the error still being handled) stays
    // visible rather than being nulled out.
    let root = json!({
        "try": [ { "tool": "explode" } ],
        "catch": [
            { "try": [ { "log": { "message": "recovering" } } ],
              "finally": [ { "set": { "session.seen": "error.message" } } ] }
        ]
    });
    let (outcome, session) = run_root(root, &tools).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(session["seen"], json!("tool `explode` failed: kaput"));
}

#[tokio::test]
async fn test_error_namespace_is_null_in_a_top_level_success_finally() {
    let tools = MockTools::none();
    let root = json!({
        "try": [ { "set": { "session.x": "1" } } ],
        "finally": [ { "set": { "session.has_error": "has(error.message)" } } ]
    });
    let (outcome, session) = run_root(root, &tools).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(session["has_error"], json!(false));
}

// --- retry -------------------------------------------------------------------

#[tokio::test]
async fn test_retry_retries_with_backoff_until_success() {
    let tools = MockTools::scripted(vec![
        Err(ToolCallError::Failed("flake 1".to_owned())),
        Err(ToolCallError::Failed("flake 2".to_owned())),
        Ok(json!({"ok": true})),
    ]);
    let clock = MockClock::new();
    let dir = tempfile::tempdir().unwrap();
    let doc = doc_with_root(json!({ "sequence": [
        { "tool": "flaky", "retry": { "max_attempts": 3, "backoff": "10s" } },
        { "set": { "session.ok": "result.ok" } }
    ] }));
    let (outcome, session) = run_in(&dir, &doc, &json!({}), &tools, &clock).await;

    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(session["ok"], json!(true));
    assert_eq!(tools.call_names(), vec!["flaky", "flaky", "flaky"]);
    assert_eq!(
        clock.sleeps(),
        vec![Duration::from_secs(10), Duration::from_secs(10)]
    );
}

#[tokio::test]
async fn test_retry_exhaustion_raises_with_the_attempt_count() {
    let tools = MockTools::new(|_, _, _| Err(ToolCallError::Failed("nope".to_owned())));
    let root = json!({ "id": "t1",
        "tool": "flaky", "retry": { "max_attempts": 2, "backoff": "1s" } });
    let (outcome, _) = run_root(root, &tools).await;
    let error = failure(outcome);
    assert_eq!(error.message, "tool `flaky` failed after 2 attempts: nope");
    assert_eq!(error.instruction_id.as_deref(), Some("t1"));
    assert_eq!(error.tool.as_deref(), Some("flaky"));
    assert_eq!(tools.calls().len(), 2);
}

#[tokio::test]
async fn test_tool_failure_without_retry_names_no_attempt_count() {
    let tools = MockTools::new(|_, _, _| Err(ToolCallError::Failed("nope".to_owned())));
    let (outcome, _) = run_root(json!({ "tool": "x" }), &tools).await;
    assert_eq!(failure(outcome).message, "tool `x` failed: nope");
}

#[tokio::test]
async fn test_session_termination_is_never_retried() {
    let tools =
        MockTools::new(|_, _, _| Err(ToolCallError::SessionTerminated("safety".to_owned())));
    let clock = MockClock::new();
    let dir = tempfile::tempdir().unwrap();
    let doc = doc_with_root(
        json!({ "tool": "capture", "retry": { "max_attempts": 5, "backoff": "10s" } }),
    );
    let (outcome, _) = run_in(&dir, &doc, &json!({}), &tools, &clock).await;

    assert_eq!(outcome, RunOutcome::Terminated);
    assert_eq!(tools.calls().len(), 1);
    assert_eq!(clock.sleeps(), Vec::<Duration>::new());
}

// --- repeat ------------------------------------------------------------------

#[tokio::test]
async fn test_repeat_until_converges_and_reports_the_loop_summary() {
    let tools = MockTools::new(|index, _, _| Ok(json!({ "n": index + 1 })));
    let root = json!({ "sequence": [
        { "repeat": { "until": "result.n >= 3", "max_iterations": 10 },
          "body": [ { "tool": "step" } ] },
        { "set": { "session.iterations": "result.iterations",
                   "session.converged": "result.converged" } }
    ] });
    let (outcome, session) = run_root(root, &tools).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(tools.calls().len(), 3);
    assert_eq!(session["iterations"], json!(3));
    assert_eq!(session["converged"], json!(true));
}

#[tokio::test]
async fn test_repeat_until_exhaustion_completes_with_converged_false() {
    let tools = MockTools::ok(json!({ "n": 0 }));
    let root = json!({ "sequence": [
        { "repeat": { "until": "result.n >= 99", "max_iterations": 2 },
          "body": [ { "tool": "step" } ] },
        { "set": { "session.iterations": "result.iterations",
                   "session.converged": "result.converged" } }
    ] });
    let (outcome, session) = run_root(root, &tools).await;
    // Exhaustion is not an error — the loop completes and the document
    // decides what `converged == false` means.
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(tools.calls().len(), 2);
    assert_eq!(session["iterations"], json!(2));
    assert_eq!(session["converged"], json!(false));
}

#[tokio::test]
async fn test_repeat_while_runs_while_the_condition_holds() {
    // The second `work` call says stop; the body's `if` flips the gate.
    let tools = MockTools::new(|index, _, _| Ok(json!({ "stop": index == 1 })));
    let root = json!({ "sequence": [
        { "set": { "session.go": "true" } },
        { "repeat": { "while": "session.go == true", "max_iterations": 5 },
          "body": [
            { "tool": "work" },
            { "if": "result.stop == true",
              "then": [ { "set": { "session.go": "false" } } ] }
          ] },
        { "set": { "session.iterations": "result.iterations",
                   "session.converged": "result.converged" } }
    ] });
    let (outcome, session) = run_root(root, &tools).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(tools.calls().len(), 2);
    assert_eq!(session["iterations"], json!(2));
    assert_eq!(session["converged"], json!(true));
}

#[tokio::test]
async fn test_repeat_while_false_at_entry_runs_zero_passes() {
    let tools = MockTools::none();
    let root = json!({ "sequence": [
        { "repeat": { "while": "session.go == true", "max_iterations": 5 },
          "body": [ { "tool": "work" } ] },
        { "set": { "session.iterations": "result.iterations",
                   "session.converged": "result.converged" } }
    ] });
    let (outcome, session) = run_root(root, &tools).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(session["iterations"], json!(0));
    assert_eq!(session["converged"], json!(true));
}

#[tokio::test]
async fn test_repeat_while_exhaustion_with_the_condition_still_true() {
    let tools = MockTools::ok(json!({}));
    let root = json!({ "sequence": [
        { "repeat": { "while": "true", "max_iterations": 2 },
          "body": [ { "tool": "work" } ] },
        { "set": { "session.iterations": "result.iterations",
                   "session.converged": "result.converged" } }
    ] });
    let (outcome, session) = run_root(root, &tools).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(tools.calls().len(), 2);
    assert_eq!(session["iterations"], json!(2));
    assert_eq!(session["converged"], json!(false));
}

#[tokio::test]
async fn test_repeat_while_condition_turning_false_at_the_budget_still_converges() {
    // The gate flips after the second (final permitted) pass: the engine
    // evaluates the condition once more before declaring exhaustion, so
    // this loop converged.
    let tools = MockTools::new(|index, _, _| Ok(json!({ "stop": index == 1 })));
    let root = json!({ "sequence": [
        { "set": { "session.go": "true" } },
        { "repeat": { "while": "session.go == true", "max_iterations": 2 },
          "body": [
            { "tool": "work" },
            { "if": "result.stop == true",
              "then": [ { "set": { "session.go": "false" } } ] }
          ] },
        { "set": { "session.converged": "result.converged" } }
    ] });
    let (outcome, session) = run_root(root, &tools).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(session["converged"], json!(true));
}

#[tokio::test]
async fn test_repeat_count_runs_exactly_count_passes() {
    let tools = MockTools::ok(json!({}));
    let root = json!({ "sequence": [
        { "repeat": { "count": 4 }, "body": [ { "tool": "capture" } ] },
        { "set": { "session.iterations": "result.iterations",
                   "session.has_converged": "has(result.converged)" } }
    ] });
    let (outcome, session) = run_root(root, &tools).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(tools.calls().len(), 4);
    assert_eq!(session["iterations"], json!(4));
    // Count loops have no condition, so no `converged` in the summary.
    assert_eq!(session["has_converged"], json!(false));
}

#[tokio::test]
async fn test_repeat_count_zero_runs_no_passes() {
    let tools = MockTools::none();
    let root = json!({ "sequence": [
        { "repeat": { "count": 0 }, "body": [ { "tool": "capture" } ] },
        { "set": { "session.iterations": "result.iterations" } }
    ] });
    let (outcome, session) = run_root(root, &tools).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(session["iterations"], json!(0));
}

#[tokio::test]
async fn test_repeat_count_from_a_parameter_expression() {
    let tools = MockTools::ok(json!({}));
    let doc = make_doc(json!({
        "version": 1, "name": "t",
        "parameters": { "count": { "type": "integer", "required": true } },
        "root": { "repeat": { "count": { "$expr": "params.count" } },
                  "body": [ { "tool": "capture" } ] }
    }));
    let params = bind_parameters(&doc.parameters, Some(&json!({"count": 3}))).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (outcome, _) = run_in(&dir, &doc, &params, &tools, &MockClock::new()).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(tools.calls().len(), 3);
}

#[tokio::test]
async fn test_repeat_count_exceeding_its_max_iterations_guard_fails_at_entry() {
    let tools = MockTools::none();
    let root = json!({ "id": "guarded",
        "repeat": { "count": { "$expr": "2 + 3" }, "max_iterations": 2 },
        "body": [ { "tool": "capture" } ] });
    let (outcome, _) = run_root(root, &tools).await;
    let error = failure(outcome);
    assert_eq!(error.message, "`count` (5) exceeds `max_iterations` (2)");
    assert_eq!(error.instruction_id.as_deref(), Some("guarded"));
    assert!(tools.calls().is_empty(), "no pass may run");
}

#[tokio::test]
async fn test_repeat_bound_expressions_must_yield_usable_integers() {
    let cases = [
        (
            json!({ "repeat": { "until": "false", "max_iterations": { "$expr": "0 - 1" } },
                    "body": [ { "log": { "message": "x" } } ] }),
            "`max_iterations` `0 - 1` must yield a non-negative integer, got -1.0",
        ),
        (
            json!({ "repeat": { "until": "false", "max_iterations": { "$expr": "2.5" } },
                    "body": [ { "log": { "message": "x" } } ] }),
            "`max_iterations` `2.5` must yield a non-negative integer, got 2.5",
        ),
        (
            json!({ "repeat": { "until": "false", "max_iterations": { "$expr": "0" } },
                    "body": [ { "log": { "message": "x" } } ] }),
            "`max_iterations` must be at least 1, got 0",
        ),
        (
            json!({ "repeat": { "count": { "$expr": "'three'" } },
                    "body": [ { "log": { "message": "x" } } ] }),
            "`count` `'three'` must yield a non-negative integer, got \"three\"",
        ),
    ];
    for (root, expected) in cases {
        let tools = MockTools::none();
        let (outcome, _) = run_root(root, &tools).await;
        assert_eq!(failure(outcome).message, expected);
    }
}

#[tokio::test]
async fn test_repeat_body_error_propagates_out_of_the_loop() {
    let tools = MockTools::new(|index, _, _| {
        if index == 1 {
            Err(ToolCallError::Failed("pass 2 broke".to_owned()))
        } else {
            Ok(json!({}))
        }
    });
    let root = json!({ "repeat": { "count": 5 }, "body": [ { "tool": "step" } ] });
    let (outcome, _) = run_root(root, &tools).await;
    assert_eq!(failure(outcome).message, "tool `step` failed: pass 2 broke");
    assert_eq!(tools.calls().len(), 2);
}

// --- once markers ------------------------------------------------------------

#[tokio::test]
async fn test_once_skips_on_reexecution_and_leaves_result_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let doc = doc_with_root(json!({ "sequence": [
        { "tool": "arm", "once": "armed" },
        { "set": { "session.result_v": "result.v" } }
    ] }));
    let tools = MockTools::ok(json!({ "v": 1 }));

    let (outcome, session) = run_in(&dir, &doc, &json!({}), &tools, &MockClock::new()).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(session["result_v"], json!(1));
    assert_eq!(session["_once"], json!({ "armed": true }));
    assert_eq!(tools.calls().len(), 1);

    // Re-execution (the resume model): the marked instruction is skipped
    // and, having produced nothing, leaves `result` at its session-start
    // null — so the following `set` writes null.
    let (outcome, session) = run_in(&dir, &doc, &json!({}), &tools, &MockClock::new()).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(session["result_v"], Value::Null);
    assert_eq!(tools.calls().len(), 1, "the armed tool must not run again");
}

#[tokio::test]
async fn test_once_marker_is_not_recorded_when_the_instruction_fails() {
    let dir = tempfile::tempdir().unwrap();
    let doc = doc_with_root(json!({ "tool": "arm", "once": "armed" }));

    let failing = MockTools::new(|_, _, _| Err(ToolCallError::Failed("no".to_owned())));
    let (outcome, session) = run_in(&dir, &doc, &json!({}), &failing, &MockClock::new()).await;
    failure(outcome);
    assert_eq!(session.get("_once"), None);

    // The instruction re-runs on the next invocation and completes.
    let working = MockTools::ok(json!({}));
    let (outcome, session) = run_in(&dir, &doc, &json!({}), &working, &MockClock::new()).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(session["_once"], json!({ "armed": true }));
    assert_eq!(working.calls().len(), 1);
}

// --- wait --------------------------------------------------------------------

#[tokio::test]
async fn test_wait_duration_sleeps_once() {
    let tools = MockTools::none();
    let clock = MockClock::new();
    let dir = tempfile::tempdir().unwrap();
    let doc = doc_with_root(json!({ "wait": { "duration": "30s" } }));
    let (outcome, _) = run_in(&dir, &doc, &json!({}), &tools, &clock).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(clock.sleeps(), vec![Duration::from_secs(30)]);
}

#[tokio::test]
async fn test_wait_until_polls_until_the_condition_turns_true() {
    // The mock clock starts at 2026-07-01T22:00:00Z; the condition turns
    // true one minute later. Six 10 s polls get there.
    let tools = MockTools::none();
    let clock = MockClock::new();
    let dir = tempfile::tempdir().unwrap();
    let doc = doc_with_root(json!({ "wait": {
        "until": "seconds_until('2026-07-01T22:01:00Z') <= 0",
        "poll_interval": "10s",
        "timeout": "5m"
    } }));
    let (outcome, _) = run_in(&dir, &doc, &json!({}), &tools, &clock).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(clock.sleeps(), vec![Duration::from_secs(10); 6]);
}

#[tokio::test]
async fn test_wait_until_timeout_raises_after_a_final_check() {
    let tools = MockTools::none();
    let clock = MockClock::new();
    let dir = tempfile::tempdir().unwrap();
    let doc = doc_with_root(json!({ "id": "w1", "wait": {
        "until": "false",
        "poll_interval": "40s",
        "timeout": "1m 30s"
    } }));
    let (outcome, _) = run_in(&dir, &doc, &json!({}), &tools, &clock).await;
    let error = failure(outcome);
    assert_eq!(
        error.message,
        "`wait` `until` condition `false` did not become true within 1m 30s"
    );
    assert_eq!(error.instruction_id.as_deref(), Some("w1"));
    // The last sleep is clamped to the remaining budget, so the condition
    // gets a final evaluation exactly at the timeout.
    assert_eq!(
        clock.sleeps(),
        vec![
            Duration::from_secs(40),
            Duration::from_secs(40),
            Duration::from_secs(10)
        ]
    );
}

/// A clock whose wall time leaps an hour forward across every sleep — the
/// NTP-step scenario a wall-clock-based timeout would misread.
struct SteppingClock {
    now: Mutex<DateTime<Utc>>,
    sleeps: Mutex<Vec<Duration>>,
}

impl SteppingClock {
    fn new() -> Self {
        Self {
            now: Mutex::new(Utc.with_ymd_and_hms(2026, 7, 1, 22, 0, 0).unwrap()),
            sleeps: Mutex::new(Vec::new()),
        }
    }
}

impl Clock for SteppingClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().unwrap()
    }

    async fn sleep(&self, duration: Duration) {
        let step = chrono::Duration::from_std(duration).unwrap() + chrono::Duration::hours(1);
        {
            let mut now = self.now.lock().unwrap();
            *now += step;
        }
        self.sleeps.lock().unwrap().push(duration);
    }
}

#[tokio::test]
async fn test_wait_until_timeout_is_immune_to_wall_clock_steps() {
    // The timeout budget is accumulated sleep time, not wall-clock
    // differences — an NTP step (here +1 h per sleep) must neither fire
    // the timeout early nor extend the wait.
    let tools = MockTools::none();
    let clock = SteppingClock::new();
    let dir = tempfile::tempdir().unwrap();
    let doc = doc_with_root(json!({ "wait": {
        "until": "false",
        "poll_interval": "10s",
        "timeout": "30s"
    } }));
    let (outcome, _) = run_in(&dir, &doc, &json!({}), &tools, &clock).await;

    assert_eq!(
        failure(outcome).message,
        "`wait` `until` condition `false` did not become true within 30s"
    );
    // The full poll schedule ran despite `now()` racing three hours ahead.
    assert_eq!(
        clock.sleeps.lock().unwrap().clone(),
        vec![Duration::from_secs(10); 3]
    );
}

#[tokio::test]
async fn test_wait_until_event_is_a_phase_d_gap_for_now() {
    let tools = MockTools::none();
    let root = json!({ "wait": { "until_event": "guide_settled", "timeout": "5m" } });
    let (outcome, _) = run_root(root, &tools).await;
    assert_eq!(
        failure(outcome).message,
        "`wait` `until_event` (`guide_settled`) is not implemented yet — event \
         subscriptions land with the trigger engine (workflow-dsl plan, Phase D)"
    );
}

// --- fail, if, log -------------------------------------------------------------

#[tokio::test]
async fn test_fail_raises_with_the_evaluated_message() {
    let tools = MockTools::none();
    let root = json!({ "id": "give-up", "fail": { "message": "'exposure never converged'" } });
    let (outcome, _) = run_root(root, &tools).await;
    let error = failure(outcome);
    assert_eq!(error.message, "exposure never converged");
    assert_eq!(error.instruction_id.as_deref(), Some("give-up"));
    assert_eq!(error.tool, None);
}

#[tokio::test]
async fn test_fail_renders_a_non_string_message_as_json() {
    let tools = MockTools::none();
    let root = json!({ "fail": { "message": "1 + 1" } });
    let (outcome, _) = run_root(root, &tools).await;
    assert_eq!(failure(outcome).message, "2.0");
}

#[tokio::test]
async fn test_if_takes_the_else_branch_and_tolerates_a_missing_else() {
    let tools = MockTools::none();
    let root = json!({ "sequence": [
        { "if": "1 > 2",
          "then": [ { "set": { "session.then": "true" } } ],
          "else": [ { "set": { "session.else": "true" } } ] },
        { "if": "1 > 2",
          "then": [ { "set": { "session.skipped": "true" } } ] }
    ] });
    let (outcome, session) = run_root(root, &tools).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(session.get("then"), None);
    assert_eq!(session["else"], json!(true));
    assert_eq!(session.get("skipped"), None);
}

#[tokio::test]
async fn test_if_condition_must_be_a_boolean() {
    let tools = MockTools::none();
    let root = json!({ "if": "1 + 1", "then": [ { "log": { "message": "x" } } ] });
    let (outcome, _) = run_root(root, &tools).await;
    assert_eq!(
        failure(outcome).message,
        "`if` condition `1 + 1` must yield a boolean, got 2.0"
    );
}

#[tokio::test]
async fn test_log_value_evaluation_errors_are_workflow_errors() {
    let tools = MockTools::none();
    let root = json!({ "log": { "message": "m", "values": { "v": "session.x + 1" } } });
    let (outcome, _) = run_root(root, &tools).await;
    let error = failure(outcome);
    assert!(
        error
            .message
            .starts_with("`log` value `v` `session.x + 1` failed:"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn test_log_renders_values_and_continues() {
    let tools = MockTools::none();
    let root = json!({ "sequence": [
        { "log": { "level": "info", "message": "hello", "values": { "n": "1 + 1" } } },
        { "set": { "session.after": "true" } }
    ] });
    let (outcome, session) = run_root(root, &tools).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(session["after"], json!(true));
}

// --- safety termination --------------------------------------------------------

#[tokio::test]
async fn test_termination_skips_catch_runs_finally_best_effort_and_ends_the_run() {
    let tools = MockTools::new(|_, tool, _| match tool {
        "a" => Ok(json!({})),
        // Everything after the safety termination fails too — rp has torn
        // the MCP session down.
        _ => Err(ToolCallError::SessionTerminated("unsafe".to_owned())),
    });
    let root = json!({ "sequence": [
        { "try": [ { "tool": "a" }, { "tool": "b" }, { "tool": "never_reached" } ],
          "catch": [ { "tool": "catch_never_runs" } ],
          "finally": [ { "tool": "cleanup" } ] },
        { "tool": "after_try_never_runs" }
    ] });
    let (outcome, _) = run_root(root, &tools).await;

    assert_eq!(outcome, RunOutcome::Terminated);
    assert_eq!(tools.call_names(), vec!["a", "b", "cleanup"]);
}

#[tokio::test]
async fn test_termination_propagates_even_when_finally_merely_fails() {
    let tools = MockTools::new(|_, tool, _| match tool {
        "b" => Err(ToolCallError::SessionTerminated("unsafe".to_owned())),
        "cleanup" => Err(ToolCallError::Failed("session gone".to_owned())),
        _ => Ok(json!({})),
    });
    let root = json!({ "try": [ { "tool": "b" } ],
                       "finally": [ { "tool": "cleanup" } ] });
    let (outcome, _) = run_root(root, &tools).await;
    assert_eq!(outcome, RunOutcome::Terminated);
    assert_eq!(tools.call_names(), vec!["b", "cleanup"]);
}

#[tokio::test]
async fn test_blackboard_reflects_every_completed_set_at_termination() {
    let dir = tempfile::tempdir().unwrap();
    let tools = MockTools::new(|_, tool, _| match tool {
        "boom" => Err(ToolCallError::SessionTerminated("unsafe".to_owned())),
        _ => Ok(json!({})),
    });
    let doc = doc_with_root(json!({ "sequence": [
        { "set": { "session.progress": "7" } },
        { "tool": "boom" }
    ] }));
    let (outcome, _) = run_in(&dir, &doc, &json!({}), &tools, &MockClock::new()).await;
    assert_eq!(outcome, RunOutcome::Terminated);

    let on_disk: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("session.json")).unwrap()).unwrap();
    assert_eq!(on_disk["progress"], json!(7.0));
}

// --- documents with triggers (Phase D gap) ---------------------------------------

#[tokio::test]
async fn test_documents_with_triggers_run_but_triggers_do_not_fire() {
    let tools = MockTools::none();
    let doc = make_doc(json!({
        "version": 1, "name": "t",
        "triggers": [ { "id": "t1", "on": { "event": "exposure_complete" },
                        "do": [ { "tool": "never" } ] } ],
        "root": { "set": { "session.ran": "true" } }
    }));
    let dir = tempfile::tempdir().unwrap();
    let (outcome, session) = run_in(&dir, &doc, &json!({}), &tools, &MockClock::new()).await;
    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(session["ran"], json!(true));
    assert!(tools.calls().is_empty());
}

// --- the golden document end-to-end ----------------------------------------------

/// Responder for the shipped `calibrator_flats.json` golden document,
/// faithful to `rp`'s actual tool results: `get_camera_info` reports the
/// exposure limits as humantime strings, `capture` returns
/// `image_path`/`document_id`, `compute_image_stats` serves the scripted
/// medians, and the cover/calibrator/filter tools return their status
/// objects.
fn flats_tools(medians: Vec<u32>) -> MockTools {
    let medians = Mutex::new(VecDeque::from(medians));
    MockTools::new(move |_, tool, _| match tool {
        "get_camera_info" => Ok(json!({
            "camera_id": "cam",
            "max_adu": 65535,
            "exposure_min": "1ms",
            "exposure_max": "30s"
        })),
        "capture" => Ok(json!({ "image_path": "/tmp/flat.fits", "document_id": "doc-1" })),
        "compute_image_stats" => Ok(json!({
            "median_adu": medians
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected compute_image_stats call")
        })),
        "set_filter" => Ok(json!({ "filter_wheel_id": "fw", "position": 0 })),
        _ => Ok(json!({ "status": "ok" })),
    })
}

fn flats_params(doc: &Document, filters: Value) -> Value {
    bind_parameters(
        &doc.parameters,
        Some(&json!({
            "camera_id": "cam",
            "filter_wheel_id": "fw",
            "calibrator_id": "panel",
            "filters": filters
        })),
    )
    .unwrap()
}

#[tokio::test]
async fn test_golden_calibrator_flats_document_runs_the_full_algorithm() {
    let doc = make_doc(crate::document::corpus::golden_calibrator_flats());
    let params = flats_params(
        &doc,
        json!([{ "name": "L", "count": 2 }, { "name": "R", "count": 1 }]),
    );
    // target_adu = 65535 * 0.5 = 32767.5. For L the first median (16000)
    // is outside the 5 % tolerance and the exposure is rescaled; the
    // second (32000) is inside it — two passes. For R the very first
    // median converges.
    let tools = flats_tools(vec![16000, 32000, 32000]);
    let dir = tempfile::tempdir().unwrap();
    let (outcome, session) = run_in(&dir, &doc, &params, &tools, &MockClock::new()).await;

    assert_eq!(outcome, RunOutcome::Completed);
    assert_eq!(
        tools.call_names(),
        vec![
            "get_camera_info",
            "close_cover",
            "calibrator_on",
            "set_filter",          // L
            "capture",             // find-exposure pass 1
            "compute_image_stats", // 16000 → rescale
            "capture",             // find-exposure pass 2
            "compute_image_stats", // 32000 → converged
            "capture",             // L flat 1
            "capture",             // L flat 2
            "set_filter",          // R
            "capture",             // find-exposure pass 1
            "compute_image_stats", // 32000 → converged
            "capture",             // R flat 1
            "calibrator_off",
            "open_cover",
        ]
    );
    assert_eq!(session["target_adu"], json!(32767.5));
    // `set` copies `result.median_adu` verbatim — the mock's JSON integer
    // survives untouched (arithmetic-produced values below are f64).
    assert_eq!(session["median_adu"], json!(32000));
    assert_eq!(session["filter_index"], json!(2.0));
    assert_eq!(session["report"]["total_frames"], json!(3.0));

    let captures: Vec<Value> = tools
        .calls()
        .into_iter()
        .filter(|(name, _)| name == "capture")
        .map(|(_, args)| args["duration"].clone())
        .collect();
    // L's search starts at the 1 s initial exposure, rescales once
    // (32767.5 / 16000 ≈ 2.048 s), and — matching the Rust orchestrator —
    // the converging pass does NOT rescale again: both L flats reuse the
    // duration that converged. R's search resets to the initial exposure
    // and converges immediately, so R's flat uses 1 s.
    assert_eq!(captures[0], json!("1s"));
    assert_eq!(captures[1], captures[2], "converged duration was rescaled");
    assert_eq!(captures[2], captures[3]);
    assert_ne!(captures[1], json!("1s"));
    assert_eq!(captures[4], json!("1s"));
    assert_eq!(captures[5], json!("1s"));
}

#[tokio::test]
async fn test_golden_calibrator_flats_cleans_up_when_the_loop_fails() {
    let doc = make_doc(crate::document::corpus::golden_calibrator_flats());
    let params = flats_params(&doc, json!([{ "name": "L", "count": 2 }]));
    let tools = MockTools::new(|_, tool, _| match tool {
        "get_camera_info" => Ok(json!({
            "max_adu": 65535,
            "exposure_min": "1ms",
            "exposure_max": "30s"
        })),
        "compute_image_stats" => Err(ToolCallError::Failed("stats broke".to_owned())),
        _ => Ok(json!({ "status": "ok" })),
    });
    let dir = tempfile::tempdir().unwrap();
    let (outcome, _) = run_in(&dir, &doc, &params, &tools, &MockClock::new()).await;

    // The error propagates out of the find-exposure loop, but the
    // `finally` block still turns the panel off and reopens the cover —
    // the calibrator-flats cleanup-on-failure contract.
    let error = failure(outcome);
    assert_eq!(
        error.message,
        "tool `compute_image_stats` failed: stats broke"
    );
    let names = tools.call_names();
    assert_eq!(
        &names[names.len() - 2..],
        &["calibrator_off".to_owned(), "open_cover".to_owned()]
    );
}
