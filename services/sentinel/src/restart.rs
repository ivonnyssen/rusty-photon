//! The Service Restart API's engine: `POST /api/services/{name}/restart`.
//!
//! Sentinel owns *process* restart for the rest of the stack (drivers reload
//! themselves via `config.apply`; sentinel restarts processes). For each entry
//! in the top-level `services` map the [`RestartManager`] runs the configured
//! `restart_command` through the platform shell and — when a `health_command`
//! is configured — polls it (exit 0 = healthy) until it succeeds or the
//! service's `max_restart_duration` budget elapses. The command's outcome is a
//! *domain* result ([`RestartReport`], HTTP 200 either way); addressing errors
//! ([`RestartError`]) map to 4xx. See `docs/services/sentinel.md`
//! §Service Restart API.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::config::ServiceConfig;
use crate::corrective::{Restarter, ShellRestarter};

/// How many times recovery re-runs `health_command` before giving up. The
/// total wait is bounded by the service's `max_restart_duration` regardless.
const RECOVERY_ATTEMPTS: u32 = 5;

/// Addressing errors — the dashboard handler maps these to 4xx.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RestartError {
    /// `{name}` is not in the `services` map (404).
    #[error("no configured service named '{0}'")]
    UnknownService(String),
    /// The service is configured `restart_command: null` (409).
    #[error("service '{0}' is not restartable")]
    NotRestartable(String),
    /// Another restart of this service is still running (409).
    #[error("a restart of '{0}' is already in flight")]
    AlreadyInFlight(String),
}

/// Recovery-confirmation outcome, reported in the response body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Recovery {
    /// `health_command` exited 0 within the remaining budget.
    Healthy,
    /// `health_command` never exited 0 before the budget elapsed.
    Timeout,
    /// No `health_command` is configured — confirmation skipped.
    Skipped,
}

/// Domain outcome of a restart request. `status:"failed"` is still HTTP 200 —
/// the *action* ran and this is its result (mirroring `config.apply`'s
/// `status:"invalid"` convention).
#[derive(Debug, Clone, Serialize)]
pub struct RestartReport {
    pub service: String,
    /// `"ok"` (command exited 0) or `"failed"`.
    pub status: &'static str,
    /// Present on `"ok"`: whether recovery was confirmed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<Recovery>,
    /// Present on `"failed"`: what went wrong with the command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// One-restart-per-service gate, shared by every restart path — the REST
/// endpoint, the watchdog corrective ladder, and health supervision. Clones
/// share the same in-flight set, so whichever path acquires a service's slot
/// first excludes the others until the slot drops.
#[derive(Debug, Clone, Default)]
pub struct RestartGate(Arc<Mutex<HashSet<String>>>);

impl RestartGate {
    /// Claim `name`'s slot. `None` means a restart of that service is already
    /// in flight somewhere.
    pub fn try_acquire(&self, name: &str) -> Option<RestartSlot> {
        let mut in_flight = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if !in_flight.insert(name.to_string()) {
            return None;
        }
        Some(RestartSlot {
            set: Arc::clone(&self.0),
            name: name.to_string(),
        })
    }
}

/// RAII slot in the gate's in-flight set: dropping releases it, so a restart
/// that panics or is cancelled never wedges its service's slot.
#[derive(Debug)]
pub struct RestartSlot {
    set: Arc<Mutex<HashSet<String>>>,
    name: String,
}

impl Drop for RestartSlot {
    fn drop(&mut self) {
        let mut in_flight = self.set.lock().unwrap_or_else(|p| p.into_inner());
        in_flight.remove(&self.name);
    }
}

/// The supervised-services registry plus the shell seam and the shared
/// [`RestartGate`].
#[derive(derive_more::Debug)]
pub struct RestartManager {
    services: HashMap<String, ServiceConfig>,
    /// Runs both `restart_command` and each `health_command` probe: the
    /// [`Restarter`] contract — run a shell command bounded by a budget,
    /// `Ok` iff it exits 0 in time — is exactly what both need.
    #[debug(skip)]
    runner: Arc<dyn Restarter>,
    gate: RestartGate,
}

impl RestartManager {
    pub fn new(services: HashMap<String, ServiceConfig>, runner: Arc<dyn Restarter>) -> Self {
        Self {
            services,
            runner,
            gate: RestartGate::default(),
        }
    }

