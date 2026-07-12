//! Service health supervision: periodic HTTP health probes with autonomous
//! restart.
//!
//! Each `services` entry with a `health` block gets one
//! [`ServiceHealthSupervisor`] — an [`EventMonitor`] task probing
//! `GET {health.url}` every `poll_interval`. Only a clean 200 counts as
//! alive; any other status, a timeout, or a connection error is a failed
//! probe. After `failure_threshold` consecutive failures the supervisor runs
//! the service's `restart_command` through the shared [`RestartManager`]
//! (inheriting the one-restart-per-service gate), then backs off — doubling
//! from `restart_backoff` up to `restart_backoff_max` — before any further
//! attempt. Probing never stops and the supervisor never gives up; one
//! successful probe resets the whole outage. See
//! `docs/services/sentinel.md` §Service Health Supervision.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::config::HealthConfig;
use crate::io::HttpClient;
use crate::notifier::{Notification, NotificationRecord, Notifier};
use crate::restart::{RestartError, RestartManager, RestartReport};
use crate::state::{ServiceHealth, ServiceHealthStatus, StateHandle};
use crate::watchdog::EventMonitor;

/// Probe timeout — the same bound as the corrective ladder's health rung, so
/// a wedged service can never stall the probe loop.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Supervises one service: probe loop + failure counting + autonomous
/// restart with backoff. One instance (and one engine task) per supervised
/// service, so an in-flight restart of one service never delays the probes
/// of another.
#[derive(derive_more::Debug)]
pub struct ServiceHealthSupervisor {
    /// The service's key in the `services` map — used in logs, notification
    /// history records, and the dashboard snapshot.
    name: String,
    config: HealthConfig,
    /// `false` when the service has no `restart_command`: probe-and-notify
    /// only, mirroring the watchdog ladder's degrade-with-warning posture.
    restartable: bool,
    #[debug(skip)]
    http: Arc<dyn HttpClient>,
    restarts: Arc<RestartManager>,
    notifiers: Vec<Arc<dyn Notifier>>,
    state: StateHandle,
}

impl ServiceHealthSupervisor {
    pub fn new(
        name: String,
        config: HealthConfig,
        restartable: bool,
        http: Arc<dyn HttpClient>,
        restarts: Arc<RestartManager>,
        notifiers: Vec<Arc<dyn Notifier>>,
        state: StateHandle,
    ) -> Self {
        if !restartable {
            warn!(
                "service '{name}' has a health block but no restart_command; \
                 supervising probe-and-notify only"
            );
        }
        Self {
            name,
            config,
            restartable,
            http,
            restarts,
            notifiers,
            state,
        }
    }

    /// One probe: `true` iff `GET {url}` answers a clean 200 within
    /// [`PROBE_TIMEOUT`]. The body is never parsed.
    async fn probe(&self) -> bool {
        let url = &self.config.url;
        match tokio::time::timeout(PROBE_TIMEOUT, self.http.get(url)).await {
            Ok(Ok(resp)) if resp.status == 200 => true,
            Ok(Ok(resp)) => {
                debug!("health probe {url} -> {} (down)", resp.status);
                false
            }
            Ok(Err(e)) => {
                debug!("health probe {url} failed: {e}");
                false
            }
            Err(_) => {
                debug!("health probe {url} timed out after {PROBE_TIMEOUT:?}");
                false
            }
        }
    }

    /// Publish this service's snapshot to the shared state. The supervisor's
    /// locals are authoritative; the epoch-ms conversion of the monotonic
    /// next-restart deadline happens only here, at the state boundary.
    async fn publish(
        &self,
        health: ServiceHealth,
        consecutive_failures: u32,
        restarts_in_outage: u32,
        total_restarts: u64,
        next_restart_at: Option<Instant>,
    ) {
        let now_ms = current_epoch_ms();
        let next_restart_epoch_ms = next_restart_at.map(|at| {
            let remaining = at.saturating_duration_since(Instant::now());
            now_ms.saturating_add(remaining.as_millis() as u64)
        });
        self.state
            .write()
            .await
            .set_service_health(ServiceHealthStatus {
                name: self.name.clone(),
                health,
                last_probe_epoch_ms: now_ms,
                consecutive_failures,
                restarts_in_outage,
                total_restarts,
                next_restart_epoch_ms,
                poll_interval: self.config.poll_interval,
            });
    }

