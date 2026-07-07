//! Session lifecycle: the state machine behind `/api/session/*` and the
//! orchestrator invocation protocol (rp.md § Orchestrator Invocation
//! Protocol, § Safety).
//!
//! A session is `Idle`, `Active`, or `Interrupted`. Starting a session
//! invokes the configured orchestrator plugin; a safety unsafe
//! transition moves an active session to `Interrupted` (the safety
//! enforcer also tears down the MCP transport — see `crate::safety`);
//! the safe transition re-invokes the orchestrator with recovery
//! context and the same ids. The invoke POST is retried on transport
//! errors and 5xx responses; a 4xx is permanent. When every attempt
//! fails the session returns to `Idle` and a `session_stopped` event
//! with `reason: "orchestrator_invoke_failed"` is emitted — a session
//! never sits active with an orchestrator that was never reached.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::events::EventBus;

/// Attempts made for one orchestrator invocation (initial or recovery).
const INVOKE_ATTEMPTS: u32 = 3;

/// Delay between invocation attempts. Short: the retry exists to ride
/// out an engine mid-restart (systemd brings it back in seconds), not
/// to wait out a long outage.
const INVOKE_RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub data_directory: String,
}

enum SessionState {
    Idle,
    Active {
        session_id: String,
        workflow_id: String,
    },
    /// A safety event interrupted the workflow; the ids are kept so the
    /// safe transition can re-invoke the orchestrator for the same
    /// session (its persisted state — e.g. session-runner's blackboard —
    /// is keyed by `session_id`).
    Interrupted {
        session_id: String,
        workflow_id: String,
    },
}

pub struct SessionManager {
    state: RwLock<SessionState>,
    event_bus: Arc<EventBus>,
    orchestrator_invoke_url: Option<String>,
    /// The orchestrator registration's `config` object — opaque to rp,
    /// passed through verbatim in the `/invoke` POST.
    orchestrator_config: Option<Value>,
    mcp_base_url: RwLock<String>,
}

