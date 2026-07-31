//! BDD step definitions for `add_target`'s import form
//! (`target_store_import.feature`, rp.md § Target Store → Import form):
//! bare coordinates plus a `source` provenance object, the shape the
//! planetarium bridge sends.
//!
//! Reuses the crud file's launcher Givens and target-result Thens; the
//! steps here own the `source`-form calls, the resolved-centroid
//! bookkeeping (`world.resolved_coord`), and assertions on the fields
//! only imports produce (writer identity, provenance notes, pause).

use cucumber::{given, then, when};
use serde_json::{json, Value};

use crate::steps::tool_steps::ensure_mcp_client;
use crate::world::RpWorld;

const SOURCE_KIND: &str = "planetarium-bridge";
const SOURCE_CLIENT: &str = "192.0.2.10:52441";
const SOURCE_RECEIVED_AT: &str = "2026-07-30T01:23:45.678Z";

fn default_source() -> Value {
    json!({
        "kind": SOURCE_KIND,
        "client": SOURCE_CLIENT,
        "received_at": SOURCE_RECEIVED_AT,
    })
}

/// Calls `add_target` with `args`, injecting the canonical `source`
/// object unless the caller already set one, and records slug + result.
async fn call_import(world: &mut RpWorld, mut args: Value) {
    ensure_mcp_client(world).await;
    if args.get("source").is_none() {
        args["source"] = default_source();
    }
    let result = world.mcp().call_tool("add_target", args).await;
    if let Ok(ref v) = result {
        world.last_target_slug = v.get("slug").and_then(|s| s.as_str()).map(String::from);
    }
    world.last_tool_result = Some(result);
}

/// The centroid captured by the resolve Given, offset by the requested
/// east/north arcminutes. Inverts `rp-catalog`'s `NearestMatch` offset
/// definition (East = Δα·cos δ against the *centroid* declination), so
/// the requested offsets come back verbatim in the offset display form.
fn offset_coordinates(world: &RpWorld, east_arcmin: f64, north_arcmin: f64) -> (f64, f64) {
    let (ra_hours, dec_degrees) = world
        .resolved_coord
        .expect("no resolved coordinates — resolve a catalog target first");
    let dec = dec_degrees + north_arcmin / 60.0;
    let ra = ra_hours + east_arcmin / (60.0 * 15.0 * dec_degrees.to_radians().cos());
    (ra, dec)
}

// ---------------------------------------------------------------------------
// Given
// ---------------------------------------------------------------------------

#[given(expr = "the MCP client has resolved catalog target {string}")]
async fn resolved_catalog_target(world: &mut RpWorld, name: String) {
    ensure_mcp_client(world).await;
    let result = world
        .mcp()
        .call_tool("resolve_target", json!({ "name": name }))
        .await;
    let value = result
        .as_ref()
        .unwrap_or_else(|e| panic!("fixture resolve_target failed: {e}"));
    let ra = value
        .get("ra_hours")
        .and_then(Value::as_f64)
        .expect("resolve_target payload missing ra_hours");
    let dec = value
        .get("dec_degrees")
        .and_then(Value::as_f64)
        .expect("resolve_target payload missing dec_degrees");
    world.resolved_coord = Some((ra, dec));
}

#[given("the MCP client has imported a target at the resolved coordinates")]
async fn imported_at_resolved(world: &mut RpWorld) {
    let (ra, dec) = world
        .resolved_coord
        .expect("no resolved coordinates — resolve a catalog target first");
    call_import(world, json!({ "ra_hours": ra, "dec_degrees": dec })).await;
    let result = world.last_tool_result.as_ref().expect("no tool result");
    assert!(result.is_ok(), "fixture import failed: {result:?}");
}

