//! BDD step definitions for dashboard API feature

use cucumber::{then, when};

use crate::world::SentinelWorld;

#[when("the health endpoint is requested")]
async fn request_health(world: &mut SentinelWorld) {
    world.http_get("/health").await;
}

#[when("the status API endpoint is requested")]
async fn request_status(world: &mut SentinelWorld) {
    world.http_get("/api/status").await;
}

#[when("the history API endpoint is requested")]
async fn request_history(world: &mut SentinelWorld) {
    world.http_get("/api/history").await;
}

#[then(expr = "the response status should be {int}")]
fn response_status(world: &mut SentinelWorld, expected: u16) {
    let status = world.last_status_code.expect("no response status captured");
    assert_eq!(status, expected, "Expected status {expected}, got {status}");
}

#[then(expr = "the response body should be {string}")]
fn response_body(world: &mut SentinelWorld, expected: String) {
    let body = world
        .last_response_body
        .as_ref()
        .expect("no response body captured");
    assert_eq!(body, &expected, "Expected body '{expected}', got '{body}'");
}

#[then(expr = "the response should be a JSON array with {int} entry")]
fn response_json_array_len(world: &mut SentinelWorld, expected: usize) {
    let body = world
        .last_response_body
        .as_ref()
        .expect("no response body captured");
    let json: Vec<serde_json::Value> =
        serde_json::from_str(body).expect("response is not a JSON array");
    assert_eq!(
        json.len(),
        expected,
        "Expected {} entries, got {}",
        expected,
        json.len()
    );
}

#[then(expr = "the first entry should have name {string}")]
fn first_entry_name(world: &mut SentinelWorld, expected: String) {
    let body = world
        .last_response_body
        .as_ref()
        .expect("no response body captured");
    let json: Vec<serde_json::Value> =
        serde_json::from_str(body).expect("response is not a JSON array");
    let name = json[0]["name"]
        .as_str()
        .expect("first entry has no 'name' field");
    assert_eq!(name, expected, "Expected name '{expected}', got '{name}'");
}

#[then(expr = "the first entry should have state {string}")]
fn first_entry_state(world: &mut SentinelWorld, expected: String) {
    let body = world
        .last_response_body
        .as_ref()
        .expect("no response body captured");
    let json: Vec<serde_json::Value> =
        serde_json::from_str(body).expect("response is not a JSON array");
    let state = json[0]["state"]
        .as_str()
        .expect("first entry has no 'state' field");
    assert_eq!(
        state, expected,
        "Expected state '{expected}', got '{state}'"
    );
}

#[then("the response should be an empty JSON array")]
fn response_empty_json_array(world: &mut SentinelWorld) {
    let body = world
        .last_response_body
        .as_ref()
        .expect("no response body captured");
    let json: Vec<serde_json::Value> =
        serde_json::from_str(body).expect("response is not a JSON array");
    assert!(
        json.is_empty(),
        "Expected empty JSON array, got {} entries",
        json.len()
    );
}
