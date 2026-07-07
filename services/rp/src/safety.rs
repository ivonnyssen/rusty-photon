//! Safety enforcement (rp.md § Safety): poll every configured ASCOM
//! SafetyMonitor, and on the overall safe → unsafe transition gate the
//! `/mcp` endpoint, terminate all open MCP sessions (cancelling in-flight
//! tool calls), interrupt the active session, and abort in-progress
//! exposures. On unsafe → safe, lift the gate and resume the interrupted
//! session by re-invoking the orchestrator with recovery context.
//!
//! Readings are **fail-unsafe**: a monitor that is disconnected or
//! errors counts as unsafe, and conditions are safe only while *all*
//! monitors report safe. Every per-monitor change emits a
//! `safety_changed` event; the assumed baseline is safe, so a monitor
//! that starts out safe emits nothing at startup while one that starts
//! out unsafe announces itself.
//!
//! Not yet implemented on unsafe: stopping guiding and parking the
//! mount (no guider integration in rp yet; parking lands with it).

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::equipment::EquipmentRegistry;
use crate::events::EventBus;
use crate::session::SessionManager;

/// One pollable safety source. A seam over the Alpaca device so the
/// polling loop is unit-testable with scripted probes.
pub trait SafetyProbe: Send + Sync {
    fn id(&self) -> &str;
    fn is_safe(&self) -> impl Future<Output = Result<bool, String>> + Send;
}

/// Production probe over a connected (or not) ASCOM Alpaca SafetyMonitor.
pub struct AlpacaSafetyProbe {
    id: String,
    device: Option<Arc<dyn ascom_alpaca::api::SafetyMonitor>>,
}

impl SafetyProbe for AlpacaSafetyProbe {
    fn id(&self) -> &str {
        &self.id
    }

    async fn is_safe(&self) -> Result<bool, String> {
        let Some(device) = &self.device else {
            return Err("safety monitor is not connected".to_string());
        };
        device.is_safe().await.map_err(|e| e.to_string())
    }
}

/// The polling loop plus everything it drives on a transition.
pub struct SafetyEnforcer<P: SafetyProbe> {
    probes: Vec<P>,
    poll_interval: Duration,
    event_bus: Arc<EventBus>,
    session: Arc<SessionManager>,
    mcp_sessions: Arc<LocalSessionManager>,
    equipment: Arc<EquipmentRegistry>,
    /// Read by the `/mcp` gate in `routes`: `false` rejects every MCP
    /// request with 503 while conditions are unsafe.
    safety_ok: Arc<AtomicBool>,
}

impl SafetyEnforcer<AlpacaSafetyProbe> {
    /// Build the enforcer over the registry's connected safety monitors.
    /// Returns `None` when none are configured — the loop never starts
    /// and sessions run ungated.
    #[allow(clippy::too_many_arguments)]
    pub fn from_registry(
        equipment: Arc<EquipmentRegistry>,
        event_bus: Arc<EventBus>,
        session: Arc<SessionManager>,
        mcp_sessions: Arc<LocalSessionManager>,
        safety_ok: Arc<AtomicBool>,
        poll_interval: Duration,
    ) -> Option<Self> {
        if equipment.safety_monitors.is_empty() {
            debug!("no safety monitors configured; safety polling disabled");
            return None;
        }
        let probes = equipment
            .safety_monitors
            .iter()
            .map(|entry| AlpacaSafetyProbe {
                id: entry.id.clone(),
                device: entry.device.clone(),
            })
            .collect();
        Some(Self {
            probes,
            poll_interval,
            event_bus,
            session,
            mcp_sessions,
            equipment,
            safety_ok,
        })
    }
}

impl<P: SafetyProbe> SafetyEnforcer<P> {
    /// Poll until cancelled (rp shutdown).
    pub async fn run(self, cancel: CancellationToken) {
        info!(
            monitors = self.probes.len(),
            interval = ?self.poll_interval,
            "safety monitoring started"
        );
        // Assumed-safe baselines: transitions are relative to these, so
        // a monitor that starts out safe is quiet and one that starts
        // out unsafe announces itself on the first poll.
        let mut per_monitor: HashMap<String, bool> = HashMap::new();
        let mut overall = true;
        loop {
            tokio::select! {
                () = cancel.cancelled() => {
                    debug!("safety monitoring stopped");
                    return;
                }
                () = tokio::time::sleep(self.poll_interval) => {}
            }
            overall = self.poll_once(&mut per_monitor, overall).await;
        }
    }

