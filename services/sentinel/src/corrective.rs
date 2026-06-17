//! Corrective-action ladder for the operation watchdog.
//!
//! When an `abort_then_restart` operation misses its deadline the watchdog
//! escalates through an *escalating* ladder against the service that owns the
//! family: health-check → abort → restart → (always) notify. Each rung is a
//! small trait with an HTTP or shell default impl, composed behind the single
//! [`Corrective`] seam ([`CorrectiveLadder`]) the watchdog calls. The least
//! invasive action that can clear the stall wins: a responsive service is
//! aborted (and the ladder stops); an unresponsive one — or a failed abort, or
//! a family with no abort verb — falls through to restart.
//!
//! See `docs/services/sentinel.md` §Operation Watchdog → Escalation.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::Instant;
use tracing::{debug, warn};

use crate::config::ServiceConfig;
use crate::io::HttpClient;

/// Health-check probe timeout (the design's 2 s cap).
const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);

/// How many times the ladder re-checks health after a restart before giving up
/// on confirming recovery. The total wait is bounded by `max_restart_duration`.
const RECOVERY_ATTEMPTS: u32 = 5;

/// The ASCOM device type and abort verb for an operation family. `None` for
/// families with no single Alpaca device to abort — compound operations like
/// `centering`, or non-device operations like `plate_solve`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlpacaBinding {
    pub device_type: &'static str,
    pub abort_verb: &'static str,
}

/// Map an operation family to the Alpaca device it drives. Park aborts the same
/// slew-to-park motion as a slew, so it shares `telescope`/`abortslew`.
pub fn alpaca_binding(family: &str) -> Option<AlpacaBinding> {
    match family {
        "slew" | "park" => Some(AlpacaBinding {
            device_type: "telescope",
            abort_verb: "abortslew",
        }),
        "exposure" => Some(AlpacaBinding {
            device_type: "camera",
            abort_verb: "abortexposure",
        }),
        "move_focuser" => Some(AlpacaBinding {
            device_type: "focuser",
            abort_verb: "halt",
        }),
        _ => None,
    }
}

/// A fully-resolved corrective target: which Alpaca device to probe / abort and
/// which command restarts the owning service. Built from an operation family
/// plus its owning [`ServiceConfig`].
#[derive(Debug, Clone)]
pub struct CorrectiveTarget {
    pub service_name: String,
    pub base_url: String,
    pub device_number: u32,
    pub binding: Option<AlpacaBinding>,
    pub restart_command: Option<String>,
}

impl CorrectiveTarget {
    /// Resolve a target from the family and the service that owns it.
    pub fn new(service_name: &str, family: &str, service: &ServiceConfig) -> Self {
        Self {
            service_name: service_name.to_string(),
            base_url: service.base_url.trim_end_matches('/').to_string(),
            device_number: service.device_number,
            binding: alpaca_binding(family),
            restart_command: service.restart_command.clone(),
        }
    }

    /// `{base}/{device-type}/{n}/connected`, or `None` when the family has no
    /// Alpaca device to probe.
    fn connected_url(&self) -> Option<String> {
        let b = self.binding?;
        Some(format!(
            "{}/{}/{}/connected",
            self.base_url, b.device_type, self.device_number
        ))
    }

    /// `{base}/{device-type}/{n}/{verb}`, or `None` when the family has no
    /// abort verb.
    fn abort_url(&self) -> Option<String> {
        let b = self.binding?;
        Some(format!(
            "{}/{}/{}/{}",
            self.base_url, b.device_type, self.device_number, b.abort_verb
        ))
    }
}

/// Whether a service answered its health probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Healthiness {
    /// A clean `200` — the service is alive (the *operation* is stuck).
    Responsive,
    /// Non-200, timeout, or transport error — the service itself is down.
    Unresponsive,
    /// No probe was possible (the family has no Alpaca device).
    Unknown,
}

fn health_label(h: Healthiness) -> &'static str {
    match h {
        Healthiness::Responsive => "responsive",
        Healthiness::Unresponsive => "unresponsive",
        Healthiness::Unknown => "unknown",
    }
}

// ---- rung traits -------------------------------------------------------

/// Rung 1 — probe whether a service is alive.
#[async_trait]
pub trait HealthChecker: Send + Sync + fmt::Debug {
    async fn check(&self, target: &CorrectiveTarget) -> Healthiness;
}

