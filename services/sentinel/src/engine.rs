//! Engine: orchestrates monitors, transitions, and notifiers

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio_util::sync::CancellationToken;

use crate::config::{Config, TransitionConfig, TransitionDirection};
use crate::monitor::{Monitor, MonitorState};
use crate::notifier::{Notification, NotificationRecord, Notifier};
use crate::state::StateHandle;

/// The engine orchestrates polling monitors and dispatching notifications
pub struct Engine {
    monitors: Vec<Arc<dyn Monitor>>,
    notifiers: Vec<Arc<dyn Notifier>>,
    transitions: Vec<TransitionConfig>,
    state: StateHandle,
    cancel: CancellationToken,
}

impl Engine {
    pub fn new(
        monitors: Vec<Arc<dyn Monitor>>,
        notifiers: Vec<Arc<dyn Notifier>>,
        config: &Config,
        state: StateHandle,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            monitors,
            notifiers,
            transitions: config.transitions.clone(),
            state,
            cancel,
        }
    }

    /// Connect all monitors
    pub async fn connect_all(&self) {
        for monitor in &self.monitors {
            tracing::debug!("Connecting monitor '{}'", monitor.name());
            if let Err(e) = monitor.connect().await {
                tracing::warn!("Failed to connect monitor '{}': {}", monitor.name(), e);
            }
        }
    }

    /// Disconnect all monitors
    pub async fn disconnect_all(&self) {
        for monitor in &self.monitors {
            tracing::debug!("Disconnecting monitor '{}'", monitor.name());
            if let Err(e) = monitor.disconnect().await {
                tracing::warn!("Failed to disconnect monitor '{}': {}", monitor.name(), e);
            }
        }
    }

    /// Start polling all monitors. Returns when the cancellation token is triggered.
    pub async fn run(&self, polling_intervals: &[(String, Duration)]) {
        let mut handles = Vec::new();

        for (name, interval) in polling_intervals {
            let monitor = self.monitors.iter().find(|m| m.name() == name).cloned();

            if let Some(monitor) = monitor {
                let state = Arc::clone(&self.state);
                let transitions = self.transitions.clone();
                let notifiers: Vec<Arc<dyn Notifier>> = self.notifiers.clone();
                let cancel = self.cancel.clone();
                let interval = *interval;

                let handle = tokio::spawn(async move {
                    poll_loop(monitor, state, transitions, notifiers, interval, cancel).await;
                });
                handles.push(handle);
            }
        }

        // Wait for cancellation
        self.cancel.cancelled().await;

        // Wait for all polling tasks to finish
        for handle in handles {
            let _ = handle.await;
        }
    }
}

async fn poll_loop(
    monitor: Arc<dyn Monitor>,
    state: StateHandle,
    transitions: Vec<TransitionConfig>,
    notifiers: Vec<Arc<dyn Notifier>>,
    interval: Duration,
    cancel: CancellationToken,
) {
    loop {
        // Poll the monitor
        let new_state = monitor.poll().await;
        let now_ms = current_epoch_ms();
        let monitor_name = monitor.name().to_string();

        // Get the previous state and update
        let (changed, previous_state) = {
            let mut state_lock = state.write().await;
            let previous = state_lock.get_monitor_state(&monitor_name);
            let changed = state_lock.update_monitor(&monitor_name, new_state, now_ms);
            let errors = state_lock.get_monitor_consecutive_errors(&monitor_name);
            if errors == 5 {
                tracing::warn!(
                    "Monitor '{}' has {} consecutive errors",
                    monitor_name,
                    errors
                );
            }
            (changed, previous.unwrap_or(MonitorState::Unknown))
        };

        tracing::debug!(
            "Poll '{}': {:?} -> {:?} (changed={})",
            monitor_name,
            previous_state,
            new_state,
            changed
        );

        // If state changed, check transition rules and dispatch notifications
        if changed {
            dispatch_notifications(
                &monitor_name,
                previous_state,
                new_state,
                &transitions,
                &notifiers,
                &state,
                now_ms,
            )
            .await;
        }

        // Wait for the next poll or cancellation
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = cancel.cancelled() => {
                tracing::debug!("Polling loop for '{}' cancelled", monitor_name);
                break;
            }
        }
    }
}