    /// Production manager: commands run through the platform shell.
    pub fn shell(services: HashMap<String, ServiceConfig>) -> Self {
        Self::new(services, Arc::new(ShellRestarter))
    }

    /// A clone of the shared gate, for wiring into the other restart paths
    /// (the corrective ladder) so they exclude this manager's restarts.
    pub fn gate(&self) -> RestartGate {
        self.gate.clone()
    }

    /// Restart `name`: run its `restart_command`, then confirm recovery via
    /// `health_command` when configured. Blocks until done — bounded by the
    /// service's `max_restart_duration`.
    pub async fn restart(&self, name: &str) -> Result<RestartReport, RestartError> {
        let service = self
            .services
            .get(name)
            .ok_or_else(|| RestartError::UnknownService(name.to_string()))?;
        let command = service
            .restart_command
            .as_deref()
            .ok_or_else(|| RestartError::NotRestartable(name.to_string()))?;
        let _slot = self
            .gate
            .try_acquire(name)
            .ok_or_else(|| RestartError::AlreadyInFlight(name.to_string()))?;

        let budget = service.max_restart_duration;
        info!("restarting service '{name}' (budget {budget:?})");
        let started = Instant::now();
        match self.runner.restart(command, budget).await {
            Ok(()) => {
                let recovery = match service.health_command.as_deref() {
                    Some(health) => {
                        let remaining = budget.saturating_sub(started.elapsed());
                        if self.await_healthy(name, health, remaining).await {
                            Recovery::Healthy
                        } else {
                            Recovery::Timeout
                        }
                    }
                    None => Recovery::Skipped,
                };
                info!("service '{name}' restarted (recovery: {recovery:?})");
                Ok(RestartReport {
                    service: name.to_string(),
                    status: "ok",
                    recovery: Some(recovery),
                    detail: None,
                })
            }
            Err(e) => {
                warn!("service '{name}' restart command failed: {e}");
                Ok(RestartReport {
                    service: name.to_string(),
                    status: "failed",
                    recovery: None,
                    detail: Some(e.to_string()),
                })
            }
        }
    }

