//! BDD step definitions for the target REST endpoints
//! (`target_store_rest.feature`, rp.md § REST Endpoints → Targets —
//! *(planned, P1)*, not yet implemented; scenarios are tagged `@wip`).
//! Mirrors the MCP tool bodies in `target_store_crud_steps.rs` — same
//! shapes, plain REST instead of `tools/call` (Decision 10's minimal
//! operator surface).

use cucumber::{given, then, when};
use serde_json::Value;

use crate::world::RpWorld;

async fn record_response(world: &mut RpWorld, response: reqwest::Response) {
    world.last_api_status = Some(response.status().as_u16());
    world.last_api_body = response.json::<Value>().await.ok();
}

// ---------------------------------------------------------------------------
// Given
// ---------------------------------------------------------------------------

#[given(expr = "a target named {string} has been created via POST \\/api\\/targets")]
async fn target_created_via_rest(world: &mut RpWorld, display_name: String) {
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{}/api/targets", world.rp_url()))
        .json(&serde_json::json!({
            "display_name": display_name,
            "ra_hours": 5.0,
            "dec_degrees": 10.0
        }))
        .send()
        .await
        .expect("POST /api/targets request failed");
    assert!(
        response.status().is_success(),
        "fixture POST /api/targets failed: {}",
        response.status()
    );
    record_response(world, response).await;
}

// ---------------------------------------------------------------------------
// When
// ---------------------------------------------------------------------------

#[when(
    expr = "I POST \\/api\\/targets with display_name {string} ra_hours {float} dec_degrees {float}"
)]
async fn post_targets(world: &mut RpWorld, display_name: String, ra_hours: f64, dec_degrees: f64) {
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{}/api/targets", world.rp_url()))
        .json(&serde_json::json!({
            "display_name": display_name,
            "ra_hours": ra_hours,
            "dec_degrees": dec_degrees
        }))
        .send()
        .await
        .expect("POST /api/targets request failed");
    record_response(world, response).await;
}

#[when("I GET /api/targets")]
async fn get_targets(world: &mut RpWorld) {
    let response = reqwest::get(format!("{}/api/targets", world.rp_url()))
        .await
        .expect("GET /api/targets request failed");
    record_response(world, response).await;
}

#[when(expr = "I GET the target at slug {string}")]
async fn get_target_at_slug(world: &mut RpWorld, slug: String) {
    let response = reqwest::get(format!("{}/api/targets/{}", world.rp_url(), slug))
        .await
        .expect("GET /api/targets/{slug} request failed");
    record_response(world, response).await;
}

#[when(expr = "I PUT the target at slug {string} setting display_name to {string}")]
async fn put_target_display_name(world: &mut RpWorld, slug: String, display_name: String) {
    let client = reqwest::Client::new();
    let response = client
        .put(format!("{}/api/targets/{}", world.rp_url(), slug))
        .json(&serde_json::json!({ "display_name": display_name }))
        .send()
        .await
        .expect("PUT /api/targets/{slug} request failed");
    record_response(world, response).await;
}

#[when(expr = "I DELETE the target at slug {string}")]
async fn delete_target_at_slug(world: &mut RpWorld, slug: String) {
    let client = reqwest::Client::new();
    let response = client
        .delete(format!("{}/api/targets/{}", world.rp_url(), slug))
        .send()
        .await
        .expect("DELETE /api/targets/{slug} request failed");
    record_response(world, response).await;
}

// ---------------------------------------------------------------------------
// Then
// ---------------------------------------------------------------------------

#[then(expr = "the targets API response status should be {int}")]
fn targets_api_status(world: &mut RpWorld, expected: u16) {
    assert_eq!(
        world.last_api_status,
        Some(expected),
        "unexpected status; body was: {:?}",
        world.last_api_body
    );
}

#[then(expr = "the targets API response should carry slug {string}")]
fn targets_api_response_slug(world: &mut RpWorld, expected: String) {
    let body = world
        .last_api_body
        .as_ref()
        .expect("no targets API response body");
    let target = body.get("target").unwrap_or(body);
    let slug = target
        .get("slug")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("targets API response missing `slug`: {body}"));
    assert_eq!(slug, expected.as_str(), "targets API response slug");
}

#[then(expr = "the targets API response should carry display_name {string}")]
fn targets_api_response_display_name(world: &mut RpWorld, expected: String) {
    let body = world
        .last_api_body
        .as_ref()
        .expect("no targets API response body");
    let target = body.get("target").unwrap_or(body);
    let name = target
        .get("display_name")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("targets API response missing `display_name`: {body}"));
    assert_eq!(name, expected.as_str(), "targets API response display_name");
}

#[then(expr = "the targets API target list should contain exactly {string}")]
fn targets_api_list_exactly(world: &mut RpWorld, expected_slug: String) {
    let body = world
        .last_api_body
        .as_ref()
        .expect("no targets API response body");
    let list = body
        .get("targets")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("targets API response missing `targets` array: {body}"));
    let slugs: Vec<&str> = list
        .iter()
        .filter_map(|t| {
            t.get("target")
                .unwrap_or(t)
                .get("slug")
                .and_then(|s| s.as_str())
        })
        .collect();
    assert_eq!(
        slugs,
        vec![expected_slug.as_str()],
        "targets API list slugs"
    );
}

#[then("GET /api/targets should list no targets")]
async fn get_targets_should_list_none(world: &mut RpWorld) {
    let response = reqwest::get(format!("{}/api/targets", world.rp_url()))
        .await
        .expect("GET /api/targets request failed");
    record_response(world, response).await;
    let body = world
        .last_api_body
        .as_ref()
        .expect("no targets API response body");
    let list = body
        .get("targets")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("targets API response missing `targets` array: {body}"));
    assert!(list.is_empty(), "expected no targets, got: {list:?}");
}
