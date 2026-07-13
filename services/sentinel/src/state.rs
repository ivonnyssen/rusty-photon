//! Shared state for monitor statuses and notification history

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::monitor::MonitorState;
use crate::notifier::NotificationRecord;

/// Status of a single monitor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorStatus {
    pub name: String,
    pub state: MonitorState,
    pub last_poll_epoch_ms: u64,
    pub last_change_epoch_ms: Option<u64>,
    pub consecutive_errors: u32,
    #[serde(with = "humantime_serde")]
    pub polling_interval: Duration,
}

/// Health of a supervised service as seen by its health supervisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceHealth {
    /// Not probed yet.
    Unknown,
    /// Last probe answered a clean 200.
    Up,
    /// Last probe failed (non-200, timeout, or connection error).
    Down,
}

impl std::fmt::Display for ServiceHealth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceHealth::Unknown => write!(f, "Unknown"),
            ServiceHealth::Up => write!(f, "Up"),
            ServiceHealth::Down => write!(f, "Down"),
        }
    }
}

/// Snapshot of one supervised service, published by its health supervisor
/// after every probe (single writer per service).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceHealthStatus {
    pub name: String,
    pub health: ServiceHealth,
    /// 0 until the first probe completes.
    pub last_probe_epoch_ms: u64,
    pub consecutive_failures: u32,
    /// Autonomous restarts in the current outage; resets on recovery.
    pub restarts_in_outage: u32,
    /// Autonomous restarts since sentinel started.
    pub total_restarts: u64,
    /// Backoff-scheduled earliest next autonomous restart; `None` when none
    /// is scheduled.
    pub next_restart_epoch_ms: Option<u64>,
    #[serde(with = "humantime_serde")]
    pub poll_interval: Duration,
}

impl ServiceHealthStatus {
    /// The unseeded/unprobed snapshot every supervised service starts from.
    pub fn unknown(name: String, poll_interval: Duration) -> Self {
        Self {
            name,
            health: ServiceHealth::Unknown,
            last_probe_epoch_ms: 0,
            consecutive_failures: 0,
            restarts_in_outage: 0,
            total_restarts: 0,
            next_restart_epoch_ms: None,
            poll_interval,
        }
    }
}

/// Shared state accessible by engine and dashboard
#[derive(Debug)]
pub struct SharedState {
    pub monitors: Vec<MonitorStatus>,
    pub services: Vec<ServiceHealthStatus>,
    pub history: VecDeque<NotificationRecord>,
    pub history_max_size: usize,
    pub started_at: Instant,
}

impl SharedState {
    pub fn new(
        monitors_with_intervals: Vec<(String, Duration)>,
        supervised_services: Vec<(String, Duration)>,
        history_max_size: usize,
    ) -> Self {
        let monitors = monitors_with_intervals
            .into_iter()
            .map(|(name, polling_interval)| MonitorStatus {
                name,
                state: MonitorState::Unknown,
                last_poll_epoch_ms: 0,
                last_change_epoch_ms: None,
                consecutive_errors: 0,
                polling_interval,
            })
            .collect();

        let services = supervised_services
            .into_iter()
            .map(|(name, poll_interval)| ServiceHealthStatus::unknown(name, poll_interval))
            .collect();

        Self {
            monitors,
            services,
            history: VecDeque::with_capacity(history_max_size),
            history_max_size,
            started_at: Instant::now(),
        }
    }

    /// Replace a supervised service's snapshot (matched by name). The
    /// supervisor's local state machine is authoritative; this is push-only.
    /// An unseeded name is inserted defensively.
    pub fn set_service_health(&mut self, status: ServiceHealthStatus) {
        match self.services.iter_mut().find(|s| s.name == status.name) {
            Some(slot) => *slot = status,
            None => self.services.push(status),
        }
    }

