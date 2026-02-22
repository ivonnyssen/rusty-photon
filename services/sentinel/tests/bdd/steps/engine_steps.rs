//! BDD step definitions for engine feature

use std::sync::Arc;
use std::time::Duration;

use cucumber::{given, then, when};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use sentinel::config::Config;
use sentinel::engine::Engine;
use sentinel::monitor::{Monitor, MonitorState};
use sentinel::state::new_state_handle;

use crate::world::SentinelWorld;

/// A test monitor with configurable behavior
#[derive(Debug)]
struct TestMonitor {
    monitor_name: String,
    state: MonitorState,
    connect_result: Result<(), String>,
    disconnect_result: Result<(), String>,
    poll_count: Arc<RwLock<u32>>,
}

impl TestMonitor {
    fn new(name: &str, state: MonitorState) -> Self {
        Self {
            monitor_name: name.to_string(),
            state,
            connect_result: Ok(()),
            disconnect_result: Ok(()),
            poll_count: Arc::new(RwLock::new(0)),
        }
    }

    fn with_connect_error(mut self, err: &str) -> Self {
        self.connect_result = Err(err.to_string());
        self
    }

    fn with_disconnect_error(mut self, err: &str) -> Self {
        self.disconnect_result = Err(err.to_string());
        self
    }
}

#[async_trait::async_trait]
impl Monitor for TestMonitor {
    fn name(&self) -> &str {
        &self.monitor_name
    }

    async fn poll(&self) -> MonitorState {
        *self.poll_count.write().await += 1;
        self.state
    }

    async fn connect(&self) -> sentinel::Result<()> {
        self.connect_result
            .clone()
            .map_err(sentinel::SentinelError::Monitor)
    }

    async fn disconnect(&self) -> sentinel::Result<()> {
        self.disconnect_result
            .clone()
            .map_err(sentinel::SentinelError::Monitor)
    }
}

#[given(expr = "monitors {string} and {string} that connect successfully")]
fn monitors_connect_ok(world: &mut SentinelWorld, name1: String, name2: String) {
    let m1 = Arc::new(TestMonitor::new(&name1, MonitorState::Safe));
    let m2 = Arc::new(TestMonitor::new(&name2, MonitorState::Safe));
    world
        .engine_monitors
        .get_or_insert_with(Vec::new)
        .extend([m1 as Arc<dyn Monitor>, m2]);
}

#[given(expr = "monitor {string} that fails to connect")]
fn monitor_fails_connect(world: &mut SentinelWorld, name: String) {
    let m = Arc::new(TestMonitor::new(&name, MonitorState::Safe).with_connect_error("boom"));
    world
        .engine_monitors
        .get_or_insert_with(Vec::new)
        .push(m as Arc<dyn Monitor>);
}

#[given(expr = "monitor {string} that connects successfully")]
fn monitor_connects_ok(world: &mut SentinelWorld, name: String) {
    let m = Arc::new(TestMonitor::new(&name, MonitorState::Safe));
    world
        .engine_monitors
        .get_or_insert_with(Vec::new)
        .push(m as Arc<dyn Monitor>);
}

#[given(expr = "monitors {string} and {string} that disconnect successfully")]
fn monitors_disconnect_ok(world: &mut SentinelWorld, name1: String, name2: String) {
    let m1 = Arc::new(TestMonitor::new(&name1, MonitorState::Safe));
    let m2 = Arc::new(TestMonitor::new(&name2, MonitorState::Safe));
    world
        .engine_monitors
        .get_or_insert_with(Vec::new)
        .extend([m1 as Arc<dyn Monitor>, m2]);
}

#[given(expr = "monitor {string} that fails to disconnect")]
fn monitor_fails_disconnect(world: &mut SentinelWorld, name: String) {
    let m = Arc::new(TestMonitor::new(&name, MonitorState::Safe).with_disconnect_error("boom"));
    world
        .engine_monitors
        .get_or_insert_with(Vec::new)
        .push(m as Arc<dyn Monitor>);
}

#[given(expr = "monitor {string} that disconnects successfully")]
fn monitor_disconnects_ok(world: &mut SentinelWorld, name: String) {
    let m = Arc::new(TestMonitor::new(&name, MonitorState::Safe));
    world
        .engine_monitors
        .get_or_insert_with(Vec::new)
        .push(m as Arc<dyn Monitor>);
}