    /// Dispatch through every configured notifier and record each attempt in
    /// the dashboard history (the watchdog's escalation pattern; manual REST
    /// restarts stay silent by design, autonomous ones never do).
    async fn notify(&self, message: String, priority: i8) {
        warn!("health supervision '{}': {}", self.name, message);
        let notification = Notification {
            title: "Observatory Service Health".to_string(),
            message: message.clone(),
            priority,
            sound: None,
        };
        let now_ms = current_epoch_ms();
        for notifier in &self.notifiers {
            let result = notifier.notify(&notification).await;
            if let Err(e) = &result {
                warn!(
                    "health notification via '{}' failed: {}",
                    notifier.type_name(),
                    e
                );
            }
            let record = NotificationRecord {
                monitor_name: self.name.clone(),
                notifier_type: notifier.type_name().to_string(),
                message: message.clone(),
                success: result.is_ok(),
                error: result.as_ref().err().map(|e| e.to_string()),
                timestamp_epoch_ms: now_ms,
            };
            self.state.write().await.add_notification(record);
        }
    }

    /// Notification for a completed autonomous restart attempt. The first
    /// attempt of an outage is priority 0; every later one escalates to
    /// priority 1 with a "still unhealthy" message (the phrase the dashboard
    /// history is asserted on — records carry no priority).
    async fn notify_restart(
        &self,
        report: &RestartReport,
        consecutive_failures: u32,
        restarts_in_outage: u32,
        next_wait: Duration,
    ) {
        let outcome = restart_outcome_text(report);
        if restarts_in_outage <= 1 {
            self.notify(
                format!(
                    "Service '{}' unhealthy ({consecutive_failures} consecutive failed \
                     probes); restarted autonomously — {outcome}",
                    self.name
                ),
                0,
            )
            .await;
        } else {
            self.notify(
                format!(
                    "Service '{}' still unhealthy after {restarts_in_outage} autonomous \
                     restarts — {outcome}; next attempt not before {}",
                    self.name,
                    humantime::format_duration(next_wait)
                ),
                1,
            )
            .await;
        }
    }
}

/// `status ok, recovery healthy` / `status failed (…detail…)` — the report
/// rendered for notification messages.
fn restart_outcome_text(report: &RestartReport) -> String {
    match (&report.recovery, &report.detail) {
        (Some(recovery), _) => format!(
            "status {}, recovery {}",
            report.status,
            match recovery {
                crate::restart::Recovery::Healthy => "healthy",
                crate::restart::Recovery::Timeout => "timeout",
                crate::restart::Recovery::Skipped => "skipped",
            }
        ),
        (None, Some(detail)) => format!("status {} ({detail})", report.status),
        (None, None) => format!("status {}", report.status),
    }
}

#[async_trait]
impl EventMonitor for ServiceHealthSupervisor {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run(&self, cancel: CancellationToken) {
        let mut consecutive_failures: u32 = 0;
        let mut restarts_in_outage: u32 = 0;
        let mut total_restarts: u64 = 0;
        let initial_backoff = self
            .config
            .restart_backoff
            .min(self.config.restart_backoff_max);
        let mut backoff = initial_backoff;
        let mut next_restart_at: Option<Instant> = None;

        debug!(
            "health supervisor '{}' starting: {} every {:?}, threshold {}",
            self.name, self.config.url, self.config.poll_interval, self.config.failure_threshold
        );