    /// Poll `health_command` until it exits 0 or the remaining `budget` runs
    /// out. Mirrors the corrective ladder's `await_recovery`: the whole phase —
    /// probe runs *and* the sleeps between them — sits under one outer timeout.
    /// Each probe gets only its per-attempt slice of the budget, so one probe
    /// that hangs (e.g. a check blocking while the service is half-up) is
    /// killed and retried instead of monopolizing the whole phase.
    async fn await_healthy(&self, name: &str, command: &str, budget: Duration) -> bool {
        if budget.is_zero() {
            return false;
        }
        let interval = budget.checked_div(RECOVERY_ATTEMPTS).unwrap_or(budget);
        let poll = async {
            for attempt in 0..RECOVERY_ATTEMPTS {
                match self.runner.restart(command, interval).await {
                    Ok(()) => return true,
                    Err(e) => debug!("service '{name}' health probe not yet passing: {e}"),
                }
                if attempt + 1 < RECOVERY_ATTEMPTS {
                    tokio::time::sleep(interval).await;
                }
            }
            false
        };
        tokio::time::timeout(budget, poll).await.unwrap_or(false)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    use async_trait::async_trait;

    /// Records every command it runs; scripts one result per named command.
    #[derive(Debug, Default)]
    struct ScriptedRunner {
        /// Commands that succeed; everything else errors.
        succeeds: Vec<String>,
        calls: Mutex<Vec<String>>,
        /// The per-call budget each command was given.
        budgets: Mutex<Vec<Duration>>,
        /// When set, the runner blocks on this notification before returning
        /// (used to hold a restart in flight).
        hold: Option<Arc<tokio::sync::Notify>>,
    }

    impl ScriptedRunner {
        fn succeeding(commands: &[&str]) -> Self {
            Self {
                succeeds: commands.iter().map(|s| s.to_string()).collect(),
                ..Self::default()
            }
        }
    }

    #[async_trait]
    impl Restarter for ScriptedRunner {
        async fn restart(&self, command: &str, budget: Duration) -> crate::Result<()> {
            self.calls.lock().unwrap().push(command.to_string());
            self.budgets.lock().unwrap().push(budget);
            if let Some(hold) = &self.hold {
                hold.notified().await;
            }
            if self.succeeds.iter().any(|c| c == command) {
                Ok(())
            } else {
                Err(crate::SentinelError::Monitor(format!(
                    "`{command}` exited with 1"
                )))
            }
        }
    }

    fn service(
        restart_command: Option<&str>,
        health_command: Option<&str>,
        budget: Duration,
    ) -> ServiceConfig {
        ServiceConfig {
            base_url: None,
            device_number: 0,
            restart_command: restart_command.map(String::from),
            health_command: health_command.map(String::from),
            max_restart_duration: budget,
            health: None,
        }
    }

    fn manager(
        services: &[(&str, ServiceConfig)],
        runner: ScriptedRunner,
    ) -> (RestartManager, Arc<ScriptedRunner>) {
        let map = services
            .iter()
            .map(|(n, s)| (n.to_string(), s.clone()))
            .collect();
        let runner = Arc::new(runner);
        (
            RestartManager::new(map, Arc::clone(&runner) as Arc<dyn Restarter>),
            runner,
        )
    }

    #[tokio::test]
    async fn unknown_service_is_rejected() {
        let (m, _) = manager(&[], ScriptedRunner::default());
        let err = m.restart("nope").await.unwrap_err();
        assert_eq!(err, RestartError::UnknownService("nope".to_string()));
        assert!(err.to_string().contains("no configured service"), "{err}");
    }

    #[tokio::test]
    async fn not_restartable_service_is_rejected() {
        let svc = service(None, None, Duration::from_secs(1));
        let (m, runner) = manager(&[("mount", svc)], ScriptedRunner::default());
        let err = m.restart("mount").await.unwrap_err();
        assert!(
            runner.calls.lock().unwrap().is_empty(),
            "no command may run for a non-restartable service"
        );
        assert_eq!(err, RestartError::NotRestartable("mount".to_string()));
        assert!(err.to_string().contains("not restartable"), "{err}");
    }

    #[tokio::test]
    async fn restart_without_health_command_skips_recovery() {
        let svc = service(Some("restart-cmd"), None, Duration::from_secs(1));
        let (m, runner) = manager(
            &[("svc", svc)],
            ScriptedRunner::succeeding(&["restart-cmd"]),
        );
        let report = m.restart("svc").await.unwrap();
        assert_eq!(report.status, "ok");
        assert_eq!(report.recovery, Some(Recovery::Skipped));
        assert_eq!(report.detail, None);
        assert_eq!(
            *runner.calls.lock().unwrap(),
            vec!["restart-cmd".to_string()],
            "exactly the restart command runs; no health probe is configured"
        );
    }

    #[tokio::test]
    async fn restart_with_passing_health_reports_healthy() {
        let svc = service(
            Some("restart-cmd"),
            Some("health-cmd"),
            Duration::from_secs(1),
        );
        let runner = ScriptedRunner::succeeding(&["restart-cmd", "health-cmd"]);
        let (m, runner) = manager(&[("svc", svc)], runner);
        let report = m.restart("svc").await.unwrap();
        assert_eq!(report.status, "ok");
        assert_eq!(report.recovery, Some(Recovery::Healthy));
        assert_eq!(
            *runner.calls.lock().unwrap(),
            vec!["restart-cmd".to_string(), "health-cmd".to_string()]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn restart_with_failing_health_reports_timeout() {
        let svc = service(
            Some("restart-cmd"),
            Some("health-cmd"),
            Duration::from_millis(50),
        );
        // Only the restart command succeeds; every health probe errors.
        let runner = ScriptedRunner::succeeding(&["restart-cmd"]);
        let (m, _) = manager(&[("svc", svc)], runner);
        let report = m.restart("svc").await.unwrap();
        assert_eq!(report.status, "ok");
        assert_eq!(report.recovery, Some(Recovery::Timeout));
    }

    #[tokio::test(start_paused = true)]
    async fn health_probes_get_a_per_attempt_slice_of_the_budget() {
        let svc = service(
            Some("restart-cmd"),
            Some("health-cmd"),
            Duration::from_millis(50),
        );
        let runner = ScriptedRunner::succeeding(&["restart-cmd"]);
        let (m, runner) = manager(&[("svc", svc)], runner);
        m.restart("svc").await.unwrap();
        let budgets = runner.budgets.lock().unwrap();
        assert_eq!(
            budgets[0],
            Duration::from_millis(50),
            "the restart command gets the full budget"
        );
        assert!(budgets.len() > 1, "health probes must have run");
        for probe_budget in &budgets[1..] {
            // budget / RECOVERY_ATTEMPTS: one hanging probe is killed after its
            // slice instead of monopolizing the phase and foreclosing retries.
            assert_eq!(*probe_budget, Duration::from_millis(10), "{budgets:?}");
        }
    }

    #[tokio::test]
    async fn failing_restart_command_reports_failed_with_detail() {
        let svc = service(
            Some("restart-cmd"),
            Some("health-cmd"),
            Duration::from_secs(1),
        );
        // Nothing succeeds — the restart command itself errors.
        let runner = ScriptedRunner::default();
        let (m, runner) = manager(&[("svc", svc)], runner);
        let report = m.restart("svc").await.unwrap();
        assert_eq!(report.status, "failed");
        assert_eq!(report.recovery, None);
        assert!(
            report.detail.as_deref().unwrap_or("").contains("exited"),
            "{report:?}"
        );
        assert_eq!(
            *runner.calls.lock().unwrap(),
            vec!["restart-cmd".to_string()],
            "a failed restart must not probe health"
        );
    }

    #[test]
    fn gate_second_acquire_fails_until_slot_dropped() {
        let gate = RestartGate::default();
        let slot = gate.try_acquire("svc").expect("first acquire must succeed");
        assert!(gate.try_acquire("svc").is_none(), "slot is held");
        assert!(
            gate.try_acquire("other").is_some(),
            "different services are independent"
        );
        drop(slot);
        assert!(gate.try_acquire("svc").is_some(), "drop releases the slot");
    }

    #[test]
    fn gate_clones_share_the_set() {
        let gate = RestartGate::default();
        let clone = gate.clone();
        let _slot = clone.try_acquire("svc").unwrap();
        assert!(
            gate.try_acquire("svc").is_none(),
            "a slot held via a clone must block the original"
        );
    }

    #[tokio::test]
    async fn manager_gate_accessor_shares_in_flight_set() {
        let svc = service(Some("restart-cmd"), None, Duration::from_secs(1));
        let (m, runner) = manager(
            &[("svc", svc)],
            ScriptedRunner::succeeding(&["restart-cmd"]),
        );
        let slot = m.gate().try_acquire("svc").unwrap();
        let err = m.restart("svc").await.unwrap_err();
        assert_eq!(err, RestartError::AlreadyInFlight("svc".to_string()));
        assert!(
            runner.calls.lock().unwrap().is_empty(),
            "no command may run while the gate slot is held externally"
        );
        drop(slot);
        assert_eq!(m.restart("svc").await.unwrap().status, "ok");
    }

    #[tokio::test]
    async fn concurrent_restart_of_same_service_is_rejected() {
        let notify = Arc::new(tokio::sync::Notify::new());
        let runner = ScriptedRunner {
            succeeds: vec!["restart-cmd".to_string()],
            hold: Some(Arc::clone(&notify)),
            ..ScriptedRunner::default()
        };
        let svc = service(Some("restart-cmd"), None, Duration::from_secs(5));
        let (m, _) = manager(&[("svc", svc)], runner);
        let m = Arc::new(m);

        // Hold the first restart mid-command, then race a second one.
        let first = tokio::spawn({
            let m = Arc::clone(&m);
            async move { m.restart("svc").await }
        });
        tokio::task::yield_now().await;
        let err = m.restart("svc").await.unwrap_err();
        assert_eq!(err, RestartError::AlreadyInFlight("svc".to_string()));

        // Release the held command; the first restart completes and frees the
        // slot, so a follow-up restart is accepted again.
        notify.notify_one();
        first.await.unwrap().unwrap();
        notify.notify_one();
        let report = m.restart("svc").await.unwrap();
        assert_eq!(report.status, "ok");
    }
}