    /// One polling pass: read every probe, emit per-monitor
    /// `safety_changed` events, and act when the overall state flips.
    /// Returns the new overall state.
    async fn poll_once(&self, per_monitor: &mut HashMap<String, bool>, prev_overall: bool) -> bool {
        let mut overall = true;
        for probe in &self.probes {
            let reading = match probe.is_safe().await {
                Ok(reading) => reading,
                Err(e) => {
                    warn!(monitor = probe.id(), error = %e,
                          "safety monitor read failed; treating as unsafe");
                    false
                }
            };
            overall &= reading;
            let prev = per_monitor
                .insert(probe.id().to_owned(), reading)
                .unwrap_or(true);
            if prev != reading {
                let new_state = if reading { "safe" } else { "unsafe" };
                debug!(monitor = probe.id(), new_state, "safety monitor transition");
                self.event_bus.emit(
                    "safety_changed",
                    serde_json::json!({
                        "monitor": probe.id(),
                        "new_state": new_state,
                    }),
                );
            }
        }
        if overall != prev_overall {
            if overall {
                self.on_safe().await;
            } else {
                self.on_unsafe().await;
            }
        }
        overall
    }

    /// Overall safe → unsafe: gate first so nothing new gets in, then
    /// tear down the workflow's transport and stop the hardware.
    async fn on_unsafe(&self) {
        warn!("conditions unsafe; cancelling the active workflow");
        self.safety_ok.store(false, Ordering::SeqCst);
        let interrupted = self.session.interrupt().await;
        close_all_mcp_sessions(&self.mcp_sessions).await;
        abort_exposures(&self.equipment).await;
        if interrupted {
            info!("session interrupted; awaiting safe conditions to resume");
        }
    }

    /// Overall unsafe → safe: lift the gate before re-invoking, so the
    /// orchestrator's immediate MCP connect isn't rejected.
    async fn on_safe(&self) {
        info!("conditions safe again");
        self.safety_ok.store(true, Ordering::SeqCst);
        if self.session.resume().await {
            info!("session resumed; orchestrator re-invoked with recovery context");
        }
    }
}

/// Close every open MCP session, cancelling in-flight tool calls; the
/// orchestrator's next call on a closed session surfaces as a terminated
/// session (the engine exits without completion and keeps its state).
async fn close_all_mcp_sessions(manager: &LocalSessionManager) {
    let handles: Vec<_> = manager.sessions.write().await.drain().collect();
    for (id, handle) in handles {
        debug!(mcp_session = %id, "terminating MCP session");
        if let Err(e) = handle.close().await {
            // The worker may already be gone; nothing to enforce then.
            debug!(mcp_session = %id, error = %e, "MCP session close reported an error");
        }
    }
}