/// Rung 2 — abort the in-flight operation on a responsive service.
#[async_trait]
pub trait Aborter: Send + Sync + fmt::Debug {
    /// `Ok` on a clean abort; `Err` otherwise (no verb, non-200, transport).
    async fn abort(&self, target: &CorrectiveTarget) -> crate::Result<()>;
}

/// Rung 3 — restart an unresponsive (or un-abortable) service.
#[async_trait]
pub trait Restarter: Send + Sync + fmt::Debug {
    /// Run `command`, bounded by `budget`. `Ok` iff it exits 0 in time.
    async fn restart(&self, command: &str, budget: Duration) -> crate::Result<()>;
}

// ---- default impls -----------------------------------------------------

/// HTTP health checker: `GET {connected_url}` with a 2 s timeout. A clean 200
/// is `Responsive`; anything else (non-200, timeout, refused) is
/// `Unresponsive`.
#[derive(derive_more::Debug)]
pub struct HttpHealthChecker {
    #[debug(skip)]
    http: Arc<dyn HttpClient>,
}

impl HttpHealthChecker {
    pub fn new(http: Arc<dyn HttpClient>) -> Self {
        Self { http }
    }
}

#[async_trait]
impl HealthChecker for HttpHealthChecker {
    async fn check(&self, target: &CorrectiveTarget) -> Healthiness {
        let Some(url) = target.connected_url() else {
            return Healthiness::Unknown;
        };
        match tokio::time::timeout(HEALTH_TIMEOUT, self.http.get(&url)).await {
            Ok(Ok(resp)) if resp.status == 200 => Healthiness::Responsive,
            Ok(Ok(resp)) => {
                debug!("health check {url} -> {} (unresponsive)", resp.status);
                Healthiness::Unresponsive
            }
            Ok(Err(e)) => {
                debug!("health check {url} failed: {e}");
                Healthiness::Unresponsive
            }
            Err(_) => {
                debug!("health check {url} timed out after {HEALTH_TIMEOUT:?}");
                Healthiness::Unresponsive
            }
        }
    }
}

/// HTTP aborter: `PUT {abort_url}` with the ASCOM `ClientID` form param.
#[derive(derive_more::Debug)]
pub struct HttpAborter {
    #[debug(skip)]
    http: Arc<dyn HttpClient>,
}

impl HttpAborter {
    pub fn new(http: Arc<dyn HttpClient>) -> Self {
        Self { http }
    }
}

#[async_trait]
impl Aborter for HttpAborter {
    async fn abort(&self, target: &CorrectiveTarget) -> crate::Result<()> {
        let Some(url) = target.abort_url() else {
            return Err(crate::SentinelError::Monitor(format!(
                "service '{}' has no abort verb for its family",
                target.service_name
            )));
        };
        debug!("abort PUT {url}");
        let resp = self.http.put_form(&url, &[("ClientID", "1")]).await?;
        if resp.status == 200 {
            Ok(())
        } else {
            Err(crate::SentinelError::Http(format!(
                "abort {url} -> {}",
                resp.status
            )))
        }
    }
}

/// Shell restarter: runs the command via `sh -c`, bounded by the budget.
#[derive(Debug, Default)]
pub struct ShellRestarter;

#[async_trait]
impl Restarter for ShellRestarter {
    async fn restart(&self, command: &str, budget: Duration) -> crate::Result<()> {
        debug!("restart: running `{command}` (budget {budget:?})");
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .spawn()
            .map_err(|e| {
                crate::SentinelError::Monitor(format!("failed to spawn restart `{command}`: {e}"))
            })?;
        match tokio::time::timeout(budget, child.wait()).await {
            Ok(Ok(status)) if status.success() => Ok(()),
            Ok(Ok(status)) => Err(crate::SentinelError::Monitor(format!(
                "restart `{command}` exited with {status}"
            ))),
            Ok(Err(e)) => Err(crate::SentinelError::Monitor(format!(
                "restart `{command}` wait failed: {e}"
            ))),
            Err(_) => {
                let _ = child.start_kill();
                Err(crate::SentinelError::Monitor(format!(
                    "restart `{command}` exceeded {budget:?}"
                )))
            }
        }
    }
}

// ---- ladder ------------------------------------------------------------

/// Human-readable summary of which ladder rungs ran and how they resolved.
/// Rendered into the escalation message's `{action}` placeholder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LadderOutcome {
    pub rungs: Vec<String>,
}

impl LadderOutcome {
    fn push(&mut self, rung: impl Into<String>) {
        self.rungs.push(rung.into());
    }