/// Check transition rules and dispatch matching notifications
pub async fn dispatch_notifications(
    monitor_name: &str,
    previous: MonitorState,
    current: MonitorState,
    transitions: &[TransitionConfig],
    notifiers: &[Arc<dyn Notifier>],
    state: &StateHandle,
    now_ms: u64,
) {
    for transition in transitions {
        if transition.monitor_name != monitor_name {
            continue;
        }

        if !matches_direction(&transition.direction, previous, current) {
            continue;
        }

        let message = transition
            .message_template
            .replace("{monitor_name}", monitor_name)
            .replace("{new_state}", &current.to_string());

        let notification = Notification {
            title: String::new(),
            message: message.clone(),
            priority: transition.priority.unwrap_or(0),
            sound: transition.sound.clone(),
        };

        for notifier_type in &transition.notifiers {
            if let Some(notifier) = notifiers.iter().find(|n| n.type_name() == notifier_type) {
                tracing::debug!(
                    "Dispatching to '{}' for '{}': {}",
                    notifier_type,
                    monitor_name,
                    message
                );

                let result = notifier.notify(&notification).await;
                let record = NotificationRecord {
                    monitor_name: monitor_name.to_string(),
                    notifier_type: notifier_type.clone(),
                    message: message.clone(),
                    success: result.is_ok(),
                    error: result.as_ref().err().map(|e| e.to_string()),
                    timestamp_epoch_ms: now_ms,
                };

                if let Err(e) = &result {
                    tracing::warn!(
                        "Notification via '{}' for '{}' failed: {}",
                        notifier_type,
                        monitor_name,
                        e
                    );
                }

                state.write().await.add_notification(record);
            }
        }
    }
}

/// Check if a state transition matches a direction rule
pub fn matches_direction(
    direction: &TransitionDirection,
    previous: MonitorState,
    current: MonitorState,
) -> bool {
    match direction {
        TransitionDirection::SafeToUnsafe => {
            previous == MonitorState::Safe && current == MonitorState::Unsafe
        }
        TransitionDirection::UnsafeToSafe => {
            previous == MonitorState::Unsafe && current == MonitorState::Safe
        }
        TransitionDirection::Both => {
            (previous == MonitorState::Safe && current == MonitorState::Unsafe)
                || (previous == MonitorState::Unsafe && current == MonitorState::Safe)
        }
    }
}