/// Best-effort `AbortExposure` on every connected camera — the tool task
/// driving an exposure was just cancelled with its MCP session, so the
/// camera would otherwise keep exposing into the (unsafe) night.
async fn abort_exposures(equipment: &EquipmentRegistry) {
    for camera in &equipment.cameras {
        let Some(device) = &camera.device else {
            continue;
        };
        match device.abort_exposure().await {
            Ok(()) => debug!(camera = %camera.id, "aborted in-progress exposure"),
            Err(e) => {
                // Usually just "no exposure in progress" — worth a debug
                // line, not an operator-facing warning.
                debug!(camera = %camera.id, error = %e, "abort_exposure failed");
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use std::sync::Mutex;

    use rmcp::transport::streamable_http_server::session::SessionManager as _;

    use super::*;

    /// Probe whose readings are scripted: pops the front of the queue,
    /// repeating the last entry once drained.
    struct ScriptedProbe {
        id: String,
        readings: Mutex<Vec<Result<bool, String>>>,
    }

    impl ScriptedProbe {
        fn new(id: &str, readings: Vec<Result<bool, String>>) -> Self {
            Self {
                id: id.to_string(),
                readings: Mutex::new(readings),
            }
        }
    }

    impl SafetyProbe for ScriptedProbe {
        fn id(&self) -> &str {
            &self.id
        }

        async fn is_safe(&self) -> Result<bool, String> {
            let mut readings = self.readings.lock().unwrap();
            if readings.len() > 1 {
                readings.remove(0)
            } else {
                readings[0].clone()
            }
        }
    }

    fn empty_registry() -> Arc<EquipmentRegistry> {
        Arc::new(EquipmentRegistry {
            cameras: vec![],
            filter_wheels: vec![],
            cover_calibrators: vec![],
            focusers: vec![],
            safety_monitors: vec![],
            mount: None,
        })
    }

    fn enforcer_with(probes: Vec<ScriptedProbe>) -> SafetyEnforcer<ScriptedProbe> {
        let event_bus = Arc::new(EventBus::from_config(&[]));
        let session = Arc::new(SessionManager::new(event_bus.clone(), &[]));
        SafetyEnforcer {
            probes,
            poll_interval: Duration::from_millis(1),
            event_bus,
            session,
            mcp_sessions: Arc::new(LocalSessionManager::default()),
            equipment: empty_registry(),
            safety_ok: Arc::new(AtomicBool::new(true)),
        }
    }

    #[tokio::test]
    async fn quiet_when_monitors_start_out_safe() {
        let enforcer = enforcer_with(vec![ScriptedProbe::new("sm", vec![Ok(true)])]);
        let mut events = enforcer.event_bus.subscribe();
        let mut state = HashMap::new();

        let overall = enforcer.poll_once(&mut state, true).await;
        assert!(overall);
        assert!(enforcer.safety_ok.load(Ordering::SeqCst));
        assert!(
            events.try_recv().is_err(),
            "a safe first reading matches the assumed baseline; no event expected"
        );
    }

    #[tokio::test]
    async fn unsafe_transition_gates_interrupts_and_terminates_mcp_sessions() {
        let enforcer = enforcer_with(vec![ScriptedProbe::new("sm", vec![Ok(false)])]);
        let mut events = enforcer.event_bus.subscribe();
        enforcer.session.start().await.unwrap();

        // Two live MCP sessions that must be torn down.
        enforcer.mcp_sessions.create_session().await.unwrap();
        enforcer.mcp_sessions.create_session().await.unwrap();

        let mut state = HashMap::new();
        let overall = enforcer.poll_once(&mut state, true).await;

        assert!(!overall);
        assert!(
            !enforcer.safety_ok.load(Ordering::SeqCst),
            "gate must close"
        );
        assert_eq!(enforcer.session.status().await, "interrupted");
        assert!(
            enforcer.mcp_sessions.sessions.read().await.is_empty(),
            "all MCP sessions must be terminated"
        );

        // session_started (from start), then the safety transition.
        assert_eq!(events.recv().await.unwrap().event, "session_started");
        let changed = events.recv().await.unwrap();
        assert_eq!(changed.event, "safety_changed");
        assert_eq!(changed.payload["monitor"], "sm");
        assert_eq!(changed.payload["new_state"], "unsafe");
    }

    #[tokio::test]
    async fn read_errors_count_as_unsafe() {
        let enforcer = enforcer_with(vec![ScriptedProbe::new(
            "sm",
            vec![Err("boom".to_string())],
        )]);
        let mut events = enforcer.event_bus.subscribe();

        let mut state = HashMap::new();
        let overall = enforcer.poll_once(&mut state, true).await;

        assert!(!overall, "a failed read must be treated as unsafe");
        let changed = events.recv().await.unwrap();
        assert_eq!(changed.payload["new_state"], "unsafe");
    }

    #[tokio::test]
    async fn safe_transition_lifts_gate_and_resumes_the_session() {
        let enforcer = enforcer_with(vec![ScriptedProbe::new("sm", vec![Ok(false), Ok(true)])]);
        let mut events = enforcer.event_bus.subscribe();
        enforcer.session.start().await.unwrap();

        let mut state = HashMap::new();
        let overall = enforcer.poll_once(&mut state, true).await;
        assert!(!overall);
        assert_eq!(enforcer.session.status().await, "interrupted");

        let overall = enforcer.poll_once(&mut state, overall).await;
        assert!(overall);
        assert!(enforcer.safety_ok.load(Ordering::SeqCst), "gate must lift");
        assert_eq!(enforcer.session.status().await, "active");

        assert_eq!(events.recv().await.unwrap().event, "session_started");
        assert_eq!(events.recv().await.unwrap().payload["new_state"], "unsafe");
        assert_eq!(events.recv().await.unwrap().payload["new_state"], "safe");
    }

    #[tokio::test]
    async fn any_unsafe_monitor_makes_the_overall_state_unsafe() {
        let enforcer = enforcer_with(vec![
            ScriptedProbe::new("one", vec![Ok(true)]),
            ScriptedProbe::new("two", vec![Ok(false)]),
        ]);
        let mut events = enforcer.event_bus.subscribe();

        let mut state = HashMap::new();
        let overall = enforcer.poll_once(&mut state, true).await;

        assert!(!overall);
        // Only the flipping monitor emits.
        let changed = events.recv().await.unwrap();
        assert_eq!(changed.payload["monitor"], "two");
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn unsafe_without_a_session_still_gates() {
        let enforcer = enforcer_with(vec![ScriptedProbe::new("sm", vec![Ok(false)])]);
        let mut state = HashMap::new();

        enforcer.poll_once(&mut state, true).await;

        assert!(!enforcer.safety_ok.load(Ordering::SeqCst));
        assert_eq!(enforcer.session.status().await, "idle");
    }

    #[tokio::test]
    async fn run_stops_on_cancellation() {
        let enforcer = enforcer_with(vec![ScriptedProbe::new("sm", vec![Ok(true)])]);
        let cancel = CancellationToken::new();
        let task = tokio::spawn(enforcer.run(cancel.clone()));

        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("run() did not stop on cancellation")
            .unwrap();
    }

    #[tokio::test]
    async fn from_registry_is_none_without_monitors() {
        let event_bus = Arc::new(EventBus::from_config(&[]));
        let session = Arc::new(SessionManager::new(event_bus.clone(), &[]));
        let enforcer = SafetyEnforcer::from_registry(
            empty_registry(),
            event_bus,
            session,
            Arc::new(LocalSessionManager::default()),
            Arc::new(AtomicBool::new(true)),
            Duration::from_secs(10),
        );
        assert!(enforcer.is_none());
    }
}