    /// Render as a message suffix (`" — corrective action: …"`), or `""` when
    /// no rung ran (notify-only).
    pub fn action_suffix(&self) -> String {
        if self.rungs.is_empty() {
            String::new()
        } else {
            format!(" — corrective action: {}", self.rungs.join(", "))
        }
    }
}

/// The single seam the watchdog calls to run the corrective ladder. Abstracted
/// so the watchdog's policy branching can be unit-tested against a recording
/// mock without HTTP or subprocesses.
#[async_trait]
pub trait Corrective: Send + Sync + fmt::Debug {
    async fn run(&self, target: &CorrectiveTarget) -> LadderOutcome;
}

/// The escalating health → abort → restart ladder.
#[derive(Debug)]
pub struct CorrectiveLadder {
    health: Arc<dyn HealthChecker>,
    aborter: Arc<dyn Aborter>,
    restarter: Arc<dyn Restarter>,
    max_restart_duration: Duration,
}

impl CorrectiveLadder {
    pub fn new(
        health: Arc<dyn HealthChecker>,
        aborter: Arc<dyn Aborter>,
        restarter: Arc<dyn Restarter>,
        max_restart_duration: Duration,
    ) -> Self {
        Self {
            health,
            aborter,
            restarter,
            max_restart_duration,
        }
    }

    /// Production ladder: HTTP health-check + abort over a shared client, shell
    /// restarter.
    pub fn http(http: Arc<dyn HttpClient>, max_restart_duration: Duration) -> Self {
        Self::new(
            Arc::new(HttpHealthChecker::new(Arc::clone(&http))),
            Arc::new(HttpAborter::new(http)),
            Arc::new(ShellRestarter),
            max_restart_duration,
        )
    }

    /// Restart rung: run the command (if any), then confirm recovery.
    async fn run_restart(&self, target: &CorrectiveTarget, outcome: &mut LadderOutcome) {
        let Some(command) = target.restart_command.as_deref() else {
            debug!(
                "ladder '{}': not restartable, stopping at notify",
                target.service_name
            );
            outcome.push("restart=skipped(not restartable)");
            return;
        };
        // `max_restart_duration` is one budget for the command *and* the
        // recovery wait — measure what the command spends so recovery gets only
        // the remainder (rather than a second full budget).
        let started = Instant::now();
        match self
            .restarter
            .restart(command, self.max_restart_duration)
            .await
        {
            Ok(()) => {
                outcome.push("restart=ran");
                let remaining = self.max_restart_duration.saturating_sub(started.elapsed());
                if self.await_recovery(target, remaining).await {
                    outcome.push("recovery=responsive");
                } else {
                    outcome.push("recovery=timeout");
                }
            }
            Err(e) => {
                warn!("ladder '{}': restart failed: {e}", target.service_name);
                outcome.push("restart=failed");
            }
        }
    }

    /// Poll health until the service is responsive again or the attempts run
    /// out. The poll interval divides the remaining restart `budget` evenly, so
    /// the command and the recovery wait together stay within
    /// `max_restart_duration`.
    async fn await_recovery(&self, target: &CorrectiveTarget, budget: Duration) -> bool {
        let interval = budget.checked_div(RECOVERY_ATTEMPTS).unwrap_or(budget);
        for attempt in 0..RECOVERY_ATTEMPTS {
            if self.health.check(target).await == Healthiness::Responsive {
                return true;
            }
            if attempt + 1 < RECOVERY_ATTEMPTS {
                tokio::time::sleep(interval).await;
            }
        }
        false
    }
}