#[given(expr = "a monitor {string} that reports {string}")]
fn monitor_reports_state(world: &mut SentinelWorld, name: String, state_str: String) {
    let state = match state_str.as_str() {
        "Safe" => MonitorState::Safe,
        "Unsafe" => MonitorState::Unsafe,
        "Unknown" => MonitorState::Unknown,
        other => panic!("Unknown state: {}", other),
    };
    let m = Arc::new(TestMonitor::new(&name, state));
    world
        .engine_monitors
        .get_or_insert_with(Vec::new)
        .push(m as Arc<dyn Monitor>);
}

#[given("the engine is configured with that monitor")]
fn engine_configured(world: &mut SentinelWorld) {
    let monitors = world.engine_monitors.as_ref().expect("no monitors set");
    let monitors_with_intervals: Vec<(String, u64)> = monitors
        .iter()
        .map(|m| (m.name().to_string(), 100))
        .collect();
    let state = new_state_handle(monitors_with_intervals, 10);
    world.engine_state = Some(state);
}

fn build_engine(world: &SentinelWorld) -> Engine {
    let monitors = world
        .engine_monitors
        .as_ref()
        .expect("no monitors set")
        .clone();
    let monitors_with_intervals: Vec<(String, u64)> = monitors
        .iter()
        .map(|m| (m.name().to_string(), 100))
        .collect();
    let state = world
        .engine_state
        .clone()
        .unwrap_or_else(|| new_state_handle(monitors_with_intervals, 10));
    let config = Config::default();
    let cancel = CancellationToken::new();
    Engine::new(monitors, vec![], &config, state, cancel)
}

#[when("the engine connects all monitors")]
async fn engine_connects_all(world: &mut SentinelWorld) {
    let engine = build_engine(world);
    engine.connect_all().await;
}

#[when("the engine disconnects all monitors")]
async fn engine_disconnects_all(world: &mut SentinelWorld) {
    let engine = build_engine(world);
    engine.disconnect_all().await;
}

#[when("the engine runs and is cancelled after a short delay")]
async fn engine_runs_and_cancels(world: &mut SentinelWorld) {
    let monitors = world
        .engine_monitors
        .as_ref()
        .expect("no monitors set")
        .clone();
    let state = world.engine_state.as_ref().expect("state not set").clone();
    let config = Config::default();
    let cancel = CancellationToken::new();

    let polling_intervals: Vec<(String, Duration)> = monitors
        .iter()
        .map(|m| (m.name().to_string(), Duration::from_millis(100)))
        .collect();

    let engine = Engine::new(monitors, vec![], &config, state, cancel.clone());

    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel_clone.cancel();
    });

    engine.run(&polling_intervals).await;
}

#[then("no errors should occur")]
fn no_errors(_world: &mut SentinelWorld) {
    // If we reached this step, no panics occurred — success
}

#[then(expr = "monitor {string} should have been polled")]
async fn monitor_was_polled(world: &mut SentinelWorld, name: String) {
    let monitors = world.engine_monitors.as_ref().expect("no monitors set");
    let monitor = monitors
        .iter()
        .find(|m| m.name() == name)
        .expect("monitor not found");

    // Downcast to check poll count - we verify through state instead
    let state = world.engine_state.as_ref().expect("state not set");
    let state_lock = state.read().await;
    let status = state_lock
        .monitors
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("monitor '{}' not in state", name));
    assert!(
        status.last_poll_epoch_ms > 0,
        "Monitor '{}' was never polled (last_poll_epoch_ms is 0)",
        monitor.name()
    );
}

#[then(expr = "the shared state should show {string} for {string}")]
async fn state_shows(world: &mut SentinelWorld, expected_state: String, name: String) {
    let expected = match expected_state.as_str() {
        "Safe" => MonitorState::Safe,
        "Unsafe" => MonitorState::Unsafe,
        "Unknown" => MonitorState::Unknown,
        other => panic!("Unknown state: {}", other),
    };
    let state = world.engine_state.as_ref().expect("state not set");
    let state_lock = state.read().await;
    assert_eq!(
        state_lock.get_monitor_state(&name),
        Some(expected),
        "Expected state {:?} for '{}', got {:?}",
        expected,
        name,
        state_lock.get_monitor_state(&name)
    );
}
