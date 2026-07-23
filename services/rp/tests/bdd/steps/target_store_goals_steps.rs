//! BDD step definitions for target acquisition goals and filter-roster
//! validation (`target_store_goals.feature`, rp.md § Target Store —
//! *(planned, P1)*, not yet implemented; scenarios are tagged `@wip`).

use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use serde_json::Value;

use crate::steps::tool_steps::ensure_mcp_client;
use crate::world::RpWorld;

/// Parses a `| filter | binning | exposure | desired_count |` Gherkin
/// table into the JSON shape `add_target`/`set_goals` accept for their
/// `goals[]` parameter.
fn goals_from_table(step: &Step) -> Vec<Value> {
    let table = step
        .table
        .as_ref()
        .expect("step requires a `| filter | binning | exposure | desired_count |` table");
    let mut rows = table.rows.iter();
    let header = rows.next().expect("goals table must have a header");
    assert_eq!(
        header.as_slice(),
        ["filter", "binning", "exposure", "desired_count"],
        "goals table header"
    );
    rows.map(|row| {
        serde_json::json!({
            "filter": row[0],
            "binning": row[1],
            "exposure": row[2],
            "desired_count": row[3].parse::<u32>().expect("desired_count must parse as u32"),
        })
    })
    .collect()
}

// ---------------------------------------------------------------------------
// Given
// ---------------------------------------------------------------------------

#[given(expr = "rp is configured with default target goals:")]
fn default_target_goals(world: &mut RpWorld, step: &Step) {
    let goals = goals_from_table(step);
    let targets_block = world
        .target_store_config
        .get_or_insert_with(|| serde_json::json!({}));
    targets_block["default_goals"] = Value::Array(goals);
}

#[given(expr = "the MCP client has set its goals to:")]
async fn set_goals_fixture(world: &mut RpWorld, step: &Step) {
    let goals = goals_from_table(step);
    let slug = world
        .last_target_slug
        .clone()
        .expect("no remembered target slug — add a target fixture step first");
    ensure_mcp_client(world).await;
    let result = world
        .mcp()
        .call_tool(
            "set_goals",
            serde_json::json!({ "slug": slug, "goals": goals }),
        )
        .await;
    result.unwrap_or_else(|e| panic!("fixture set_goals failed: {e}"));
}

// ---------------------------------------------------------------------------
// When
// ---------------------------------------------------------------------------

#[when(expr = "the MCP client calls \"set_goals\" for slug {string} with goals:")]
async fn call_set_goals(world: &mut RpWorld, slug: String, step: &Step) {
    let goals = goals_from_table(step);
    ensure_mcp_client(world).await;
    let result = world
        .mcp()
        .call_tool(
            "set_goals",
            serde_json::json!({ "slug": slug, "goals": goals }),
        )
        .await;
    world.last_tool_result = Some(result);
}

#[when(
    expr = "the MCP client calls \"add_target\" with display_name {string} ra_hours {float} dec_degrees {float} and goals:"
)]
async fn call_add_target_with_goals(
    world: &mut RpWorld,
    display_name: String,
    ra_hours: f64,
    dec_degrees: f64,
    step: &Step,
) {
    let goals = goals_from_table(step);
    ensure_mcp_client(world).await;
    let result = world
        .mcp()
        .call_tool(
            "add_target",
            serde_json::json!({
                "display_name": display_name,
                "ra_hours": ra_hours,
                "dec_degrees": dec_degrees,
                "goals": goals
            }),
        )
        .await;
    world.last_tool_result = Some(result);
}

// ---------------------------------------------------------------------------
// Then
// ---------------------------------------------------------------------------

#[then(expr = "the fetched target should have exactly these goals:")]
fn fetched_target_has_goals(world: &mut RpWorld, step: &Step) {
    let expected = goals_from_table(step);
    let payload = world
        .last_tool_result
        .as_ref()
        .expect("no tool result")
        .as_ref()
        .expect("expected tool call to succeed");
    let target = payload
        .get("target")
        .unwrap_or_else(|| panic!("tool result missing `target`: {payload}"));
    let actual = target
        .get("goals")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("target payload missing `goals` array: {target}"));
    assert_eq!(actual, &expected, "target goals");
}

// "the tool error message should mention {string}" is registered
// globally in ephemeris_steps.rs. Reused here, not redefined.
