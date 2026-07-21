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
//!
//! The registry is persisted (rp.md § Session Persistence): every
//! transition — and, via [`SessionManager::persist_progress`], every
//! recorded exposure — rewrites the session state file atomically, and
//! every transition to `Idle` deletes it. On startup
//! [`SessionManager::recover_startup`] reads the file back: a live
//! session is restored (counters included) and the orchestrator is
//! re-invoked with `recovery.reason = "rp_restart"`. Persistence
//! failures are logged at `warn!`, never raised — bookkeeping must not
//! end an otherwise healthy night.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
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
        /// RFC 3339 wall-clock start, minted at `start()` and carried
        /// through interrupts and restarts into the state file.
        started_at: String,
    },
    /// A safety event interrupted the workflow; the ids are kept so the
    /// safe transition can re-invoke the orchestrator for the same
    /// session (its persisted state — e.g. session-runner's blackboard —
    /// is keyed by `session_id`).
    Interrupted {
        session_id: String,
        workflow_id: String,
        started_at: String,
    },
}

/// The `status` field of the persisted session state file.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum PersistedStatus {
    Active,
    Interrupted,
}

/// The on-disk shape of the session state file (rp.md § Session
/// Persistence): the registry plus the planner's progress counters.
/// An idle session has no file.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct PersistedSession {
    session_id: String,
    workflow_id: String,
    status: PersistedStatus,
    started_at: String,
    /// The serialized [`crate::planner::progress::SessionProgress`]
    /// store; kept as raw JSON here so persisting never needs to clone
    /// the store — it is serialized under its own lock. `null` when no
    /// progress store is wired (tests).
    #[serde(default)]
    progress: Value,
}