impl SessionManager {
    pub fn new(event_bus: Arc<EventBus>, plugins: &[Value]) -> Self {
        let orchestrator = plugins
            .iter()
            .find(|p| p.get("type").and_then(|v| v.as_str()) == Some("orchestrator"));
        let orchestrator_invoke_url = orchestrator
            .and_then(|p| p.get("invoke_url"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let orchestrator_config = orchestrator.and_then(|p| p.get("config")).cloned();

        debug!(orchestrator_url = ?orchestrator_invoke_url, "session manager initialized");

        Self {
            state: RwLock::new(SessionState::Idle),
            event_bus,
            orchestrator_invoke_url,
            orchestrator_config,
            mcp_base_url: RwLock::new(String::new()),
        }
    }

    pub async fn set_mcp_base_url(&self, url: String) {
        *self.mcp_base_url.write().await = url;
    }

    pub async fn start(self: &Arc<Self>) -> Result<Value, String> {
        let mut state = self.state.write().await;

        match &*state {
            SessionState::Idle => {}
            SessionState::Active { .. } => {
                return Err("a session is already active".to_string());
            }
            SessionState::Interrupted { .. } => {
                return Err(
                    "a session is interrupted, awaiting safe conditions to resume".to_string(),
                );
            }
        }

        let session_id = Uuid::new_v4().to_string();
        let workflow_id = Uuid::new_v4().to_string();

        *state = SessionState::Active {
            session_id: session_id.clone(),
            workflow_id: workflow_id.clone(),
        };
        drop(state);

        debug!(session_id = %session_id, workflow_id = %workflow_id, "session started");

        self.event_bus.emit(
            "session_started",
            serde_json::json!({
                "session_id": session_id,
                "workflow_id": workflow_id,
            }),
        );

        self.spawn_invoke(workflow_id.clone(), session_id.clone(), None)
            .await;

        Ok(serde_json::json!({
            "session_id": session_id,
            "workflow_id": workflow_id,
        }))
    }

    /// Move an active session to `Interrupted` (safety unsafe
    /// transition). Returns whether there was an active session to
    /// interrupt. The MCP teardown happens in `crate::safety` — this is
    /// only the bookkeeping half.
    pub async fn interrupt(&self) -> bool {
        let mut state = self.state.write().await;
        match &*state {
            SessionState::Active {
                session_id,
                workflow_id,
            } => {
                debug!(session_id = %session_id, workflow_id = %workflow_id,
                       "session interrupted by safety event");
                *state = SessionState::Interrupted {
                    session_id: session_id.clone(),
                    workflow_id: workflow_id.clone(),
                };
                true
            }
            _ => false,
        }
    }

    /// Resume an interrupted session (safety safe transition): mark it
    /// active again and re-invoke the orchestrator with recovery
    /// context. Returns whether there was an interrupted session.
    pub async fn resume(self: &Arc<Self>) -> bool {
        let mut state = self.state.write().await;
        let SessionState::Interrupted {
            session_id,
            workflow_id,
        } = &*state
        else {
            return false;
        };
        let (session_id, workflow_id) = (session_id.clone(), workflow_id.clone());
        *state = SessionState::Active {
            session_id: session_id.clone(),
            workflow_id: workflow_id.clone(),
        };
        drop(state);

        debug!(session_id = %session_id, workflow_id = %workflow_id,
               "conditions safe again; re-invoking the orchestrator with recovery context");
        let recovery = serde_json::json!({ "reason": "safety_interruption" });
        self.spawn_invoke(workflow_id, session_id, Some(recovery))
            .await;
        true
    }

    /// POST the orchestrator invocation in the background, retrying per
    /// the protocol (rp.md § Orchestrator Invocation Protocol). No-op
    /// when no orchestrator is configured.
    async fn spawn_invoke(
        self: &Arc<Self>,
        workflow_id: String,
        session_id: String,
        recovery: Option<Value>,
    ) {
        let Some(invoke_url) = self.orchestrator_invoke_url.clone() else {
            return;
        };
        let mcp_url = self.mcp_base_url.read().await.clone();
        let body = serde_json::json!({
            "workflow_id": workflow_id,
            "session_id": session_id,
            "mcp_server_url": format!("{}/mcp", mcp_url),
            "recovery": recovery,
            "config": self.orchestrator_config.clone(),
        });

        let manager = Arc::clone(self);
        tokio::spawn(async move {
            if !invoke_with_retry(&invoke_url, &body).await {
                manager.fail_invoke(&workflow_id).await;
            }
        });
    }

    /// Every invocation attempt failed: return the session to idle so
    /// the operator can see the failure and start over, unless the
    /// session has already moved on (completed, stopped, restarted).
    async fn fail_invoke(&self, failed_workflow_id: &str) {
        let mut state = self.state.write().await;
        let matches = match &*state {
            SessionState::Active { workflow_id, .. }
            | SessionState::Interrupted { workflow_id, .. } => workflow_id == failed_workflow_id,
            SessionState::Idle => false,
        };
        if !matches {
            debug!(workflow_id = %failed_workflow_id,
                   "invoke failure for a session that already moved on; ignoring");
            return;
        }
        warn!(workflow_id = %failed_workflow_id,
              "orchestrator could not be invoked; session returns to idle");
        *state = SessionState::Idle;
        drop(state);

        self.event_bus.emit(
            "session_stopped",
            serde_json::json!({
                "reason": "orchestrator_invoke_failed",
                "workflow_id": failed_workflow_id,
            }),
        );
    }

    pub async fn stop(&self) -> Result<(), String> {
        let mut state = self.state.write().await;
        *state = SessionState::Idle;

        debug!("session stopped");

        self.event_bus.emit(
            "session_stopped",
            serde_json::json!({
                "reason": "manual_stop",
            }),
        );

        Ok(())
    }

    pub async fn status(&self) -> String {
        let state = self.state.read().await;
        match *state {
            SessionState::Idle => "idle".to_string(),
            SessionState::Active { .. } => "active".to_string(),
            SessionState::Interrupted { .. } => "interrupted".to_string(),
        }
    }

    pub async fn workflow_complete(&self, workflow_id: &str) {
        let mut state = self.state.write().await;

        // An interrupted session can complete too: the engine may post
        // completion in the same instant the safety monitor turns
        // unsafe — the completion wins, there is nothing left to resume.
        let matches = match &*state {
            SessionState::Active {
                workflow_id: wf_id, ..
            }
            | SessionState::Interrupted {
                workflow_id: wf_id, ..
            } => wf_id == workflow_id,
            SessionState::Idle => false,
        };

        if matches {
            debug!(workflow_id = %workflow_id, "workflow completed, session ending");
            *state = SessionState::Idle;

            self.event_bus.emit(
                "session_stopped",
                serde_json::json!({
                    "reason": "workflow_complete",
                    "workflow_id": workflow_id,
                }),
            );
        } else {
            debug!(workflow_id = %workflow_id, "workflow_complete received but no matching active session");
        }
    }
}

/// POST `body` to the orchestrator's invoke URL, retrying transient
/// failures (transport errors, 5xx) up to [`INVOKE_ATTEMPTS`] times with
/// [`INVOKE_RETRY_DELAY`] between attempts. A 4xx response is permanent —
/// the same request will fail the same way. Returns whether an attempt
/// was acknowledged with a success status.
async fn invoke_with_retry(invoke_url: &str, body: &Value) -> bool {
    let client = reqwest::Client::new();
    for attempt in 1..=INVOKE_ATTEMPTS {
        debug!(url = %invoke_url, attempt, "invoking orchestrator");
        match client.post(invoke_url).json(body).send().await {
            Ok(resp) if resp.status().is_success() => {
                debug!(status = %resp.status(), attempt, "orchestrator invoked");
                return true;
            }
            Ok(resp) if resp.status().is_client_error() => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                warn!(%status, %body, "orchestrator rejected the invocation; not retrying");
                return false;
            }
            Ok(resp) => {
                warn!(status = %resp.status(), attempt, max = INVOKE_ATTEMPTS,
                      "orchestrator invocation failed");
            }
            Err(e) => {
                warn!(error = %e, attempt, max = INVOKE_ATTEMPTS,
                      "failed to reach the orchestrator");
            }
        }
        if attempt < INVOKE_ATTEMPTS {
            tokio::time::sleep(INVOKE_RETRY_DELAY).await;
        }
    }
    false
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::Json;
    use axum::Router;
    use serde_json::json;

    use super::*;

    /// In-process orchestrator stub: records every `/invoke` body and
    /// answers with the scripted status sequence (last entry repeats).
    struct InvokeStub {
        url: String,
        bodies: Arc<RwLock<Vec<Value>>>,
        hits: Arc<AtomicU32>,
        _shutdown: tokio::sync::oneshot::Sender<()>,
    }

    async fn spawn_invoke_stub(statuses: Vec<StatusCode>) -> InvokeStub {
        #[derive(Clone)]
        struct StubState {
            bodies: Arc<RwLock<Vec<Value>>>,
            hits: Arc<AtomicU32>,
            statuses: Arc<Vec<StatusCode>>,
        }
        let state = StubState {
            bodies: Arc::new(RwLock::new(Vec::new())),
            hits: Arc::new(AtomicU32::new(0)),
            statuses: Arc::new(statuses),
        };
        let app = Router::new()
            .route(
                "/invoke",
                post(
                    |State(state): State<StubState>, Json(body): Json<Value>| async move {
                        state.bodies.write().await.push(body);
                        let n = state.hits.fetch_add(1, Ordering::SeqCst) as usize;
                        let status = *state
                            .statuses
                            .get(n)
                            .or(state.statuses.last())
                            .unwrap_or(&StatusCode::OK);
                        (
                            status,
                            Json(json!({"estimated_duration": "1s", "max_duration": "0s"})),
                        )
                    },
                ),
            )
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await
                .unwrap();
        });
        InvokeStub {
            url: format!("http://127.0.0.1:{port}/invoke"),
            bodies: state.bodies,
            hits: state.hits,
            _shutdown: tx,
        }
    }

    fn manager_for(invoke_url: &str) -> Arc<SessionManager> {
        let event_bus = Arc::new(EventBus::from_config(&[]));
        let plugins = vec![json!({
            "name": "test-orchestrator",
            "type": "orchestrator",
            "invoke_url": invoke_url,
            "config": {"workflow": "w"},
        })];
        Arc::new(SessionManager::new(event_bus, &plugins))
    }

    // 300 × 50ms = 15s. Generous because the unreachable-orchestrator test
    // sits through the full retry schedule first, and on Windows a refused
    // localhost connect is not instant — WinSock retries SYNs for about a
    // second before reporting `WSAECONNREFUSED`, so three attempts plus two
    // 1s backoffs already burn ~5s there.
    async fn wait_for_status(manager: &Arc<SessionManager>, expected: &str) -> bool {
        for _ in 0..300 {
            if manager.status().await == expected {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    async fn wait_for_hits(stub: &InvokeStub, expected: u32) -> bool {
        for _ in 0..100 {
            if stub.hits.load(Ordering::SeqCst) >= expected {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test]
    async fn start_invokes_orchestrator_with_null_recovery() {
        let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
        let manager = manager_for(&stub.url);

        let response = manager.start().await.unwrap();
        assert!(response.get("session_id").is_some());
        assert!(wait_for_hits(&stub, 1).await, "orchestrator never invoked");

        let bodies = stub.bodies.read().await;
        assert!(bodies[0]["recovery"].is_null());
        assert_eq!(bodies[0]["config"], json!({"workflow": "w"}));
        drop(bodies);
        assert_eq!(manager.status().await, "active");
    }

    #[tokio::test]
    async fn invoke_retries_a_5xx_then_succeeds() {
        let stub = spawn_invoke_stub(vec![StatusCode::INTERNAL_SERVER_ERROR, StatusCode::OK]).await;
        let manager = manager_for(&stub.url);

        manager.start().await.unwrap();
        assert!(wait_for_hits(&stub, 2).await, "no retry after the 5xx");
        // The session must remain active: the retry succeeded.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(manager.status().await, "active");
    }

    #[tokio::test]
    async fn invoke_gives_up_immediately_on_4xx() {
        let stub = spawn_invoke_stub(vec![StatusCode::BAD_REQUEST]).await;
        let manager = manager_for(&stub.url);
        let mut events = manager.event_bus.subscribe();

        manager.start().await.unwrap();
        assert!(
            wait_for_status(&manager, "idle").await,
            "session did not return to idle after the permanent invoke failure"
        );
        assert_eq!(
            stub.hits.load(Ordering::SeqCst),
            1,
            "a 4xx must not be retried"
        );

        // session_started, then session_stopped with the failure reason.
        let started = events.recv().await.unwrap();
        assert_eq!(started.event, "session_started");
        let stopped = events.recv().await.unwrap();
        assert_eq!(stopped.event, "session_stopped");
        assert_eq!(stopped.payload["reason"], "orchestrator_invoke_failed");
    }

    #[tokio::test]
    async fn invoke_unreachable_exhausts_retries_and_returns_to_idle() {
        // Bind a port then drop the listener: connects are refused.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let manager = manager_for(&format!("http://127.0.0.1:{port}/invoke"));

        manager.start().await.unwrap();
        assert!(
            wait_for_status(&manager, "idle").await,
            "session did not return to idle with an unreachable orchestrator"
        );
    }

    #[tokio::test]
    async fn interrupt_then_resume_reinvokes_with_recovery_and_same_ids() {
        let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
        let manager = manager_for(&stub.url);

        manager.start().await.unwrap();
        assert!(wait_for_hits(&stub, 1).await);

        assert!(manager.interrupt().await);
        assert_eq!(manager.status().await, "interrupted");

        assert!(manager.resume().await);
        assert_eq!(manager.status().await, "active");
        assert!(wait_for_hits(&stub, 2).await, "resume never re-invoked");

        let bodies = stub.bodies.read().await;
        assert_eq!(
            bodies[1]["recovery"],
            json!({"reason": "safety_interruption"})
        );
        assert_eq!(bodies[1]["workflow_id"], bodies[0]["workflow_id"]);
        assert_eq!(bodies[1]["session_id"], bodies[0]["session_id"]);
    }

    #[tokio::test]
    async fn start_is_refused_while_active_and_while_interrupted() {
        let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
        let manager = manager_for(&stub.url);
        manager.start().await.unwrap();

        let err = manager.start().await.unwrap_err();
        assert!(err.contains("already active"), "got: {err}");

        manager.interrupt().await;
        // The refusal must name the interrupted state, not claim the
        // session is active — the operator would otherwise look for a
        // running workflow that isn't there.
        let err = manager.start().await.unwrap_err();
        assert!(err.contains("interrupted"), "got: {err}");
    }

    #[tokio::test]
    async fn workflow_complete_ends_an_interrupted_session() {
        let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
        let manager = manager_for(&stub.url);
        manager.start().await.unwrap();
        assert!(wait_for_hits(&stub, 1).await);
        let workflow_id = stub.bodies.read().await[0]["workflow_id"]
            .as_str()
            .unwrap()
            .to_owned();

        manager.interrupt().await;
        manager.workflow_complete(&workflow_id).await;
        assert_eq!(manager.status().await, "idle");
        // Nothing left to resume.
        assert!(!manager.resume().await);
    }

    #[tokio::test]
    async fn interrupt_and_resume_are_noops_when_idle() {
        let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
        let manager = manager_for(&stub.url);
        assert!(!manager.interrupt().await);
        assert!(!manager.resume().await);
        assert_eq!(manager.status().await, "idle");
    }

    #[tokio::test]
    async fn invoke_failure_for_a_moved_on_session_is_ignored() {
        let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
        let manager = manager_for(&stub.url);
        manager.start().await.unwrap();
        assert!(wait_for_hits(&stub, 1).await);

        // A stale failure from a previous workflow must not clobber the
        // current session.
        manager.fail_invoke("some-older-workflow").await;
        assert_eq!(manager.status().await, "active");

        // Nor may a late failure resurrect activity once the session is
        // over: fail_invoke on an idle manager stays idle.
        let workflow_id = stub.bodies.read().await[0]["workflow_id"]
            .as_str()
            .unwrap()
            .to_owned();
        manager.stop().await.unwrap();
        manager.fail_invoke(&workflow_id).await;
        assert_eq!(manager.status().await, "idle");
    }
}
