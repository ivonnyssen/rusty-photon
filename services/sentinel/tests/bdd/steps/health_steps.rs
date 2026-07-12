//! BDD step definitions for the service health supervision feature

use std::time::Duration;

use cucumber::{given, then, when};

use crate::world::SentinelWorld;

/// A shell command that appends one line to the marker file per run, so the
/// test can count restarts. Works under both platform shells.
fn append_marker_command(path: &std::path::Path) -> String {
    format!("echo ok >> \"{}\"", path.display())
}

#[given(expr = "a stub service whose health endpoint answers {int}")]
async fn health_stub_answering(world: &mut SentinelWorld, status: u16) {
    let healthy = match status {
        200 => true,
        503 => false,
        other => panic!("unsupported stub health status {other}"),
    };
    world.start_health_stub(healthy).await;
}

#[given(
    expr = "sentinel is running with service {string} supervised at the stub with a restart command that appends to a marker file"
)]
async fn sentinel_with_supervised_service(world: &mut SentinelWorld, name: String) {
    let marker = world.restart_marker_path();
    world.add_health_supervised_service(&name, Some(append_marker_command(&marker)));
    world.start_sentinel().await;
}

#[given(
    expr = "sentinel is running with notifiers and service {string} supervised at the stub with a restart command that appends to a marker file"
)]
async fn sentinel_with_notifiers_and_supervised_service(world: &mut SentinelWorld, name: String) {
    world.sentinel_has_notifiers = true;
    let marker = world.restart_marker_path();
    world.add_health_supervised_service(&name, Some(append_marker_command(&marker)));
    world.start_sentinel().await;
}

#[when(expr = "the dashboard reports service {string} health {string}")]
#[then(expr = "the dashboard reports service {string} health {string}")]
async fn dashboard_reports_service_health(
    world: &mut SentinelWorld,
    name: String,
    expected: String,
) {
    let last = world.wait_for_service_health(&name, &expected).await;
    assert_eq!(
        last.as_deref(),
        Some(expected.as_str()),
        "service '{name}' never reported health '{expected}'"
    );
}

#[then("the restart marker file does not exist after a settle period")]
async fn marker_does_not_exist(world: &mut SentinelWorld) {
    // A negative assertion needs a fixed window: five poll intervals is ample
    // time for a spurious restart (threshold is two failed probes) to appear.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let marker = world.restart_marker.as_ref().expect("no marker path");
    assert!(
        !marker.exists(),
        "healthy service was restarted: {}",
        marker.display()
    );
}

#[when(expr = "the restart marker file records at least {int} restart(s) within {int} seconds")]
#[then(expr = "the restart marker file records at least {int} restart(s) within {int} seconds")]
async fn marker_records_restarts(world: &mut SentinelWorld, min: usize, ceiling_secs: u64) {
    let count = world
        .wait_for_marker_lines(min, Duration::from_secs(ceiling_secs))
        .await;
    assert!(
        count >= min,
        "expected at least {min} restarts within {ceiling_secs}s, saw {count}"
    );
}

#[when("the stub service becomes healthy")]
fn stub_becomes_healthy(world: &mut SentinelWorld) {
    world
        .health_stub
        .as_ref()
        .expect("health stub not started")
        .set_healthy(true);
}

#[then(expr = "the dashboard reports zero restarts in the current outage for {string}")]
async fn zero_restarts_in_outage(world: &mut SentinelWorld, name: String) {
    let services = world.get_services().await;
    let service = services
        .iter()
        .find(|s| s["name"].as_str() == Some(name.as_str()))
        .unwrap_or_else(|| panic!("service '{name}' not in /api/services: {services:?}"));
    assert_eq!(
        service["restarts_in_outage"].as_u64(),
        Some(0),
        "outage counter did not reset: {service}"
    );
}

#[then(expr = "the notification history records an autonomous restart of {string}")]
async fn history_records_autonomous_restart(world: &mut SentinelWorld, name: String) {
    let history = world
        .wait_for_history(|record| {
            record["monitor_name"].as_str() == Some(name.as_str())
                && record["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("restarted autonomously"))
        })
        .await;
    assert!(
        history.iter().any(|record| {
            record["monitor_name"].as_str() == Some(name.as_str())
                && record["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("restarted autonomously"))
        }),
        "no autonomous-restart record for '{name}' in history: {history:?}"
    );
}

#[then(
    expr = "the notification history records an escalated still-unhealthy notification for {string}"
)]
async fn history_records_escalation(world: &mut SentinelWorld, name: String) {
    let history = world
        .wait_for_history(|record| {
            record["monitor_name"].as_str() == Some(name.as_str())
                && record["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("still unhealthy"))
        })
        .await;
    assert!(
        history.iter().any(|record| {
            record["monitor_name"].as_str() == Some(name.as_str())
                && record["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("still unhealthy"))
        }),
        "no still-unhealthy escalation for '{name}' in history: {history:?}"
    );
}

#[then(expr = "the dashboard reports a scheduled next restart for {string}")]
async fn dashboard_reports_next_restart(world: &mut SentinelWorld, name: String) {
    for _ in 0..60 {
        let services = world.get_services().await;
        if let Some(service) = services
            .iter()
            .find(|s| s["name"].as_str() == Some(name.as_str()))
        {
            if service["next_restart_epoch_ms"].as_u64().is_some() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("service '{name}' never reported a scheduled next restart");
}

#[when("the services endpoint is requested")]
async fn request_services_endpoint(world: &mut SentinelWorld) {
    world.http_get("/api/services").await;
}

#[then(expr = "the services response lists {string} with health {string}")]
fn services_response_lists(world: &mut SentinelWorld, name: String, health: String) {
    let body = world
        .last_response_body
        .as_ref()
        .expect("no response body captured");
    let services: Vec<serde_json::Value> = serde_json::from_str(body)
        .unwrap_or_else(|e| panic!("services response is not JSON ({e}): {body}"));
    let service = services
        .iter()
        .find(|s| s["name"].as_str() == Some(name.as_str()))
        .unwrap_or_else(|| panic!("service '{name}' not listed: {services:?}"));
    assert_eq!(
        service["health"].as_str(),
        Some(health.as_str()),
        "unexpected health: {service}"
    );
}
