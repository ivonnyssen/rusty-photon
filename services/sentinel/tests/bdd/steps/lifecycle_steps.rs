//! BDD step definitions for service lifecycle feature

use cucumber::{given, then, when};

use crate::world::SentinelWorld;

#[given("sentinel is running with no monitors")]
async fn sentinel_running_no_monitors(world: &mut SentinelWorld) {
    world.start_sentinel().await;
}

#[given("sentinel is configured to monitor the filemonitor")]
fn sentinel_configured_with_filemonitor(world: &mut SentinelWorld) {
    world.add_filemonitor_monitor("Roof Monitor");
}

#[given("sentinel is running")]
async fn sentinel_running(world: &mut SentinelWorld) {
    world.start_sentinel().await;
}

#[when(expr = "I try to start sentinel with config {string}")]
async fn try_start_sentinel_with_config(world: &mut SentinelWorld, path: String) {
    world.try_start_sentinel(&path).await;
}

#[then("the dashboard health endpoint should return OK")]
async fn dashboard_health_ok(world: &mut SentinelWorld) {
    let client = reqwest::Client::new();
    let url = world.dashboard_url("/health");

    let mut ok = false;
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().as_u16() == 200 {
                ok = true;
                break;
            }
        }
    }
    assert!(ok, "Dashboard /health did not return 200 OK");
}

#[then("sentinel should fail to start")]
fn sentinel_should_fail(world: &mut SentinelWorld) {
    assert!(
        world.last_error.is_some(),
        "Expected sentinel to fail to start, but no error was captured"
    );
}
