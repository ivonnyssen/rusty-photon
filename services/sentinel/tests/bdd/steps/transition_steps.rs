//! BDD step definitions for transition feature

use std::sync::Arc;

use cucumber::{given, then, when};
use tokio::sync::RwLock;

use sentinel::config::{TransitionConfig, TransitionDirection};
use sentinel::monitor::MonitorState;
use sentinel::notifier::{Notification, Notifier};
use sentinel::state::new_state_handle;

use crate::world::SentinelWorld;

fn parse_state(s: &str) -> MonitorState {
    match s {
        "Safe" => MonitorState::Safe,
        "Unsafe" => MonitorState::Unsafe,
        "Unknown" => MonitorState::Unknown,
        other => panic!("Unknown state: {}", other),
    }
}

fn parse_direction(s: &str) -> TransitionDirection {
    match s {
        "safe_to_unsafe" => TransitionDirection::SafeToUnsafe,
        "unsafe_to_safe" => TransitionDirection::UnsafeToSafe,
        "both" => TransitionDirection::Both,
        other => panic!("Unknown direction: {}", other),
    }
}

/// A test notifier that always fails
#[derive(Debug)]
struct FailingNotifier {
    type_name: String,
}

impl FailingNotifier {
    fn new(type_name: &str) -> Self {
        Self {
            type_name: type_name.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl Notifier for FailingNotifier {
    fn type_name(&self) -> &str {
        &self.type_name
    }

    async fn notify(&self, _notification: &Notification) -> sentinel::Result<()> {
        Err(sentinel::SentinelError::Notifier(
            "test failure".to_string(),
        ))
    }
}

/// A test notifier that records notifications
#[derive(Debug)]
struct RecordingNotifier {
    type_name: String,
    records: Arc<RwLock<Vec<Notification>>>,
}

impl RecordingNotifier {
    fn new(type_name: &str) -> Self {
        Self {
            type_name: type_name.to_string(),
            records: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

#[async_trait::async_trait]
impl Notifier for RecordingNotifier {
    fn type_name(&self) -> &str {
        &self.type_name
    }

    async fn notify(&self, notification: &Notification) -> sentinel::Result<()> {
        self.records.write().await.push(notification.clone());
        Ok(())
    }
}

#[given(expr = "a monitor named {string} in state {string}")]
async fn monitor_in_state(world: &mut SentinelWorld, name: String, state_str: String) {
    let state = parse_state(&state_str);
    world.transition_monitor_name = Some(name.clone());
    world.transition_initial_state = Some(state);

    let state_handle = new_state_handle(vec![(name.clone(), 30000)], 100);
    {
        let mut s = state_handle.write().await;
        s.update_monitor(&name, state, 0);
    }
    world.transition_state = Some(state_handle);
}

#[given(expr = "a transition rule for {string} on {string} via {string}")]
fn transition_rule(
    world: &mut SentinelWorld,
    monitor_name: String,
    direction_str: String,
    notifier_type: String,
) {
    let direction = parse_direction(&direction_str);
    let transition = TransitionConfig {
        monitor_name,
        direction,
        notifiers: vec![notifier_type.clone()],
        message_template: "{monitor_name} is now {new_state}".to_string(),
        priority: None,
        sound: None,
    };
    world
        .transition_rules
        .get_or_insert_with(Vec::new)
        .push(transition);

    // Create the notifier if not already present
    if world.transition_notifiers.is_none() {
        world.transition_notifiers = Some(Vec::new());
    }
    let notifiers = world.transition_notifiers.as_mut().unwrap();
    if !notifiers.iter().any(|n| n.type_name() == notifier_type) {
        let notifier: Arc<dyn Notifier> = Arc::new(RecordingNotifier::new(&notifier_type));
        world.transition_recording_notifier = Some(Arc::clone(&notifier));
        notifiers.push(notifier);
    }
}

#[when(expr = "the monitor transitions to {string}")]
async fn monitor_transitions(world: &mut SentinelWorld, new_state_str: String) {
    let new_state = parse_state(&new_state_str);
    let monitor_name = world
        .transition_monitor_name
        .as_ref()
        .expect("monitor name not set")
        .clone();
    let initial_state = world
        .transition_initial_state
        .expect("initial state not set");
    let state_handle = world
        .transition_state
        .as_ref()
        .expect("state handle not set");
    let transitions = world.transition_rules.as_ref().cloned().unwrap_or_default();
    let notifiers: Vec<Arc<dyn Notifier>> = world
        .transition_notifiers
        .as_ref()
        .map(|n| n.iter().map(|x| x.clone() as Arc<dyn Notifier>).collect())
        .unwrap_or_default();

    // Update state
    let changed = {
        let mut s = state_handle.write().await;
        s.update_monitor(&monitor_name, new_state, 1000)
    };

    // Dispatch if changed
    if changed {
        sentinel::engine::dispatch_notifications(
            &monitor_name,
            initial_state,
            new_state,
            &transitions,
            &notifiers,
            state_handle,
            1000,
        )
        .await;
    }
}

#[then("a notification should be dispatched")]
async fn notification_dispatched(world: &mut SentinelWorld) {
    let state_handle = world.transition_state.as_ref().unwrap();
    let state = state_handle.read().await;
    assert!(
        !state.history.is_empty(),
        "Expected at least one notification in history"
    );
}

#[then("no notification should be dispatched")]
async fn no_notification_dispatched(world: &mut SentinelWorld) {
    let state_handle = world.transition_state.as_ref().unwrap();
    let state = state_handle.read().await;
    assert!(
        state.history.is_empty(),
        "Expected no notifications in history, but found {}",
        state.history.len()
    );
}

#[given(expr = "a notifier {string} that returns errors")]
fn failing_notifier(world: &mut SentinelWorld, notifier_type: String) {
    let notifiers = world
        .transition_notifiers
        .as_mut()
        .expect("notifiers not set");
    // Replace any existing notifier with the same type name
    notifiers.retain(|n| n.type_name() != notifier_type);
    let notifier: Arc<dyn Notifier> = Arc::new(FailingNotifier::new(&notifier_type));
    notifiers.push(notifier);
}

#[then(expr = "the notification message should contain {string}")]
async fn notification_contains(world: &mut SentinelWorld, expected: String) {
    let state_handle = world.transition_state.as_ref().unwrap();
    let state = state_handle.read().await;
    let last = state.history.back().expect("no notification in history");
    assert!(
        last.message.contains(&expected),
        "Expected message to contain '{}', got '{}'",
        expected,
        last.message
    );
}

#[then("the notification should be marked as failed")]
async fn notification_marked_failed(world: &mut SentinelWorld) {
    let state_handle = world.transition_state.as_ref().unwrap();
    let state = state_handle.read().await;
    let last = state.history.back().expect("no notification in history");
    assert!(
        !last.success,
        "Expected notification to be marked as failed, but it was marked as successful"
    );
    assert!(
        last.error.is_some(),
        "Expected notification to have an error message, but it was None"
    );
}