        loop {
            if cancel.is_cancelled() {
                return;
            }

            if self.probe().await {
                if restarts_in_outage > 0 || consecutive_failures > 0 {
                    info!(
                        "service '{}' is healthy again ({} autonomous restart(s) this outage)",
                        self.name, restarts_in_outage
                    );
                }
                consecutive_failures = 0;
                restarts_in_outage = 0;
                backoff = initial_backoff;
                next_restart_at = None;
                self.publish(ServiceHealth::Up, 0, 0, total_restarts, None)
                    .await;
            } else {
                consecutive_failures = consecutive_failures.saturating_add(1);
                debug!(
                    "service '{}' probe failed ({consecutive_failures} consecutive)",
                    self.name
                );
                // Publish before any restart so the dashboard shows Down (with
                // current counters) while a slow restart is running.
                self.publish(
                    ServiceHealth::Down,
                    consecutive_failures,
                    restarts_in_outage,
                    total_restarts,
                    next_restart_at,
                )
                .await;

                if !self.restartable {
                    // Once per outage: the counter only resets on recovery.
                    if consecutive_failures == self.config.failure_threshold {
                        self.notify(
                            format!(
                                "Service '{}' is unhealthy ({consecutive_failures} consecutive \
                                 failed probes) and has no restart_command; manual intervention \
                                 required",
                                self.name
                            ),
                            1,
                        )
                        .await;
                    }
                } else if consecutive_failures >= self.config.failure_threshold
                    && next_restart_at.is_none_or(|at| Instant::now() >= at)
                {
                    let attempt = tokio::select! {
                        result = self.restarts.restart(&self.name) => result,
                        // A cancelled restart drops its gate slot; the shell
                        // child runs to completion detached.
                        _ = cancel.cancelled() => return,
                    };
                    match attempt {
                        Ok(report) => {
                            restarts_in_outage = restarts_in_outage.saturating_add(1);
                            total_restarts = total_restarts.saturating_add(1);
                            let wait = backoff;
                            next_restart_at = Some(Instant::now() + wait);
                            backoff = backoff
                                .saturating_mul(2)
                                .min(self.config.restart_backoff_max);
                            self.notify_restart(
                                &report,
                                consecutive_failures,
                                restarts_in_outage,
                                wait,
                            )
                            .await;
                            self.publish(
                                ServiceHealth::Down,
                                consecutive_failures,
                                restarts_in_outage,
                                total_restarts,
                                next_restart_at,
                            )
                            .await;
                        }
                        Err(RestartError::AlreadyInFlight(_)) => {
                            // Another path (REST endpoint, watchdog ladder) is
                            // restarting this service right now — its effect
                            // shows up in the next probes. Nothing to count,
                            // schedule, or notify.
                            debug!(
                                "service '{}': restart already in flight elsewhere; \
                                 continuing to probe",
                                self.name
                            );
                        }
                        Err(e) => {
                            // Unreachable by construction: the supervisor only
                            // exists for configured, restartable services.
                            warn!("service '{}': autonomous restart rejected: {e}", self.name);
                        }
                    }
                }
            }

            tokio::select! {
                _ = tokio::time::sleep(self.config.poll_interval) => {}
                _ = cancel.cancelled() => return,
            }
        }
    }
}

