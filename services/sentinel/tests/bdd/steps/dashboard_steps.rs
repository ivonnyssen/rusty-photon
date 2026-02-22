//! BDD step definitions for dashboard feature

use axum::body::Body;
use axum::http::Request;
use cucumber::{given, then, when};
use tower::ServiceExt;

use sentinel::dashboard::build_router;
use sentinel::monitor::MonitorState;
use sentinel::notifier::NotificationRecord;
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

#[given(expr = "a monitor {string} with poll timestamp {int} in state {string}")]
async fn monitor_with_poll_data(
    world: &mut SentinelWorld,
    name: String,
    timestamp: u64,
    state_str: String,
) {
    let state = parse_state(&state_str);
    let handle = new_state_handle(vec![(name.clone(), 30000)], 10);
    {
        let mut s = handle.write().await;
        s.update_monitor(&name, state, timestamp);
    }
    world.dashboard_state = Some(handle);
}

#[given(expr = "a monitor {string} in the dashboard state")]
fn monitor_in_dashboard(world: &mut SentinelWorld, name: String) {
    let handle = new_state_handle(vec![(name, 30000)], 10);
    world.dashboard_state = Some(handle);
}

#[given(expr = "a notification record for {string} with message {string} that succeeded")]
async fn notification_succeeded(world: &mut SentinelWorld, monitor_name: String, message: String) {
    let handle = world.dashboard_state.as_ref().expect("state not set");
    let mut s = handle.write().await;
    s.add_notification(NotificationRecord {
        monitor_name,
        notifier_type: "pushover".to_string(),
        message,
        success: true,
        error: None,
        timestamp_epoch_ms: 1000,
    });
}

#[given(expr = "a notification record for {string} with message {string} that failed")]
async fn notification_failed(world: &mut SentinelWorld, monitor_name: String, message: String) {
    let handle = world.dashboard_state.as_ref().expect("state not set");
    let mut s = handle.write().await;
    s.add_notification(NotificationRecord {
        monitor_name,
        notifier_type: "pushover".to_string(),
        message,
        success: false,
        error: Some("timeout".to_string()),
        timestamp_epoch_ms: 2000,
    });
}

#[when("the dashboard index page is requested")]
async fn request_index(world: &mut SentinelWorld) {
    let state = world
        .dashboard_state
        .as_ref()
        .expect("state not set")
        .clone();
    let app = build_router(state);
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    world.dashboard_response_body = Some(String::from_utf8(body.to_vec()).unwrap());
}

#[then(expr = "the response should contain {string}")]
fn response_contains(world: &mut SentinelWorld, expected: String) {
    let body = world
        .dashboard_response_body
        .as_ref()
        .expect("no response body");
    assert!(
        body.contains(&expected),
        "Expected response to contain '{}', but it didn't.\nResponse body:\n{}",
        expected,
        body
    );
}

#[then(expr = "the response should contain a time script for epoch {int}")]
fn response_contains_time_script(world: &mut SentinelWorld, epoch: u64) {
    let body = world
        .dashboard_response_body
        .as_ref()
        .expect("no response body");
    let expected = format!(
        "<script>document.write(new Date({}).toLocaleTimeString())</script>",
        epoch
    );
    assert!(
        body.contains(&expected),
        "Expected response to contain time script for epoch {}, but it didn't.\nResponse body:\n{}",
        epoch,
        body
    );
}