#[async_trait]
impl Corrective for CorrectiveLadder {
    async fn run(&self, target: &CorrectiveTarget) -> LadderOutcome {
        let mut outcome = LadderOutcome::default();

        // Rung 1 — health check.
        let health = self.health.check(target).await;
        debug!("ladder '{}': health {:?}", target.service_name, health);
        outcome.push(format!("health={}", health_label(health)));

        // Rung 2 — abort, but only a responsive service with an abort verb.
        let aborted = if health == Healthiness::Responsive && target.binding.is_some() {
            match self.aborter.abort(target).await {
                Ok(()) => {
                    debug!("ladder '{}': abort ok", target.service_name);
                    outcome.push("abort=ok");
                    true
                }
                Err(e) => {
                    warn!("ladder '{}': abort failed: {e}", target.service_name);
                    outcome.push("abort=failed");
                    false
                }
            }
        } else {
            false
        };

        // A clean abort is the gentle fix — stop here.
        if aborted {
            return outcome;
        }

        // Rung 3 — restart (unresponsive, abort failed, or no abort verb).
        self.run_restart(target, &mut outcome).await;
        outcome
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    use crate::io::{HttpResponse, MockHttpClient};

    fn target(restart_command: Option<&str>) -> CorrectiveTarget {
        CorrectiveTarget::new(
            "mount",
            "slew",
            &ServiceConfig {
                base_url: "http://svc/api/v1".to_string(),
                device_number: 0,
                restart_command: restart_command.map(String::from),
            },
        )
    }

    fn compound_target(restart_command: Option<&str>) -> CorrectiveTarget {
        // `centering` has no Alpaca binding.
        CorrectiveTarget::new(
            "rp",
            "centering",
            &ServiceConfig {
                base_url: "http://svc/api/v1".to_string(),
                device_number: 0,
                restart_command: restart_command.map(String::from),
            },
        )
    }

    // ---- mock rungs ----------------------------------------------------

    #[derive(Debug)]
    struct MockHealth {
        results: Mutex<VecDeque<Healthiness>>,
        default: Healthiness,
        calls: AtomicU32,
    }
    impl MockHealth {
        fn new(default: Healthiness, scripted: Vec<Healthiness>) -> Arc<Self> {
            Arc::new(Self {
                results: Mutex::new(scripted.into()),
                default,
                calls: AtomicU32::new(0),
            })
        }
    }
    #[async_trait]
    impl HealthChecker for MockHealth {
        async fn check(&self, _target: &CorrectiveTarget) -> Healthiness {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(self.default)
        }
    }

    #[derive(Debug)]
    struct MockAbort {
        ok: bool,
        calls: AtomicU32,
    }
    impl MockAbort {
        fn new(ok: bool) -> Arc<Self> {
            Arc::new(Self {
                ok,
                calls: AtomicU32::new(0),
            })
        }
    }
    #[async_trait]
    impl Aborter for MockAbort {
        async fn abort(&self, _target: &CorrectiveTarget) -> crate::Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.ok {
                Ok(())
            } else {
                Err(crate::SentinelError::Http("abort boom".to_string()))
            }
        }
    }