fn current_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    use crate::config::ServiceConfig;
    use crate::corrective::Restarter;
    use crate::io::HttpResponse;
    use crate::state::new_state_handle;

    /// What every `GET` answers; flippable mid-test.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ProbeAnswer {
        Ok200,
        Status(u16),
        TransportError,
        /// Never resolves — exercises the probe timeout.
        Hang,
    }

    #[derive(Debug)]
    struct ScriptedHttp {
        answer: Mutex<ProbeAnswer>,
        probes: AtomicU32,
    }

    impl ScriptedHttp {
        fn new(answer: ProbeAnswer) -> Arc<Self> {
            Arc::new(Self {
                answer: Mutex::new(answer),
                probes: AtomicU32::new(0),
            })
        }

        fn set(&self, answer: ProbeAnswer) {
            *self.answer.lock().unwrap() = answer;
        }

        fn probes(&self) -> u32 {
            self.probes.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl HttpClient for ScriptedHttp {
        async fn get(&self, _url: &str) -> crate::Result<HttpResponse> {
            self.probes.fetch_add(1, Ordering::SeqCst);
            let answer = *self.answer.lock().unwrap();
            match answer {
                ProbeAnswer::Ok200 => Ok(HttpResponse {
                    status: 200,
                    body: r#"{"status":"ok"}"#.to_string(),
                }),
                ProbeAnswer::Status(status) => Ok(HttpResponse {
                    status,
                    body: String::new(),
                }),
                ProbeAnswer::TransportError => {
                    Err(crate::SentinelError::Http("connection refused".to_string()))
                }
                ProbeAnswer::Hang => std::future::pending().await,
            }
        }

        async fn put_form(
            &self,
            _url: &str,
            _params: &[(&str, &str)],
        ) -> crate::Result<HttpResponse> {
            unreachable!("health probes never PUT")
        }

        async fn post_form(
            &self,
            _url: &str,
            _params: &[(&str, &str)],
        ) -> crate::Result<HttpResponse> {
            unreachable!("health probes never POST")
        }
    }

    /// Records the (virtual) instant of every restart-command run.
    #[derive(Debug, Default)]
    struct RecordingRunner {
        fails: bool,
        calls: Mutex<Vec<Instant>>,
    }

    impl RecordingRunner {
        fn call_instants(&self) -> Vec<Instant> {
            self.calls.lock().unwrap().clone()
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl Restarter for RecordingRunner {
        async fn restart(&self, _command: &str, _budget: Duration) -> crate::Result<()> {
            self.calls.lock().unwrap().push(Instant::now());
            if self.fails {
                Err(crate::SentinelError::Monitor(
                    "`restart-cmd` exited with 1".to_string(),
                ))
            } else {
                Ok(())
            }
        }
    }

    #[derive(Debug, Default)]
    struct RecordingNotifier {
        sent: Mutex<Vec<(i8, String)>>,
    }

    impl RecordingNotifier {
        fn sent(&self) -> Vec<(i8, String)> {
            self.sent.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Notifier for RecordingNotifier {
        fn type_name(&self) -> &str {
            "recorder"
        }

        async fn notify(&self, notification: &Notification) -> crate::Result<()> {
            self.sent
                .lock()
                .unwrap()
                .push((notification.priority, notification.message.clone()));
            Ok(())
        }
    }

    const SVC: &str = "svc";

    /// Fast timings: probe every 100ms, threshold 3, backoff 1s doubling to
    /// a 4s cap. With paused time the loop's schedule is exact: probes at
    /// t = 0, 100ms, 200ms, …; the first restart fires on the probe at
    /// t = 200ms (third consecutive failure).
    fn health_config() -> HealthConfig {
        HealthConfig {
            url: "http://svc:1/health".to_string(),
            poll_interval: Duration::from_millis(100),
            failure_threshold: 3,
            restart_backoff: Duration::from_secs(1),
            restart_backoff_max: Duration::from_secs(4),
        }
    }

    struct Fixture {
        http: Arc<ScriptedHttp>,
        runner: Arc<RecordingRunner>,
        notifier: Arc<RecordingNotifier>,
        restarts: Arc<RestartManager>,
        state: StateHandle,
        cancel: CancellationToken,
        handle: tokio::task::JoinHandle<()>,
    }

    impl Fixture {
        /// Spawn a supervisor over a scripted probe answer. `restartable`
        /// mirrors whether the service has a `restart_command`.
        fn spawn(answer: ProbeAnswer, restartable: bool, runner: RecordingRunner) -> Self {
            let http = ScriptedHttp::new(answer);
            let runner = Arc::new(runner);
            let service = ServiceConfig {
                base_url: None,
                device_number: 0,
                restart_command: restartable.then(|| "restart-cmd".to_string()),
                health_command: None,
                max_restart_duration: Duration::from_secs(1),
                health: Some(health_config()),
            };
            let restarts = Arc::new(RestartManager::new(
                HashMap::from([(SVC.to_string(), service)]),
                Arc::clone(&runner) as Arc<dyn Restarter>,
            ));
            let notifier = Arc::new(RecordingNotifier::default());
            let state = new_state_handle(
                vec![],
                vec![(SVC.to_string(), Duration::from_millis(100))],
                100,
            );
            let supervisor = ServiceHealthSupervisor::new(
                SVC.to_string(),
                health_config(),
                restartable,
                Arc::clone(&http) as Arc<dyn HttpClient>,
                Arc::clone(&restarts),
                vec![Arc::clone(&notifier) as Arc<dyn Notifier>],
                Arc::clone(&state),
            );
            let cancel = CancellationToken::new();
            let handle = tokio::spawn({
                let cancel = cancel.clone();
                async move { supervisor.run(cancel).await }
            });
            Self {
                http,
                runner,
                notifier,
                restarts,
                state,
                cancel,
                handle,
            }
        }

        async fn stop(self) {
            self.cancel.cancel();
            self.handle.await.unwrap();
        }

        async fn service_status(&self) -> ServiceHealthStatus {
            self.state.read().await.services[0].clone()
        }
    }

    /// Advance virtual time; with `start_paused` the clock jumps timer to
    /// timer, so the supervisor's schedule stays exact.
    async fn run_until(offset: Duration) {
        tokio::time::sleep(offset).await;
    }

    #[tokio::test(start_paused = true)]
    async fn healthy_probe_publishes_up() {
        let f = Fixture::spawn(ProbeAnswer::Ok200, true, RecordingRunner::default());
        run_until(Duration::from_millis(50)).await;
        let status = f.service_status().await;
        assert_eq!(status.health, ServiceHealth::Up);
        assert_eq!(status.consecutive_failures, 0);
        assert!(status.last_probe_epoch_ms > 0);
        f.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn non_200_probe_publishes_down() {
        let f = Fixture::spawn(ProbeAnswer::Status(503), true, RecordingRunner::default());
        run_until(Duration::from_millis(50)).await;
        let status = f.service_status().await;
        assert_eq!(status.health, ServiceHealth::Down);
        assert_eq!(status.consecutive_failures, 1);
        f.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn transport_error_probe_publishes_down() {
        let f = Fixture::spawn(
            ProbeAnswer::TransportError,
            true,
            RecordingRunner::default(),
        );
        run_until(Duration::from_millis(50)).await;
        assert_eq!(f.service_status().await.health, ServiceHealth::Down);
        f.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn hanging_probe_times_out_as_down() {
        let f = Fixture::spawn(ProbeAnswer::Hang, true, RecordingRunner::default());
        // The probe blocks until the 2s PROBE_TIMEOUT fires.
        run_until(Duration::from_millis(2050)).await;
        let status = f.service_status().await;
        assert_eq!(status.health, ServiceHealth::Down);
        assert_eq!(f.http.probes(), 1, "one probe, timed out");
        f.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn below_threshold_never_restarts() {
        let f = Fixture::spawn(ProbeAnswer::Status(503), true, RecordingRunner::default());
        // Probes at t=0 and t=100ms — two failures, threshold is three.
        run_until(Duration::from_millis(150)).await;
        assert_eq!(f.runner.call_count(), 0);
        assert_eq!(f.service_status().await.consecutive_failures, 2);
        assert!(f.notifier.sent().is_empty());
        f.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn threshold_triggers_restart_and_notifies() {
        let f = Fixture::spawn(ProbeAnswer::Status(503), true, RecordingRunner::default());
        // Third failure at t=200ms triggers the restart.
        run_until(Duration::from_millis(250)).await;
        assert_eq!(f.runner.call_count(), 1);

        let sent = f.notifier.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, 0, "first restart of an outage is priority 0");
        assert!(
            sent[0].1.contains("restarted autonomously"),
            "{}",
            sent[0].1
        );
        assert!(
            sent[0].1.contains("status ok, recovery skipped"),
            "{}",
            sent[0].1
        );

        let status = f.service_status().await;
        assert_eq!(status.restarts_in_outage, 1);
        assert_eq!(status.total_restarts, 1);
        assert!(
            status.next_restart_epoch_ms.is_some(),
            "a next attempt must be scheduled"
        );

        let state = f.state.read().await;
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.history[0].monitor_name, SVC);
        assert!(state.history[0].success);
        drop(state);
        f.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn restart_attempts_double_backoff_and_cap() {
        let f = Fixture::spawn(ProbeAnswer::Status(503), true, RecordingRunner::default());
        // Restarts at t=0.2s, then after 1s, 2s, 4s, 4s (cap): 3.2s → 7.2s.
        run_until(Duration::from_millis(7300)).await;
        let calls = f.runner.call_instants();
        assert_eq!(calls.len(), 4, "restarts at 0.2s, 1.2s, 3.2s, 7.2s");
        let spacings: Vec<Duration> = calls.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(
            spacings,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
            ],
            "backoff doubles from 1s and caps at 4s"
        );
        // Probing continued at the poll cadence throughout: one probe per
        // 100ms tick from t=0 through t=7.2s inclusive, +1 by 7.3s.
        assert!(
            f.http.probes() >= 73,
            "probing must not pause during backoff (saw {})",
            f.http.probes()
        );
        f.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn second_restart_escalates_priority() {
        let f = Fixture::spawn(ProbeAnswer::Status(503), true, RecordingRunner::default());
        // First restart at t=0.2s, second at t=1.2s.
        run_until(Duration::from_millis(1250)).await;
        let sent = f.notifier.sent();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1].0, 1, "later restarts escalate to priority 1");
        assert!(
            sent[1]
                .1
                .contains("still unhealthy after 2 autonomous restarts"),
            "{}",
            sent[1].1
        );
        assert!(
            sent[1].1.contains("next attempt not before 2s"),
            "{}",
            sent[1].1
        );
        f.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn failed_restart_command_still_counts_and_notifies() {
        let f = Fixture::spawn(
            ProbeAnswer::Status(503),
            true,
            RecordingRunner {
                fails: true,
                ..RecordingRunner::default()
            },
        );
        run_until(Duration::from_millis(250)).await;
        let sent = f.notifier.sent();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].1.contains("status failed"), "{}", sent[0].1);
        assert!(sent[0].1.contains("exited with 1"), "{}", sent[0].1);
        let status = f.service_status().await;
        assert_eq!(
            status.restarts_in_outage, 1,
            "a failed attempt still counts"
        );
        assert!(status.next_restart_epoch_ms.is_some());
        f.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn recovery_resets_counters_and_backoff_without_notifying() {
        let f = Fixture::spawn(ProbeAnswer::Status(503), true, RecordingRunner::default());
        // One restart (t=0.2s), then the service comes back.
        run_until(Duration::from_millis(250)).await;
        f.http.set(ProbeAnswer::Ok200);
        run_until(Duration::from_millis(100)).await; // healthy probe at t=300ms
        let status = f.service_status().await;
        assert_eq!(status.health, ServiceHealth::Up);
        assert_eq!(status.restarts_in_outage, 0, "outage counter resets");
        assert_eq!(status.total_restarts, 1, "lifetime counter does not");
        assert_eq!(status.next_restart_epoch_ms, None);
        assert_eq!(
            f.notifier.sent().len(),
            1,
            "recovery itself must not notify"
        );

        // A fresh outage starts from scratch: threshold anew, initial backoff,
        // and its first restart is priority 0 again.
        f.http.set(ProbeAnswer::Status(503));
        run_until(Duration::from_millis(350)).await; // fails at 0.4/0.5/0.6s → restart
        assert_eq!(f.runner.call_count(), 2);
        let sent = f.notifier.sent();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1].0, 0, "new outage restarts at priority 0");
        assert!(
            sent[1].1.contains("restarted autonomously"),
            "{}",
            sent[1].1
        );
        f.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn already_in_flight_is_silent() {
        let f = Fixture::spawn(ProbeAnswer::Status(503), true, RecordingRunner::default());
        // Another restart path holds the service's slot across the threshold.
        let slot = f.restarts.gate().try_acquire(SVC).unwrap();
        run_until(Duration::from_millis(250)).await;
        assert_eq!(f.runner.call_count(), 0);
        assert!(f.notifier.sent().is_empty(), "AlreadyInFlight is silent");
        assert_eq!(f.service_status().await.restarts_in_outage, 0);

        // Slot released: the next failing probe restarts normally (nothing
        // was scheduled, so no backoff wait applies).
        drop(slot);
        run_until(Duration::from_millis(100)).await;
        assert_eq!(f.runner.call_count(), 1);
        f.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn non_restartable_notifies_once_per_outage() {
        let f = Fixture::spawn(ProbeAnswer::Status(503), false, RecordingRunner::default());
        run_until(Duration::from_millis(1050)).await; // 11 failed probes
        assert_eq!(f.runner.call_count(), 0, "never restarts");
        let sent = f.notifier.sent();
        assert_eq!(sent.len(), 1, "exactly one notification per outage");
        assert_eq!(sent[0].0, 1);
        assert!(
            sent[0].1.contains("manual intervention required"),
            "{}",
            sent[0].1
        );
        f.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_stops_the_loop() {
        let f = Fixture::spawn(ProbeAnswer::Ok200, true, RecordingRunner::default());
        run_until(Duration::from_millis(50)).await;
        f.stop().await; // stop() awaits the join handle — hangs if run() leaks
    }
}