#[given("the MCP client has activated the target it just added")]
async fn activated_last_target(world: &mut RpWorld) {
    let slug = world
        .last_target_slug
        .clone()
        .expect("no remembered target slug — import a target first");
    ensure_mcp_client(world).await;
    let result = world
        .mcp()
        .call_tool("update_target", json!({ "slug": slug, "active": true }))
        .await;
    assert!(result.is_ok(), "fixture update_target failed: {result:?}");
}

#[given(expr = "the MCP client has renamed the target it just added to {string}")]
async fn renamed_last_target(world: &mut RpWorld, display_name: String) {
    let slug = world
        .last_target_slug
        .clone()
        .expect("no remembered target slug — import a target first");
    ensure_mcp_client(world).await;
    let result = world
        .mcp()
        .call_tool(
            "update_target",
            json!({ "slug": slug, "display_name": display_name }),
        )
        .await;
    assert!(result.is_ok(), "fixture update_target failed: {result:?}");
}

// ---------------------------------------------------------------------------
// When
// ---------------------------------------------------------------------------

#[when(expr = "the MCP client imports a target at ra_hours {float} dec_degrees {float}")]
async fn import_at(world: &mut RpWorld, ra_hours: f64, dec_degrees: f64) {
    call_import(
        world,
        json!({ "ra_hours": ra_hours, "dec_degrees": dec_degrees }),
    )
    .await;
}

#[when("the MCP client imports a target at the resolved coordinates")]
async fn import_at_resolved(world: &mut RpWorld) {
    let (ra, dec) = world
        .resolved_coord
        .expect("no resolved coordinates — resolve a catalog target first");
    call_import(world, json!({ "ra_hours": ra, "dec_degrees": dec })).await;
}

#[when(
    expr = "the MCP client imports a target at the resolved coordinates offset east {float} north {float} arcmin"
)]
async fn import_at_offset(world: &mut RpWorld, east_arcmin: f64, north_arcmin: f64) {
    let (ra, dec) = offset_coordinates(world, east_arcmin, north_arcmin);
    call_import(world, json!({ "ra_hours": ra, "dec_degrees": dec })).await;
}

#[when("the MCP client imports a target without coordinates")]
async fn import_without_coordinates(world: &mut RpWorld) {
    call_import(world, json!({})).await;
}

#[when(
    expr = "the MCP client imports at ra_hours {float} dec_degrees {float} with extra parameter {string}"
)]
async fn import_with_extra_parameter(
    world: &mut RpWorld,
    ra_hours: f64,
    dec_degrees: f64,
    parameter: String,
) {
    let mut args = json!({ "ra_hours": ra_hours, "dec_degrees": dec_degrees });
    args[parameter.as_str()] = match parameter.as_str() {
        "catalog_ref" => json!("M 31"),
        "display_name" => json!("My Frame"),
        "active" => json!(false),
        "notes" => json!("hand-written"),
        "position_angle_degrees" => json!(90.0),
        other => panic!("unsupported extra parameter {other:?}"),
    };
    call_import(world, args).await;
}

#[when(
    expr = "the MCP client imports at ra_hours {float} dec_degrees {float} with source kind {string}"
)]
async fn import_with_source_kind(
    world: &mut RpWorld,
    ra_hours: f64,
    dec_degrees: f64,
    kind: String,
) {
    let args = json!({
        "ra_hours": ra_hours,
        "dec_degrees": dec_degrees,
        "source": {
            "kind": kind,
            "client": SOURCE_CLIENT,
            "received_at": SOURCE_RECEIVED_AT,
        },
    });
    call_import(world, args).await;
}

// ---------------------------------------------------------------------------
// Then
// ---------------------------------------------------------------------------

#[then(expr = "the imported target display_name should be {string}")]
fn imported_display_name(world: &mut RpWorld, expected: String) {
    let name = target_field(world, "display_name");
    assert_eq!(name.as_str(), Some(expected.as_str()), "display_name");
}