fn current_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_to_unsafe_matches_correct_direction() {
        assert!(matches_direction(
            &TransitionDirection::SafeToUnsafe,
            MonitorState::Safe,
            MonitorState::Unsafe
        ));
        assert!(!matches_direction(
            &TransitionDirection::SafeToUnsafe,
            MonitorState::Unsafe,
            MonitorState::Safe
        ));
    }

    #[test]
    fn unsafe_to_safe_matches_correct_direction() {
        assert!(matches_direction(
            &TransitionDirection::UnsafeToSafe,
            MonitorState::Unsafe,
            MonitorState::Safe
        ));
        assert!(!matches_direction(
            &TransitionDirection::UnsafeToSafe,
            MonitorState::Safe,
            MonitorState::Unsafe
        ));
    }

    #[test]
    fn both_matches_either_direction() {
        assert!(matches_direction(
            &TransitionDirection::Both,
            MonitorState::Safe,
            MonitorState::Unsafe
        ));
        assert!(matches_direction(
            &TransitionDirection::Both,
            MonitorState::Unsafe,
            MonitorState::Safe
        ));
    }

    #[test]
    fn unknown_transitions_dont_match() {
        assert!(!matches_direction(
            &TransitionDirection::SafeToUnsafe,
            MonitorState::Unknown,
            MonitorState::Safe
        ));
        assert!(!matches_direction(
            &TransitionDirection::UnsafeToSafe,
            MonitorState::Unknown,
            MonitorState::Unsafe
        ));
        assert!(!matches_direction(
            &TransitionDirection::Both,
            MonitorState::Unknown,
            MonitorState::Safe
        ));
    }

    #[test]
    fn same_state_doesnt_match() {
        assert!(!matches_direction(
            &TransitionDirection::Both,
            MonitorState::Safe,
            MonitorState::Safe
        ));
    }

    #[tokio::test]
    async fn dispatch_sends_notification_on_matching_transition() {
        use crate::state::new_state_handle;

        let state = new_state_handle(vec!["m1".to_string()], 10);
        let transitions = vec![TransitionConfig {
            monitor_name: "m1".to_string(),
            direction: TransitionDirection::SafeToUnsafe,
            notifiers: vec!["test".to_string()],
            message_template: "{monitor_name} is now {new_state}".to_string(),
            priority: Some(1),
            sound: Some("siren".to_string()),
        }];

        let notifier = Arc::new(TestNotifier::new(true));
        let notifiers: Vec<Arc<dyn Notifier>> = vec![notifier.clone()];

        dispatch_notifications(
            "m1",
            MonitorState::Safe,
            MonitorState::Unsafe,
            &transitions,
            &notifiers,
            &state,
            1000,
        )
        .await;

        let state_lock = state.read().await;
        assert_eq!(state_lock.history.len(), 1);
        assert!(state_lock.history[0].success);
        assert_eq!(state_lock.history[0].message, "m1 is now Unsafe");
        assert_eq!(notifier.call_count().await, 1);
    }

    #[tokio::test]
    async fn dispatch_skips_non_matching_transition() {
        use crate::state::new_state_handle;

        let state = new_state_handle(vec!["m1".to_string()], 10);
        let transitions = vec![TransitionConfig {
            monitor_name: "m1".to_string(),
            direction: TransitionDirection::SafeToUnsafe,
            notifiers: vec!["test".to_string()],
            message_template: "alert".to_string(),
            priority: None,
            sound: None,
        }];

        let notifier = Arc::new(TestNotifier::new(true));
        let notifiers: Vec<Arc<dyn Notifier>> = vec![notifier.clone()];

        // This is unsafe -> safe, but rule is safe -> unsafe
        dispatch_notifications(
            "m1",
            MonitorState::Unsafe,
            MonitorState::Safe,
            &transitions,
            &notifiers,
            &state,
            1000,
        )
        .await;

        let state_lock = state.read().await;
        assert_eq!(state_lock.history.len(), 0);
        assert_eq!(notifier.call_count().await, 0);
    }

    #[tokio::test]
    async fn dispatch_records_failure() {
        use crate::state::new_state_handle;

        let state = new_state_handle(vec!["m1".to_string()], 10);
        let transitions = vec![TransitionConfig {
            monitor_name: "m1".to_string(),
            direction: TransitionDirection::SafeToUnsafe,
            notifiers: vec!["test".to_string()],
            message_template: "alert".to_string(),
            priority: None,
            sound: None,
        }];

        let notifier = Arc::new(TestNotifier::new(false));
        let notifiers: Vec<Arc<dyn Notifier>> = vec![notifier];

        dispatch_notifications(
            "m1",
            MonitorState::Safe,
            MonitorState::Unsafe,
            &transitions,
            &notifiers,
            &state,
            1000,
        )
        .await;

        let state_lock = state.read().await;
        assert_eq!(state_lock.history.len(), 1);
        assert!(!state_lock.history[0].success);
        assert!(state_lock.history[0].error.is_some());
    }

    /// A test notifier that can succeed or fail
    #[derive(Debug)]
    struct TestNotifier {
        succeed: bool,
        calls: Arc<tokio::sync::RwLock<u32>>,
    }

    impl TestNotifier {
        fn new(succeed: bool) -> Self {
            Self {
                succeed,
                calls: Arc::new(tokio::sync::RwLock::new(0)),
            }
        }

        async fn call_count(&self) -> u32 {
            *self.calls.read().await
        }
    }

    #[async_trait::async_trait]
    impl Notifier for TestNotifier {
        fn type_name(&self) -> &str {
            "test"
        }

        async fn notify(&self, _notification: &Notification) -> crate::Result<()> {
            *self.calls.write().await += 1;
            if self.succeed {
                Ok(())
            } else {
                Err(crate::SentinelError::Notifier("test failure".to_string()))
            }
        }
    }
}
