//! BDD step definitions for the service restart API feature.
//!
//! The discovered units, the recorded restarts, and the recovery answers all
//! live in the stub service manager (`SENTINEL_SERVICE_MANAGER_DIR`); the
//! discovery/manager Given/Then steps themselves are shared with the health
//! feature and live in `health_steps.rs`.

use cucumber::{then, when};

use crate::world::SentinelWorld;

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

fn restart_body(world: &SentinelWorld) -> serde_json::Value {
    let body = world
        .last_response_body
        .as_ref()
        .expect("no response body captured");
    serde_json::from_str(body)
        .unwrap_or_else(|e| panic!("restart response is not JSON ({e}): {body}"))
}
