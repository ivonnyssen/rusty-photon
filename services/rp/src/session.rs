use std::sync::Arc;

use serde_json::Value;
use tokio::sync::RwLock;
use tracing::debug;
use uuid::Uuid;

use crate::events::EventBus;

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub data_directory: String,
}

enum SessionState {
    Idle,
    Active {
        _session_id: String,
        workflow_id: String,
    },
}

pub struct SessionManager {
    state: RwLock<SessionState>,
    event_bus: Arc<EventBus>,
    orchestrator_invoke_url: Option<String>,
    mcp_base_url: RwLock<String>,
}

impl SessionManager {
    pub fn new(event_bus: Arc<EventBus>, plugins: &[Value]) -> Self {
        let orchestrator_invoke_url = plugins
            .iter()
            .find(|p| p.get("type").and_then(|v| v.as_str()) == Some("orchestrator"))
            .and_then(|p| p.get("invoke_url"))
            .and_then(|v| v.as_str())
            .map(String::from);

        debug!(orchestrator_url = ?orchestrator_invoke_url, "session manager initialized");

        Self {
            state: RwLock::new(SessionState::Idle),
            event_bus,
            orchestrator_invoke_url,
            mcp_base_url: RwLock::new(String::new()),
        }
    }

    pub async fn set_mcp_base_url(&self, url: String) {
        *self.mcp_base_url.write().await = url;
    }

    pub async fn start(&self) -> Result<Value, String> {
        let mut state = self.state.write().await;

        if matches!(*state, SessionState::Active { .. }) {
            return Err("a session is already active".to_string());
        }

        let session_id = Uuid::new_v4().to_string();
        let workflow_id = Uuid::new_v4().to_string();

        *state = SessionState::Active {
            _session_id: session_id.clone(),
            workflow_id: workflow_id.clone(),
        };

        debug!(session_id = %session_id, workflow_id = %workflow_id, "session started");

        // Emit session_started event
        self.event_bus.emit(
            "session_started",
            serde_json::json!({
                "session_id": session_id,
                "workflow_id": workflow_id,
            }),
        );

        // Invoke orchestrator if configured
        if let Some(invoke_url) = &self.orchestrator_invoke_url {
            let mcp_url = self.mcp_base_url.read().await.clone();
            let mcp_server_url = format!("{}/mcp", mcp_url);
            let invoke_url = invoke_url.clone();
            let wf_id = workflow_id.clone();
            let s_id = session_id.clone();

            tokio::spawn(async move {
                debug!(url = %invoke_url, "invoking orchestrator");
                let client = reqwest::Client::new();
                let body = serde_json::json!({
                    "workflow_id": wf_id,
                    "session_id": s_id,
                    "mcp_server_url": mcp_server_url,
                    "recovery": null,
                });
                match client.post(&invoke_url).json(&body).send().await {
                    Ok(resp) => {
                        debug!(status = %resp.status(), "orchestrator invoked");
                    }
                    Err(e) => {
                        debug!(error = %e, "failed to invoke orchestrator");
                    }
                }
            });
        }

        Ok(serde_json::json!({
            "session_id": session_id,
            "workflow_id": workflow_id,
        }))
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
        }
    }

    pub async fn workflow_complete(&self, workflow_id: &str) {
        let mut state = self.state.write().await;

        let matches = match &*state {
            SessionState::Active {
                workflow_id: wf_id, ..
            } => wf_id == workflow_id,
            _ => false,
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