    /// Update a monitor's state, returning true if the state changed
    pub fn update_monitor(&mut self, name: &str, new_state: MonitorState, now_ms: u64) -> bool {
        if let Some(status) = self.monitors.iter_mut().find(|m| m.name == name) {
            let changed = status.state != new_state;
            status.state = new_state;
            status.last_poll_epoch_ms = now_ms;
            if new_state == MonitorState::Unknown {
                status.consecutive_errors += 1;
            } else {
                status.consecutive_errors = 0;
            }
            if changed {
                status.last_change_epoch_ms = Some(now_ms);
            }
            changed
        } else {
            false
        }
    }

    /// Get a monitor's current state
    pub fn get_monitor_state(&self, name: &str) -> Option<MonitorState> {
        self.monitors
            .iter()
            .find(|m| m.name == name)
            .map(|m| m.state)
    }

    /// Get a monitor's previous state (before last update) — uses current state as proxy
    pub fn get_monitor_consecutive_errors(&self, name: &str) -> u32 {
        self.monitors
            .iter()
            .find(|m| m.name == name)
            .map(|m| m.consecutive_errors)
            .unwrap_or(0)
    }

    /// Add a notification to history
    pub fn add_notification(&mut self, record: NotificationRecord) {
        if self.history.len() >= self.history_max_size {
            self.history.pop_front();
        }
        self.history.push_back(record);
    }
}

/// Thread-safe shared state handle
pub type StateHandle = Arc<RwLock<SharedState>>;