#[then(expr = "the imported target catalog_ref should be {string}")]
fn imported_catalog_ref(world: &mut RpWorld, expected: String) {
    let catalog_ref = target_field(world, "catalog_ref");
    assert_eq!(catalog_ref.as_str(), Some(expected.as_str()), "catalog_ref");
}

#[then("the imported target should be paused")]
fn imported_paused(world: &mut RpWorld) {
    let active = target_field(world, "active");
    assert_eq!(active.as_bool(), Some(false), "expected active: false");
}

#[then(expr = "the imported target created_by should be {string}")]
fn imported_created_by(world: &mut RpWorld, expected: String) {
    let created_by = target_field(world, "created_by");
    assert_eq!(created_by.as_str(), Some(expected.as_str()), "created_by");
}

#[then(expr = "the imported target updated_by should be {string}")]
fn imported_updated_by(world: &mut RpWorld, expected: String) {
    let updated_by = target_field(world, "updated_by");
    assert_eq!(updated_by.as_str(), Some(expected.as_str()), "updated_by");
}

#[then(expr = "the imported target notes should contain {string}")]
fn imported_notes_contain(world: &mut RpWorld, expected: String) {
    let notes = target_field(world, "notes");
    let notes = notes.as_str().expect("notes should be a string");
    assert!(
        notes.contains(&expected),
        "notes {notes:?} does not contain {expected:?}"
    );
}

#[then(expr = "the imported target should have {int} goal(s) with filter {string}")]
fn imported_goal_count(world: &mut RpWorld, expected: usize, filter: String) {
    let goals = target_field(world, "goals");
    let goals = goals.as_array().expect("goals should be an array");
    let matching = goals
        .iter()
        .filter(|g| g.get("filter").and_then(|f| f.as_str()) == Some(filter.as_str()))
        .count();
    assert_eq!(
        matching, expected,
        "goals with filter {filter:?}: {goals:?}"
    );
}

#[then(
    expr = "the imported target coordinates should be the resolved coordinates offset east {float} north {float} arcmin"
)]
fn imported_coordinates(world: &mut RpWorld, east_arcmin: f64, north_arcmin: f64) {
    let (expected_ra, expected_dec) = offset_coordinates(world, east_arcmin, north_arcmin);
    let coord = target_field(world, "coord").clone();
    let ra = coord
        .get("ra_hours")
        .and_then(Value::as_f64)
        .expect("coord missing ra_hours");
    let dec = coord
        .get("dec_degrees")
        .and_then(Value::as_f64)
        .expect("coord missing dec_degrees");
    assert!(
        (ra - expected_ra).abs() < 1e-9 && (dec - expected_dec).abs() < 1e-9,
        "coordinates ({ra}, {dec}) differ from expected ({expected_ra}, {expected_dec})"
    );
}

#[then(expr = "the fetched target created_by should be {string}")]
fn fetched_created_by(world: &mut RpWorld, expected: String) {
    let created_by = target_field(world, "created_by");
    assert_eq!(created_by.as_str(), Some(expected.as_str()), "created_by");
}

#[then(expr = "the fetched target updated_by should be {string}")]
fn fetched_updated_by(world: &mut RpWorld, expected: String) {
    let updated_by = target_field(world, "updated_by");
    assert_eq!(updated_by.as_str(), Some(expected.as_str()), "updated_by");
}

// --- Helpers ---

/// Reads a field off the nested `target` object of the last tool result
/// (the same shape the crud file's `target_field` reads; restated here
/// because those helpers are file-private).
fn target_field<'a>(world: &'a RpWorld, field: &str) -> &'a Value {
    let payload = world
        .last_tool_result
        .as_ref()
        .expect("no tool result")
        .as_ref()
        .expect("expected tool call to succeed");
    let target = payload
        .get("target")
        .unwrap_or_else(|| panic!("tool result missing `target`: {payload}"));
    target
        .get(field)
        .unwrap_or_else(|| panic!("target payload missing field {field:?}: {payload}"))
}
