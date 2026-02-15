//! Shared state for monitor statuses and notification history

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

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
}

/// Shared state accessible by engine and dashboard
#[derive(Debug)]
pub struct SharedState {
    pub monitors: Vec<MonitorStatus>,
    pub history: VecDeque<NotificationRecord>,
    pub history_max_size: usize,
    pub started_at: Instant,
}

impl SharedState {
    pub fn new(monitor_names: Vec<String>, history_max_size: usize) -> Self {
        let monitors = monitor_names
            .into_iter()
            .map(|name| MonitorStatus {
                name,
                state: MonitorState::Unknown,
                last_poll_epoch_ms: 0,
                last_change_epoch_ms: None,
                consecutive_errors: 0,
            })
            .collect();

        Self {
            monitors,
            history: VecDeque::with_capacity(history_max_size),
            history_max_size,
            started_at: Instant::now(),
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

pub fn new_state_handle(monitor_names: Vec<String>, history_max_size: usize) -> StateHandle {
    Arc::new(RwLock::new(SharedState::new(
        monitor_names,
        history_max_size,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_has_unknown_monitors() {
        let state = SharedState::new(vec!["m1".to_string(), "m2".to_string()], 10);
        assert_eq!(state.monitors.len(), 2);
        assert_eq!(state.monitors[0].state, MonitorState::Unknown);
        assert_eq!(state.monitors[1].state, MonitorState::Unknown);
    }

    #[test]
    fn update_monitor_returns_true_on_change() {
        let mut state = SharedState::new(vec!["m1".to_string()], 10);
        let changed = state.update_monitor("m1", MonitorState::Safe, 1000);
        assert!(changed);
        assert_eq!(state.monitors[0].state, MonitorState::Safe);
        assert_eq!(state.monitors[0].last_change_epoch_ms, Some(1000));
    }

    #[test]
    fn update_monitor_returns_false_on_same_state() {
        let mut state = SharedState::new(vec!["m1".to_string()], 10);
        state.update_monitor("m1", MonitorState::Safe, 1000);
        let changed = state.update_monitor("m1", MonitorState::Safe, 2000);
        assert!(!changed);
        assert_eq!(state.monitors[0].last_poll_epoch_ms, 2000);
        assert_eq!(state.monitors[0].last_change_epoch_ms, Some(1000));
    }

    #[test]
    fn update_unknown_increments_error_count() {
        let mut state = SharedState::new(vec!["m1".to_string()], 10);
        state.update_monitor("m1", MonitorState::Unknown, 1000);
        assert_eq!(state.monitors[0].consecutive_errors, 1);
        state.update_monitor("m1", MonitorState::Unknown, 2000);
        assert_eq!(state.monitors[0].consecutive_errors, 2);
    }

    #[test]
    fn update_resets_error_count_on_recovery() {
        let mut state = SharedState::new(vec!["m1".to_string()], 10);
        state.update_monitor("m1", MonitorState::Unknown, 1000);
        state.update_monitor("m1", MonitorState::Unknown, 2000);
        assert_eq!(state.monitors[0].consecutive_errors, 2);
        state.update_monitor("m1", MonitorState::Safe, 3000);
        assert_eq!(state.monitors[0].consecutive_errors, 0);
    }

    #[test]
    fn update_unknown_monitor_returns_false() {
        let mut state = SharedState::new(vec!["m1".to_string()], 10);
        let changed = state.update_monitor("nonexistent", MonitorState::Safe, 1000);
        assert!(!changed);
    }

    #[test]
    fn history_respects_max_size() {
        let mut state = SharedState::new(vec![], 2);
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
        let mut state = SharedState::new(vec!["m1".to_string()], 10);
        assert_eq!(state.get_monitor_state("m1"), Some(MonitorState::Unknown));
        state.update_monitor("m1", MonitorState::Safe, 1000);
        assert_eq!(state.get_monitor_state("m1"), Some(MonitorState::Safe));
        assert_eq!(state.get_monitor_state("nonexistent"), None);
    }
}
