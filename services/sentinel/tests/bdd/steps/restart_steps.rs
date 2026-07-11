//! BDD step definitions for the service restart API feature

use cucumber::{given, then, when};

use crate::world::SentinelWorld;

/// A shell command that writes the marker file. The same line works under both
/// platform shells (`sh -c` on unix, `cmd /C` on windows), quoted path included.
fn write_marker_command(path: &std::path::Path) -> String {
    format!("echo ok > \"{}\"", path.display())
}

/// Exit-0 / exit-1 one-liners understood by both platform shells.
const SUCCEED: &str = "exit 0";
const FAIL: &str = "exit 1";

#[given(
    expr = "sentinel is running with a supervised service {string} whose restart command writes a marker file and whose health command succeeds"
)]
async fn service_with_marker_and_health(world: &mut SentinelWorld, name: String) {
    let marker = world.restart_marker_path();
    world.add_supervised_service(
        &name,
        Some(write_marker_command(&marker)),
        Some(SUCCEED.to_string()),
        None,
    );
    world.start_sentinel().await;
}

#[given(
    expr = "sentinel is running with a supervised service {string} whose restart command writes a marker file"
)]
async fn service_with_marker(world: &mut SentinelWorld, name: String) {
    let marker = world.restart_marker_path();
    world.add_supervised_service(&name, Some(write_marker_command(&marker)), None, None);
    world.start_sentinel().await;
}

#[given(
    expr = "sentinel is running with a supervised service {string} whose restart command succeeds, whose health command fails, and whose restart budget is {string}"
)]
async fn service_with_failing_health(world: &mut SentinelWorld, name: String, budget: String) {
    world.add_supervised_service(
        &name,
        Some(SUCCEED.to_string()),
        Some(FAIL.to_string()),
        Some(&budget),
    );
    world.start_sentinel().await;
}

#[given(
    expr = "sentinel is running with a supervised service {string} whose restart command fails"
)]
async fn service_with_failing_restart(world: &mut SentinelWorld, name: String) {
    world.add_supervised_service(&name, Some(FAIL.to_string()), None, None);
    world.start_sentinel().await;
}

#[given(
    expr = "sentinel is running with a supervised service {string} that has no restart command"
)]
async fn service_not_restartable(world: &mut SentinelWorld, name: String) {
    world.add_supervised_service(&name, None, None, None);
    world.start_sentinel().await;
}

#[when(expr = "the restart endpoint is requested for {string}")]
async fn request_restart(world: &mut SentinelWorld, name: String) {
    world
        .http_post(&format!("/api/services/{name}/restart"))
        .await;
}

#[then(expr = "the restart response reports status {string} and recovery {string}")]
fn restart_status_and_recovery(world: &mut SentinelWorld, status: String, recovery: String) {
    let json = restart_body(world);
    assert_eq!(
        json["status"].as_str(),
        Some(status.as_str()),
        "body: {json}"
    );
    assert_eq!(
        json["recovery"].as_str(),
        Some(recovery.as_str()),
        "body: {json}"
    );
}

#[then(expr = "the restart response reports status {string}")]
fn restart_status(world: &mut SentinelWorld, status: String) {
    let json = restart_body(world);
    assert_eq!(
        json["status"].as_str(),
        Some(status.as_str()),
        "body: {json}"
    );
}

#[then("the restart marker file exists")]
fn restart_marker_exists(world: &mut SentinelWorld) {
    let marker = world
        .restart_marker
        .as_ref()
        .expect("no marker path recorded");
    assert!(
        marker.exists(),
        "restart command did not write {}",
        marker.display()
    );
}

fn restart_body(world: &SentinelWorld) -> serde_json::Value {
    let body = world
        .last_response_body
        .as_ref()
        .expect("no response body captured");
    serde_json::from_str(body)
        .unwrap_or_else(|e| panic!("restart response is not JSON ({e}): {body}"))
}