pub fn new_state_handle(
    monitors_with_intervals: Vec<(String, Duration)>,
    supervised_services: Vec<(String, Duration)>,
    history_max_size: usize,
) -> StateHandle {
    Arc::new(RwLock::new(SharedState::new(
        monitors_with_intervals,
        supervised_services,
        history_max_size,
    )))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn new_state_has_unknown_monitors() {
        let state = SharedState::new(
            vec![
                ("m1".to_string(), Duration::from_secs(30)),
                ("m2".to_string(), Duration::from_secs(60)),
            ],
            vec![],
            10,
        );
        assert_eq!(state.monitors.len(), 2);
        assert_eq!(state.monitors[0].state, MonitorState::Unknown);
        assert_eq!(state.monitors[0].polling_interval, Duration::from_secs(30));
        assert_eq!(state.monitors[1].state, MonitorState::Unknown);
        assert_eq!(state.monitors[1].polling_interval, Duration::from_secs(60));
    }

    #[test]
    fn update_monitor_returns_true_on_change() {
        let mut state = SharedState::new(
            vec![("m1".to_string(), Duration::from_secs(30))],
            vec![],
            10,
        );
        let changed = state.update_monitor("m1", MonitorState::Safe, 1000);
        assert!(changed);
        assert_eq!(state.monitors[0].state, MonitorState::Safe);
        assert_eq!(state.monitors[0].last_change_epoch_ms, Some(1000));
    }

    #[test]
    fn update_monitor_returns_false_on_same_state() {
        let mut state = SharedState::new(
            vec![("m1".to_string(), Duration::from_secs(30))],
            vec![],
            10,
        );
        state.update_monitor("m1", MonitorState::Safe, 1000);
        let changed = state.update_monitor("m1", MonitorState::Safe, 2000);
        assert!(!changed);
        assert_eq!(state.monitors[0].last_poll_epoch_ms, 2000);
        assert_eq!(state.monitors[0].last_change_epoch_ms, Some(1000));
    }

    #[test]
    fn update_unknown_increments_error_count() {
        let mut state = SharedState::new(
            vec![("m1".to_string(), Duration::from_secs(30))],
            vec![],
            10,
        );
        state.update_monitor("m1", MonitorState::Unknown, 1000);
        assert_eq!(state.monitors[0].consecutive_errors, 1);
        state.update_monitor("m1", MonitorState::Unknown, 2000);
        assert_eq!(state.monitors[0].consecutive_errors, 2);
    }

    #[test]
    fn update_resets_error_count_on_recovery() {
        let mut state = SharedState::new(
            vec![("m1".to_string(), Duration::from_secs(30))],
            vec![],
            10,
        );
        state.update_monitor("m1", MonitorState::Unknown, 1000);
        state.update_monitor("m1", MonitorState::Unknown, 2000);
        assert_eq!(state.monitors[0].consecutive_errors, 2);
        state.update_monitor("m1", MonitorState::Safe, 3000);
        assert_eq!(state.monitors[0].consecutive_errors, 0);
    }

    #[test]
    fn update_unknown_monitor_returns_false() {
        let mut state = SharedState::new(
            vec![("m1".to_string(), Duration::from_secs(30))],
            vec![],
            10,
        );
        let changed = state.update_monitor("nonexistent", MonitorState::Safe, 1000);
        assert!(!changed);
    }

    #[test]
    fn history_respects_max_size() {
        let mut state = SharedState::new(vec![], vec![], 2);
        for i in 0..5 {
            state.add_notification(NotificationRecord {
                monitor_name: format!("m{}", i),
                notifier_type: "pushover".to_string(),
                message: format!("msg{}", i),
                success: true,
                error: None,
                timestamp_epoch_ms: i * 1000,
            });
        }
        assert_eq!(state.history.len(), 2);
        assert_eq!(state.history[0].monitor_name, "m3");
        assert_eq!(state.history[1].monitor_name, "m4");
    }

    #[test]
    fn get_monitor_state() {
        let mut state = SharedState::new(
            vec![("m1".to_string(), Duration::from_secs(30))],
            vec![],
            10,
        );
        assert_eq!(state.get_monitor_state("m1"), Some(MonitorState::Unknown));
        state.update_monitor("m1", MonitorState::Safe, 1000);
        assert_eq!(state.get_monitor_state("m1"), Some(MonitorState::Safe));
        assert_eq!(state.get_monitor_state("nonexistent"), None);
    }

    #[test]
    fn get_consecutive_errors_for_unknown_monitor_returns_zero() {
        let state = SharedState::new(
            vec![("m1".to_string(), Duration::from_secs(30))],
            vec![],
            10,
        );
        assert_eq!(state.get_monitor_consecutive_errors("nonexistent"), 0);
    }

    #[test]
    fn new_state_seeds_supervised_services_unknown() {
        let state = SharedState::new(
            vec![],
            vec![("plate-solver".to_string(), Duration::from_secs(30))],
            10,
        );
        assert_eq!(state.services.len(), 1);
        let service = &state.services[0];
        assert_eq!(service.name, "plate-solver");
        assert_eq!(service.health, ServiceHealth::Unknown);
        assert_eq!(service.last_probe_epoch_ms, 0);
        assert_eq!(service.consecutive_failures, 0);
        assert_eq!(service.restarts_in_outage, 0);
        assert_eq!(service.total_restarts, 0);
        assert_eq!(service.next_restart_epoch_ms, None);
        assert_eq!(service.poll_interval, Duration::from_secs(30));
    }

    #[test]
    fn set_service_health_replaces_by_name() {
        let mut state = SharedState::new(
            vec![],
            vec![("svc".to_string(), Duration::from_secs(30))],
            10,
        );
        let mut status = ServiceHealthStatus::unknown("svc".to_string(), Duration::from_secs(30));
        status.health = ServiceHealth::Down;
        status.consecutive_failures = 2;
        state.set_service_health(status);
        assert_eq!(state.services.len(), 1, "replace, not append");
        assert_eq!(state.services[0].health, ServiceHealth::Down);
        assert_eq!(state.services[0].consecutive_failures, 2);

        // An unseeded name is inserted defensively.
        let other = ServiceHealthStatus::unknown("other".to_string(), Duration::from_secs(5));
        state.set_service_health(other);
        assert_eq!(state.services.len(), 2);
    }
}