pub struct SessionManager {
    state: RwLock<SessionState>,
    event_bus: Arc<EventBus>,
    orchestrator_invoke_url: Option<String>,
    /// The orchestrator registration's `config` object — opaque to rp,
    /// passed through verbatim in the `/invoke` POST.
    orchestrator_config: Option<Value>,
    mcp_base_url: RwLock<String>,
    /// The planner's `record_exposure` counters, shared with
    /// `McpHandler` (see `lib.rs`). A fresh `start()` clears them — a
    /// new `session_id` is a new night — while the safety
    /// interrupt/resume path never passes through `start()`, so a
    /// resumed session keeps its progress. `None` in tests that don't
    /// exercise the planner.
    planner_progress: Option<Arc<std::sync::Mutex<crate::planner::progress::SessionProgress>>>,
    /// Where the session state file lives (rp.md § Session
    /// Persistence). `None` disables persistence entirely — tests that
    /// only exercise the state machine.
    state_path: Option<PathBuf>,
    /// Camera-cooling controller (rp.md § Camera Cooling): session
    /// start runs its cooldown pass, every transition to idle its
    /// warm-up ramp, and — only under safe conditions — startup
    /// recovery and safety resume its re-adopt path (no-actuation-on-
    /// connect tenet: an unsafe startup leaves the cooler untouched
    /// and defers to the resume path). `None` in tests that only
    /// exercise the state machine.
    cooling: Option<Arc<crate::cooling::CoolingController>>,
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
            planner_progress: None,
            state_path: None,
            cooling: None,
        }
    }

    /// Share the planner's `record_exposure` counters so `start()`
    /// can clear them when a fresh session begins.
    pub fn with_progress_store(
        mut self,
        store: Arc<std::sync::Mutex<crate::planner::progress::SessionProgress>>,
    ) -> Self {
        self.planner_progress = Some(store);
        self
    }

    /// Enable session-state persistence at the given path (rp.md
    /// § Session Persistence).
    pub fn with_state_path(mut self, path: PathBuf) -> Self {
        self.state_path = Some(path);
        self
    }

    /// Wire the camera-cooling controller so session transitions drive
    /// cooldown, warm-up, and recovery (rp.md § Camera Cooling).
    pub fn with_cooling(mut self, cooling: Arc<crate::cooling::CoolingController>) -> Self {
        self.cooling = Some(cooling);
        self
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
        let started_at = chrono::Utc::now().to_rfc3339();

        *state = SessionState::Active {
            session_id: session_id.clone(),
            workflow_id: workflow_id.clone(),
            started_at,
        };

        // A fresh session is a fresh night: reset the planner's
        // record_exposure counters *before* persisting, so the state
        // file starts the night at zero. The safety interrupt/resume
        // path re-invokes the orchestrator without passing through
        // here, so a resumed session keeps its progress.
        if let Some(progress) = &self.planner_progress {
            progress.lock().unwrap_or_else(|e| e.into_inner()).clear();
            debug!("planner progress counters cleared for the fresh session");
        }
        self.persist(&state).await;
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

        // Cooldown runs concurrently with the orchestrator — imaging
        // preparation is never blocked on thermal settling (rp.md
        // § Camera Cooling). A warm-up still ramping from the previous
        // session is cancelled and superseded.
        if let Some(cooling) = &self.cooling {
            cooling.start_cooldown();
        }

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
        let SessionState::Active {
            session_id,
            workflow_id,
            started_at,
        } = &*state
        else {
            return false;
        };
        debug!(session_id = %session_id, workflow_id = %workflow_id,
               "session interrupted by safety event");
        *state = SessionState::Interrupted {
            session_id: session_id.clone(),
            workflow_id: workflow_id.clone(),
            started_at: started_at.clone(),
        };
        self.persist(&state).await;
        true
    }

    /// Resume an interrupted session (safety safe transition): mark it
    /// active again and re-invoke the orchestrator with recovery
    /// context. Returns whether there was an interrupted session.
    pub async fn resume(self: &Arc<Self>) -> bool {
        let mut state = self.state.write().await;
        let SessionState::Interrupted {
            session_id,
            workflow_id,
            started_at,
        } = &*state
        else {
            return false;
        };
        let (session_id, workflow_id, started_at) =
            (session_id.clone(), workflow_id.clone(), started_at.clone());
        *state = SessionState::Active {
            session_id: session_id.clone(),
            workflow_id: workflow_id.clone(),
            started_at,
        };
        self.persist(&state).await;
        drop(state);

        // Re-adopt (or re-select) cooler rungs now that conditions are
        // safe: this is the deferred half of an unsafe-at-startup
        // restore (`recover_startup` skips it entirely to honor the
        // no-actuation-on-connect tenet), and a no-op re-adoption for an
        // ordinary live interruption, whose cooler was never touched
        // (rp.md § Camera Cooling → Recovery). Not a tenet violation:
        // this session was already operator-started before the outage
        // or safety event, so re-adopting on its unsafe -> safe
        // transition is automatic cleanup inside an operator-started
        // session, the same carve-out class as park-on-safety-transition
        // (workspace.md § Project Tenets, "No actuation on connect").
        if let Some(cooling) = &self.cooling {
            cooling.recover();
        }

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
        self.delete_state_file().await;
        drop(state);

        self.event_bus.emit(
            "session_stopped",
            serde_json::json!({
                "reason": "orchestrator_invoke_failed",
                "workflow_id": failed_workflow_id,
            }),
        );
        if let Some(cooling) = &self.cooling {
            cooling.start_warmup();
        }
    }

    pub async fn stop(&self) -> Result<(), String> {
        let mut state = self.state.write().await;
        *state = SessionState::Idle;
        self.delete_state_file().await;
        drop(state);

        debug!("session stopped");

        self.event_bus.emit(
            "session_stopped",
            serde_json::json!({
                "reason": "manual_stop",
            }),
        );

        // Every transition to idle ramps cooled cameras warm (a no-op
        // for cameras rp never commanded).
        if let Some(cooling) = &self.cooling {
            cooling.start_warmup();
        }

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
            self.delete_state_file().await;

            self.event_bus.emit(
                "session_stopped",
                serde_json::json!({
                    "reason": "workflow_complete",
                    "workflow_id": workflow_id,
                }),
            );
            if let Some(cooling) = &self.cooling {
                cooling.start_warmup();
            }
        } else {
            debug!(workflow_id = %workflow_id, "workflow_complete received but no matching active session");
        }
    }

    /// Re-persist the state file with the current planner counters.
    /// Called by the `record_exposure` tool after each recorded frame
    /// (rp.md § Write Strategy: at most one frame's progress is lost to
    /// a power failure). A no-op while idle — an idle session has no
    /// file — or when persistence is not configured.
    ///
    /// Takes the **write** lock despite not mutating: `RwLock` admits
    /// concurrent readers, so a read lock would let two overlapping
    /// `record_exposure` calls race `persist()` and land their atomic
    /// renames out of order — regressing the file to older counters
    /// (a repeated frame after a restart). The write lock upholds the
    /// writer-serialization invariant `persist()` documents.
    pub async fn persist_progress(&self) {
        let state = self.state.write().await;
        self.persist(&state).await;
    }

    /// Startup recovery (rp.md § Recovery Behavior): read the session
    /// state file back and, when a session was live, restore the
    /// registry and the planner's counters. Under safe conditions
    /// (`conditions_safe`, read from the `/mcp` gate after the safety
    /// poller's inline first pass — `BoundServer::start`) the
    /// orchestrator is re-invoked with `recovery.reason = "rp_restart"`;
    /// under unsafe ones the session is restored **interrupted** with
    /// no invocation, and the ordinary unsafe → safe machinery resumes
    /// it when conditions clear. Returns whether a session was
    /// restored. Called once, immediately before the server starts
    /// serving.
    ///
    /// The persisted status itself gates nothing — conditions may have
    /// flipped either way while rp was down, so the current poll, not
    /// the file, decides. An unreadable or corrupt file is never fatal:
    /// rp starts idle with a `warn!`, because refusing to start over
    /// unreadable bookkeeping would be worse than losing one resume.
    pub async fn recover_startup(self: &Arc<Self>, conditions_safe: bool) -> bool {
        let Some(path) = self.state_path.clone() else {
            return false;
        };
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!("no session state file; starting idle");
                return false;
            }
            Err(e) => {
                warn!(path = %path.display(), error = %e,
                      "cannot read the session state file; starting idle");
                return false;
            }
        };
        let persisted: PersistedSession = match serde_json::from_slice(&bytes) {
            Ok(persisted) => persisted,
            Err(e) => {
                warn!(path = %path.display(), error = %e,
                      "session state file is corrupt; starting idle");
                return false;
            }
        };

        // Restore the planner's counters first — the re-invoked
        // orchestrator's dispatch reads them immediately. The store is
        // assigned unconditionally: the file is the source of truth, so
        // missing (`null`) or unreadable persisted counters overwrite
        // whatever is in memory with a zeroed slate rather than
        // trusting the caller to have constructed the store empty.
        if let Some(store) = &self.planner_progress {
            let restored = if persisted.progress.is_null() {
                crate::planner::progress::SessionProgress::default()
            } else {
                serde_json::from_value(persisted.progress).unwrap_or_else(|e| {
                    warn!(error = %e,
                        "persisted progress counters are unreadable; resuming with zeroed counters");
                    crate::planner::progress::SessionProgress::default()
                })
            };
            *store.lock().unwrap_or_else(|e| e.into_inner()) = restored;
        }

        let restored_status = if conditions_safe {
            PersistedStatus::Active
        } else {
            PersistedStatus::Interrupted
        };
        let mut state = self.state.write().await;
        *state = match restored_status {
            PersistedStatus::Active => SessionState::Active {
                session_id: persisted.session_id.clone(),
                workflow_id: persisted.workflow_id.clone(),
                started_at: persisted.started_at.clone(),
            },
            PersistedStatus::Interrupted => SessionState::Interrupted {
                session_id: persisted.session_id.clone(),
                workflow_id: persisted.workflow_id.clone(),
                started_at: persisted.started_at.clone(),
            },
        };
        // Normalize the on-disk status to what was restored. When it
        // already matches, the rewrite would reproduce the bytes just
        // read — skip it (the pre-serve path should not pay a needless
        // fsync).
        if !matches!(
            (persisted.status, restored_status),
            (PersistedStatus::Active, PersistedStatus::Active)
                | (PersistedStatus::Interrupted, PersistedStatus::Interrupted)
        ) {
            self.persist(&state).await;
        }
        drop(state);

        if !conditions_safe {
            // No cooler actuation on an unsafe startup (no-actuation-on-
            // connect tenet, docs/workspace.md § Project Tenets): a
            // restored-as-interrupted session leaves the cooler
            // untouched here — `resume()` re-adopts (or re-selects) it
            // on the ordinary unsafe → safe transition instead.
            info!(session_id = %persisted.session_id, workflow_id = %persisted.workflow_id,
                  persisted_status = ?persisted.status,
                  "restored the persisted session as interrupted — conditions are unsafe; \
                   the orchestrator will be re-invoked on the safe transition");
            return true;
        }

        // Re-adopt (or re-select) cooler rungs for the restored session —
        // interrupted sessions included, since the cooler holds through
        // an interruption (rp.md § Camera Cooling → Recovery).
        if let Some(cooling) = &self.cooling {
            cooling.recover();
        }

        info!(session_id = %persisted.session_id, workflow_id = %persisted.workflow_id,
              persisted_status = ?persisted.status, started_at = %persisted.started_at,
              "restored the persisted session; re-invoking the orchestrator with recovery context");
        let recovery = serde_json::json!({ "reason": "rp_restart" });
        self.spawn_invoke(persisted.workflow_id, persisted.session_id, Some(recovery))
            .await;
        true
    }

    /// Serialize the given registry state + current counters and write
    /// the state file atomically (a no-op for `Idle` — an idle session
    /// has no file). Failures are logged at `warn!`, never raised —
    /// bookkeeping must not end an otherwise healthy night (rp.md
    /// § Write Strategy).
    ///
    /// Callers hold the state **write** lock (they pass the value they
    /// just stored in it) **across the write on purpose**: it serializes
    /// concurrent writers so the file can never regress to an older
    /// state, and it makes the delete-then-recreate race with `stop()`
    /// impossible (a `persist_progress` landing after a stop would
    /// otherwise resurrect a stale file that a later restart resumes).
    /// The fsync held under the lock is the accepted cost — transitions
    /// are rare and `record_exposure` runs at frame cadence.
    async fn persist(&self, state: &SessionState) {
        let Some(path) = self.state_path.clone() else {
            return;
        };
        let (session_id, workflow_id, status, started_at) = match state {
            SessionState::Active {
                session_id,
                workflow_id,
                started_at,
            } => (session_id, workflow_id, PersistedStatus::Active, started_at),
            SessionState::Interrupted {
                session_id,
                workflow_id,
                started_at,
            } => (
                session_id,
                workflow_id,
                PersistedStatus::Interrupted,
                started_at,
            ),
            SessionState::Idle => return,
        };
        let progress = match &self.planner_progress {
            Some(store) => {
                let store = store.lock().unwrap_or_else(|e| e.into_inner());
                serde_json::to_value(&*store).unwrap_or(Value::Null)
            }
            None => Value::Null,
        };
        let persisted = PersistedSession {
            session_id: session_id.clone(),
            workflow_id: workflow_id.clone(),
            status,
            started_at: started_at.clone(),
            progress,
        };
        let body = match serde_json::to_vec_pretty(&persisted) {
            Ok(body) => body,
            Err(e) => {
                warn!(error = %e, "cannot serialize the session state; skipping the write");
                return;
            }
        };
        let write_path = path.clone();
        let result =
            tokio::task::spawn_blocking(move || rp_fits::atomic::write_atomic(&write_path, &body))
                .await;
        match result {
            Ok(Ok(())) => debug!(path = %path.display(), "session state persisted"),
            Ok(Err(e)) => warn!(path = %path.display(), error = %e,
                                "failed to write the session state file; continuing"),
            Err(e) => warn!(error = %e, "session state write task failed; continuing"),
        }
    }

    /// Delete the state file — every transition to idle. Missing is
    /// fine (persistence may be disabled, or nothing was ever written).
    async fn delete_state_file(&self) {
        let Some(path) = &self.state_path else {
            return;
        };
        match tokio::fs::remove_file(path).await {
            Ok(()) => debug!(path = %path.display(), "session state file deleted"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!(path = %path.display(), error = %e,
                            "failed to delete the session state file"),
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
    use crate::cooling::test_support::{controller_for, stub_router, CoolerSim, Sim};
    use crate::equipment::test_support::spawn_stub;

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

    /// Polls `sim`'s `set_setpoint_calls` up to 5s for the background
    /// `cooling.recover()` task's actuation to land.
    async fn wait_for_setpoint_calls(sim: &Sim, expected: u32) -> bool {
        for _ in 0..100 {
            if sim.lock().unwrap().set_setpoint_calls >= expected {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    /// Asserts `sim` sees no cooler actuation for `window` — fails as soon
    /// as one is observed rather than only after the window elapses.
    async fn assert_no_setpoint_calls_within(sim: &Sim, window: Duration) {
        let deadline = tokio::time::Instant::now() + window;
        while tokio::time::Instant::now() < deadline {
            assert_eq!(
                sim.lock().unwrap().set_setpoint_calls,
                0,
                "cooler was actuated during the no-actuation window"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
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
    async fn a_fresh_start_clears_the_planner_progress_counters() {
        let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
        let event_bus = Arc::new(EventBus::from_config(&[]));
        let plugins = vec![json!({
            "name": "test-orchestrator",
            "type": "orchestrator",
            "invoke_url": stub.url,
        })];
        let progress = Arc::new(std::sync::Mutex::new(
            crate::planner::progress::SessionProgress::default(),
        ));
        progress.lock().unwrap().record("M31", Some("Red"));
        let manager = Arc::new(
            SessionManager::new(event_bus, &plugins).with_progress_store(progress.clone()),
        );

        manager.start().await.unwrap();

        assert_eq!(
            progress.lock().unwrap().completed_for("M31", Some("Red")),
            0,
            "a fresh session start must reset last night's counters"
        );
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

    // --- Session-state persistence (rp.md § Session Persistence) ---

    fn state_path(dir: &tempfile::TempDir) -> std::path::PathBuf {
        dir.path().join("session_state.json")
    }

    fn manager_with_state(
        invoke_url: &str,
        path: std::path::PathBuf,
    ) -> (
        Arc<SessionManager>,
        Arc<std::sync::Mutex<crate::planner::progress::SessionProgress>>,
    ) {
        let event_bus = Arc::new(EventBus::from_config(&[]));
        let plugins = vec![json!({
            "name": "test-orchestrator",
            "type": "orchestrator",
            "invoke_url": invoke_url,
            "config": {"workflow": "w"},
        })];
        let progress = Arc::new(std::sync::Mutex::new(
            crate::planner::progress::SessionProgress::default(),
        ));
        let manager = Arc::new(
            SessionManager::new(event_bus, &plugins)
                .with_progress_store(progress.clone())
                .with_state_path(path),
        );
        (manager, progress)
    }

    fn manager_with_cooling(
        invoke_url: &str,
        path: std::path::PathBuf,
        cooling: Arc<crate::cooling::CoolingController>,
    ) -> Arc<SessionManager> {
        let event_bus = Arc::new(EventBus::from_config(&[]));
        let plugins = vec![json!({
            "name": "test-orchestrator",
            "type": "orchestrator",
            "invoke_url": invoke_url,
            "config": {"workflow": "w"},
        })];
        Arc::new(
            SessionManager::new(event_bus, &plugins)
                .with_state_path(path)
                .with_cooling(cooling),
        )
    }

    fn read_state(path: &std::path::Path) -> Value {
        let bytes = std::fs::read(path).expect("no session state file");
        serde_json::from_slice(&bytes).expect("session state file is not JSON")
    }

    #[tokio::test]
    async fn start_persists_the_state_file_and_stop_deletes_it() {
        let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir);
        let (manager, _) = manager_with_state(&stub.url, path.clone());

        let response = manager.start().await.unwrap();
        let persisted = read_state(&path);
        assert_eq!(persisted["status"], "active");
        assert_eq!(persisted["session_id"], response["session_id"]);
        assert_eq!(persisted["workflow_id"], response["workflow_id"]);
        assert!(
            persisted["started_at"]
                .as_str()
                .is_some_and(|s| { chrono::DateTime::parse_from_rfc3339(s).is_ok() }),
            "started_at is not RFC 3339: {}",
            persisted["started_at"]
        );

        manager.stop().await.unwrap();
        assert!(!path.exists(), "stop must delete the session state file");
    }

    #[tokio::test]
    async fn interrupt_and_resume_rewrite_the_persisted_status() {
        let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir);
        let (manager, _) = manager_with_state(&stub.url, path.clone());
        manager.start().await.unwrap();
        let started_at = read_state(&path)["started_at"].clone();

        manager.interrupt().await;
        let persisted = read_state(&path);
        assert_eq!(persisted["status"], "interrupted");
        assert_eq!(
            persisted["started_at"], started_at,
            "the start time survives the interrupt"
        );

        manager.resume().await;
        assert_eq!(read_state(&path)["status"], "active");
    }

    #[tokio::test]
    async fn workflow_complete_deletes_the_state_file() {
        let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir);
        let (manager, _) = manager_with_state(&stub.url, path.clone());
        manager.start().await.unwrap();
        assert!(wait_for_hits(&stub, 1).await);
        let workflow_id = stub.bodies.read().await[0]["workflow_id"]
            .as_str()
            .unwrap()
            .to_owned();

        manager.workflow_complete(&workflow_id).await;
        assert!(
            !path.exists(),
            "workflow completion must delete the session state file"
        );
    }

    #[tokio::test]
    async fn a_failed_invocation_deletes_the_state_file() {
        let stub = spawn_invoke_stub(vec![StatusCode::BAD_REQUEST]).await;
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir);
        let (manager, _) = manager_with_state(&stub.url, path.clone());

        manager.start().await.unwrap();
        assert!(wait_for_status(&manager, "idle").await);
        assert!(
            !path.exists(),
            "the invoke-failure transition to idle must delete the state file"
        );
    }

    #[tokio::test]
    async fn persist_progress_rewrites_the_counters_and_is_a_noop_when_idle() {
        let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir);
        let (manager, progress) = manager_with_state(&stub.url, path.clone());

        // Idle: no session, no file — even with counters recorded.
        progress.lock().unwrap().record("M31", Some("Red"));
        manager.persist_progress().await;
        assert!(!path.exists(), "an idle session must have no state file");

        manager.start().await.unwrap();
        // start() cleared the counters; the persisted store is empty.
        assert_eq!(read_state(&path)["progress"]["completed"], json!({}));

        progress.lock().unwrap().record("M31", Some("Red"));
        progress.lock().unwrap().record("M31", Some("Red"));
        manager.persist_progress().await;
        let persisted = read_state(&path);
        assert_eq!(persisted["progress"]["completed"]["M31"]["Red"], 2);
        assert_eq!(persisted["progress"]["last_filter_key"], "Red");
    }

    #[tokio::test]
    async fn recover_startup_restores_the_session_and_reinvokes_with_rp_restart() {
        let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir);

        // First life: a session with two recorded frames, then a crash
        // (the manager is simply dropped — nothing deletes the file).
        let (first, progress) = manager_with_state(&stub.url, path.clone());
        first.start().await.unwrap();
        assert!(wait_for_hits(&stub, 1).await);
        progress.lock().unwrap().record("M31", Some("Red"));
        progress.lock().unwrap().record("M31", Some("Red"));
        first.persist_progress().await;
        drop(first);

        // Second life: a fresh manager over the same path.
        let (second, fresh_progress) = manager_with_state(&stub.url, path.clone());
        assert!(
            second.recover_startup(true).await,
            "no session was restored"
        );
        assert_eq!(second.status().await, "active");
        assert_eq!(
            fresh_progress
                .lock()
                .unwrap()
                .completed_for("M31", Some("Red")),
            2,
            "the planner counters must be restored from the state file"
        );

        assert!(wait_for_hits(&stub, 2).await, "no recovery re-invocation");
        let bodies = stub.bodies.read().await;
        assert_eq!(bodies[1]["recovery"], json!({"reason": "rp_restart"}));
        assert_eq!(bodies[1]["workflow_id"], bodies[0]["workflow_id"]);
        assert_eq!(bodies[1]["session_id"], bodies[0]["session_id"]);
        assert_eq!(bodies[1]["config"], json!({"workflow": "w"}));
    }

    #[tokio::test]
    async fn recover_startup_restores_an_interrupted_session_as_active() {
        let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir);
        let (first, _) = manager_with_state(&stub.url, path.clone());
        first.start().await.unwrap();
        first.interrupt().await;
        assert_eq!(read_state(&path)["status"], "interrupted");
        drop(first);

        let (second, _) = manager_with_state(&stub.url, path.clone());
        assert!(second.recover_startup(true).await);
        // Conditions are safe, so the poll — not the file — decides:
        // restored active and re-persisted as such.
        assert_eq!(second.status().await, "active");
        assert_eq!(read_state(&path)["status"], "active");
    }

    #[tokio::test]
    async fn recover_startup_under_unsafe_conditions_restores_interrupted_without_invoking() {
        let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir);
        let (first, _) = manager_with_state(&stub.url, path.clone());
        first.start().await.unwrap();
        assert!(wait_for_hits(&stub, 1).await);
        drop(first);

        let (second, _) = manager_with_state(&stub.url, path.clone());
        assert!(second.recover_startup(false).await);
        // Unsafe conditions: no re-invocation — the ordinary
        // unsafe → safe machinery resumes the session when they clear.
        assert_eq!(second.status().await, "interrupted");
        assert_eq!(read_state(&path)["status"], "interrupted");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            stub.hits.load(Ordering::SeqCst),
            1,
            "an unsafe restore must not re-invoke the orchestrator"
        );

        assert!(second.resume().await, "the safe transition resumes it");
        assert!(wait_for_hits(&stub, 2).await, "resume never re-invoked");
        assert_eq!(
            stub.bodies.read().await[1]["recovery"],
            json!({"reason": "safety_interruption"})
        );
    }

    #[tokio::test]
    async fn recover_startup_recovers_the_cooler_when_conditions_are_safe() {
        let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir);
        let (first, _) = manager_with_state(&stub.url, path.clone());
        first.start().await.unwrap();
        assert!(wait_for_hits(&stub, 1).await);
        drop(first);

        let sim: Sim = Arc::new(std::sync::Mutex::new(CoolerSim::new()));
        let cam_stub = spawn_stub(stub_router(sim.clone())).await;
        let (cooling, _rx) = controller_for(&cam_stub.url(), &[-10]).await;

        let second = manager_with_cooling(&stub.url, path.clone(), cooling);
        assert!(second.recover_startup(true).await);
        assert!(wait_for_hits(&stub, 2).await);

        assert!(
            wait_for_setpoint_calls(&sim, 1).await,
            "a safe restart must recover (command) the cooler"
        );
    }

    /// The no-actuation-on-connect tenet (docs/workspace.md § Project
    /// Tenets), applied to camera cooling: issue #636.
    #[tokio::test]
    async fn recover_startup_under_unsafe_conditions_defers_cooling_to_the_resume_path() {
        let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir);
        let (first, _) = manager_with_state(&stub.url, path.clone());
        first.start().await.unwrap();
        assert!(wait_for_hits(&stub, 1).await);
        drop(first);

        // The cooler is off — the driver-truth read in `run_recover`
        // would otherwise fall through to an actuating cooldown pass.
        let sim: Sim = Arc::new(std::sync::Mutex::new(CoolerSim::new()));
        let cam_stub = spawn_stub(stub_router(sim.clone())).await;
        let (cooling, _rx) = controller_for(&cam_stub.url(), &[-10]).await;

        let second = manager_with_cooling(&stub.url, path.clone(), cooling);
        assert!(second.recover_startup(false).await);
        assert_eq!(second.status().await, "interrupted");

        assert_no_setpoint_calls_within(&sim, Duration::from_millis(300)).await;

        assert!(second.resume().await, "the safe transition resumes it");
        assert!(
            wait_for_setpoint_calls(&sim, 1).await,
            "the deferred cooler recovery must run once conditions are safe"
        );
    }

    #[tokio::test]
    async fn recover_startup_is_a_noop_without_a_state_file() {
        let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
        let dir = tempfile::tempdir().unwrap();
        let (manager, _) = manager_with_state(&stub.url, state_path(&dir));

        assert!(!manager.recover_startup(true).await);
        assert_eq!(manager.status().await, "idle");
        assert_eq!(stub.hits.load(Ordering::SeqCst), 0, "nothing to re-invoke");
    }

    #[tokio::test]
    async fn recover_startup_with_a_corrupt_file_starts_idle() {
        let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir);
        std::fs::write(&path, b"{ not json").unwrap();
        let (manager, _) = manager_with_state(&stub.url, path.clone());

        assert!(!manager.recover_startup(true).await);
        assert_eq!(manager.status().await, "idle");
        assert_eq!(stub.hits.load(Ordering::SeqCst), 0);
        // The corrupt file is left in place for the operator; the next
        // session start overwrites it.
        assert!(path.exists());
    }

    #[tokio::test]
    async fn recover_startup_zeroes_stale_counters_when_progress_is_unreadable_or_absent() {
        // The log promises "resuming with zeroed counters" — a store
        // that already holds counts (a reused manager) must not leak
        // them into the recovered session.
        for progress in [json!("garbage"), Value::Null] {
            let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
            let dir = tempfile::tempdir().unwrap();
            let path = state_path(&dir);
            std::fs::write(
                &path,
                serde_json::to_vec(&json!({
                    "session_id": "s-1",
                    "workflow_id": "w-1",
                    "status": "active",
                    "started_at": "2026-07-11T00:00:00Z",
                    "progress": progress,
                }))
                .unwrap(),
            )
            .unwrap();

            let (manager, store) = manager_with_state(&stub.url, path.clone());
            store.lock().unwrap().record("M31", Some("Red"));

            assert!(manager.recover_startup(true).await);
            assert_eq!(
                store.lock().unwrap().completed_for("M31", Some("Red")),
                0,
                "progress {progress} must overwrite stale in-memory counters"
            );
        }
    }

    #[tokio::test]
    async fn a_manager_without_a_state_path_never_writes_a_file() {
        let stub = spawn_invoke_stub(vec![StatusCode::OK]).await;
        let manager = manager_for(&stub.url);
        manager.start().await.unwrap();
        manager.persist_progress().await;
        // Nothing observable to assert beyond "no panic" — the manager
        // has no path to write to; recover_startup is equally inert.
        assert!(!manager.recover_startup(true).await);
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