    #[derive(Debug)]
    struct MockRestart {
        ok: bool,
        calls: AtomicU32,
        last_command: Mutex<Option<String>>,
    }
    impl MockRestart {
        fn new(ok: bool) -> Arc<Self> {
            Arc::new(Self {
                ok,
                calls: AtomicU32::new(0),
                last_command: Mutex::new(None),
            })
        }
    }
    #[async_trait]
    impl Restarter for MockRestart {
        async fn restart(&self, command: &str, _budget: Duration) -> crate::Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_command.lock().unwrap() = Some(command.to_string());
            if self.ok {
                Ok(())
            } else {
                Err(crate::SentinelError::Monitor("restart boom".to_string()))
            }
        }
    }

    fn ladder(
        health: Arc<MockHealth>,
        aborter: Arc<MockAbort>,
        restarter: Arc<MockRestart>,
    ) -> CorrectiveLadder {
        CorrectiveLadder::new(health, aborter, restarter, Duration::from_millis(20))
    }

    // ---- ladder orchestration -----------------------------------------

    #[tokio::test(start_paused = true)]
    async fn responsive_service_is_aborted_not_restarted() {
        let health = MockHealth::new(Healthiness::Responsive, vec![]);
        let aborter = MockAbort::new(true);
        let restarter = MockRestart::new(true);
        let l = ladder(health.clone(), aborter.clone(), restarter.clone());

        let outcome = l.run(&target(Some("restart"))).await;

        assert_eq!(aborter.calls.load(Ordering::SeqCst), 1, "abort must run");
        assert_eq!(
            restarter.calls.load(Ordering::SeqCst),
            0,
            "a clean abort must stop the ladder before restart"
        );
        assert_eq!(
            outcome.rungs,
            vec!["health=responsive".to_string(), "abort=ok".to_string()]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn unresponsive_service_is_restarted_then_recovers() {
        // Initial probe unresponsive; recovery probe responsive.
        let health = MockHealth::new(
            Healthiness::Responsive,
            vec![Healthiness::Unresponsive, Healthiness::Responsive],
        );
        let aborter = MockAbort::new(true);
        let restarter = MockRestart::new(true);
        let l = ladder(health.clone(), aborter.clone(), restarter.clone());

        let outcome = l.run(&target(Some("systemctl restart mount"))).await;

        assert_eq!(
            aborter.calls.load(Ordering::SeqCst),
            0,
            "no abort when down"
        );
        assert_eq!(restarter.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            restarter.last_command.lock().unwrap().as_deref(),
            Some("systemctl restart mount")
        );
        assert!(outcome.rungs.contains(&"restart=ran".to_string()));
        assert!(outcome.rungs.contains(&"recovery=responsive".to_string()));
    }

    #[tokio::test(start_paused = true)]
    async fn restart_without_recovery_reports_timeout() {
        // Every probe unresponsive -> recovery never confirmed.
        let health = MockHealth::new(Healthiness::Unresponsive, vec![]);
        let aborter = MockAbort::new(true);
        let restarter = MockRestart::new(true);
        let l = ladder(health, aborter, restarter);

        let outcome = l.run(&target(Some("restart"))).await;

        assert!(outcome.rungs.contains(&"restart=ran".to_string()));
        assert!(outcome.rungs.contains(&"recovery=timeout".to_string()));
    }

    #[tokio::test(start_paused = true)]
    async fn abort_failure_falls_through_to_restart() {
        let health = MockHealth::new(Healthiness::Responsive, vec![Healthiness::Responsive]);
        let aborter = MockAbort::new(false);
        let restarter = MockRestart::new(true);
        let l = ladder(health, aborter.clone(), restarter.clone());

        let outcome = l.run(&target(Some("restart"))).await;

        assert_eq!(aborter.calls.load(Ordering::SeqCst), 1);
        assert_eq!(restarter.calls.load(Ordering::SeqCst), 1);
        assert!(outcome.rungs.contains(&"abort=failed".to_string()));
        assert!(outcome.rungs.contains(&"restart=ran".to_string()));
    }

    #[tokio::test(start_paused = true)]
    async fn compound_family_skips_abort_and_restarts() {
        let health = MockHealth::new(Healthiness::Responsive, vec![Healthiness::Responsive]);
        let aborter = MockAbort::new(true);
        let restarter = MockRestart::new(true);
        let l = ladder(health, aborter.clone(), restarter.clone());

        let outcome = l.run(&compound_target(Some("restart"))).await;

        assert_eq!(
            aborter.calls.load(Ordering::SeqCst),
            0,
            "a family with no abort verb must skip abort"
        );
        assert_eq!(restarter.calls.load(Ordering::SeqCst), 1);
        assert!(outcome.rungs.iter().any(|r| r.starts_with("health=")));
    }

    #[tokio::test(start_paused = true)]
    async fn not_restartable_stops_at_abort() {
        let health = MockHealth::new(Healthiness::Unresponsive, vec![]);
        let aborter = MockAbort::new(true);
        let restarter = MockRestart::new(true);
        let l = ladder(health, aborter.clone(), restarter.clone());

        let outcome = l.run(&target(None)).await;

        assert_eq!(restarter.calls.load(Ordering::SeqCst), 0);
        assert!(outcome.rungs.iter().any(|r| r.contains("not restartable")));
    }

    #[tokio::test(start_paused = true)]
    async fn restart_failure_is_reported() {
        let health = MockHealth::new(Healthiness::Unresponsive, vec![]);
        let aborter = MockAbort::new(true);
        let restarter = MockRestart::new(false);
        let l = ladder(health, aborter, restarter);

        let outcome = l.run(&target(Some("restart"))).await;

        assert!(outcome.rungs.contains(&"restart=failed".to_string()));
        assert!(!outcome.rungs.iter().any(|r| r.starts_with("recovery=")));
    }

    // ---- target / binding ---------------------------------------------

    #[test]
    fn binding_table_maps_known_families() {
        assert_eq!(alpaca_binding("slew").unwrap().device_type, "telescope");
        assert_eq!(alpaca_binding("park").unwrap().abort_verb, "abortslew");
        assert_eq!(
            alpaca_binding("exposure").unwrap().abort_verb,
            "abortexposure"
        );
        assert_eq!(alpaca_binding("move_focuser").unwrap().abort_verb, "halt");
        assert!(alpaca_binding("centering").is_none());
        assert!(alpaca_binding("plate_solve").is_none());
    }

    #[test]
    fn target_builds_alpaca_urls_and_trims_base() {
        let t = CorrectiveTarget::new(
            "mount",
            "exposure",
            &ServiceConfig {
                base_url: "http://svc:11111/api/v1/".to_string(),
                device_number: 3,
                restart_command: None,
            },
        );
        assert_eq!(
            t.connected_url().unwrap(),
            "http://svc:11111/api/v1/camera/3/connected"
        );
        assert_eq!(
            t.abort_url().unwrap(),
            "http://svc:11111/api/v1/camera/3/abortexposure"
        );
    }

    #[test]
    fn target_with_no_binding_has_no_urls() {
        let t = compound_target(None);
        assert!(t.connected_url().is_none());
        assert!(t.abort_url().is_none());
    }

    #[test]
    fn action_suffix_empty_when_no_rungs() {
        assert_eq!(LadderOutcome::default().action_suffix(), "");
    }

    #[test]
    fn action_suffix_lists_rungs() {
        let mut o = LadderOutcome::default();
        o.push("health=responsive");
        o.push("abort=ok");
        assert_eq!(
            o.action_suffix(),
            " — corrective action: health=responsive, abort=ok"
        );
    }

    // ---- default HTTP impls -------------------------------------------

    fn ok_200() -> HttpResponse {
        HttpResponse {
            status: 200,
            body: r#"{"Value": true, "ErrorNumber": 0, "ErrorMessage": ""}"#.to_string(),
        }
    }

    #[tokio::test]
    async fn http_health_200_is_responsive() {
        let mut mock = MockHttpClient::new();
        mock.expect_get()
            .withf(|url| url.contains("/telescope/0/connected"))
            .returning(|_| Box::pin(async { Ok(ok_200()) }));
        let checker = HttpHealthChecker::new(Arc::new(mock));
        assert_eq!(checker.check(&target(None)).await, Healthiness::Responsive);
    }

    #[tokio::test]
    async fn http_health_non_200_is_unresponsive() {
        let mut mock = MockHttpClient::new();
        mock.expect_get().returning(|_| {
            Box::pin(async {
                Ok(HttpResponse {
                    status: 500,
                    body: String::new(),
                })
            })
        });
        let checker = HttpHealthChecker::new(Arc::new(mock));
        assert_eq!(
            checker.check(&target(None)).await,
            Healthiness::Unresponsive
        );
    }

    #[tokio::test]
    async fn http_health_error_is_unresponsive() {
        let mut mock = MockHttpClient::new();
        mock.expect_get()
            .returning(|_| Box::pin(async { Err(crate::SentinelError::Http("refused".into())) }));
        let checker = HttpHealthChecker::new(Arc::new(mock));
        assert_eq!(
            checker.check(&target(None)).await,
            Healthiness::Unresponsive
        );
    }

    #[tokio::test]
    async fn http_health_no_binding_is_unknown() {
        // No HTTP call expected — the mock has no expectations set.
        let checker = HttpHealthChecker::new(Arc::new(MockHttpClient::new()));
        assert_eq!(
            checker.check(&compound_target(None)).await,
            Healthiness::Unknown
        );
    }

    #[tokio::test]
    async fn http_abort_puts_verb_and_succeeds() {
        let mut mock = MockHttpClient::new();
        mock.expect_put_form()
            .withf(|url, params| {
                url.ends_with("/telescope/0/abortslew") && params.contains(&("ClientID", "1"))
            })
            .returning(|_, _| {
                Box::pin(async {
                    Ok(HttpResponse {
                        status: 200,
                        body: String::new(),
                    })
                })
            });
        let aborter = HttpAborter::new(Arc::new(mock));
        aborter.abort(&target(None)).await.unwrap();
    }

    #[tokio::test]
    async fn http_abort_no_verb_errors() {
        let aborter = HttpAborter::new(Arc::new(MockHttpClient::new()));
        let err = aborter.abort(&compound_target(None)).await.unwrap_err();
        assert!(err.to_string().contains("no abort verb"), "{err}");
    }

    #[tokio::test]
    async fn http_abort_non_200_errors() {
        let mut mock = MockHttpClient::new();
        mock.expect_put_form().returning(|_, _| {
            Box::pin(async {
                Ok(HttpResponse {
                    status: 409,
                    body: String::new(),
                })
            })
        });
        let aborter = HttpAborter::new(Arc::new(mock));
        let err = aborter.abort(&target(None)).await.unwrap_err();
        assert!(err.to_string().contains("409"), "{err}");
    }

    // ---- shell restarter (unix only; CI Windows lacks `sh`) -----------

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_restart_zero_exit_ok() {
        ShellRestarter
            .restart("true", Duration::from_secs(5))
            .await
            .unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_restart_nonzero_exit_errors() {
        let err = ShellRestarter
            .restart("false", Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("exited"), "{err}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_restart_times_out() {
        let err = ShellRestarter
            .restart("sleep 5", Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("exceeded"), "{err}");
    }
}
