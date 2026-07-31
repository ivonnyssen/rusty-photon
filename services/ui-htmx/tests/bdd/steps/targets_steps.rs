//! Steps for `targets_page.feature` — the targets inbox over rp's MCP
//! target tools.
//!
//! Scenarios seed the store through rp's **own** `add_target` (the
//! operator form, and the import form for provenance rows), so what the
//! page renders is what rp actually wrote — never a fixture. Stored-state
//! assertions read back through `get_target` the same way.

use cucumber::{given, then, when};
use serde_json::json;

use crate::dom;
use crate::world::UiWorld;

/// The Given-seeded coordinates for operator-created targets. Arbitrary
/// but away from the poles; operator adds never consult the catalog, so
/// the slug is the kebab-cased display name by construction.
const SEED_RA_HOURS: f64 = 5.5;
const SEED_DEC_DEGREES: f64 = 10.0;

// ---------------------------------------------------------------------------
// Given: rp fixtures
// ---------------------------------------------------------------------------

#[given(
    expr = "a running rp orchestrator with an imaging train whose default position angle is {float}"
)]
async fn rp_with_imaging_train(world: &mut UiWorld, angle: f64) {
    world.start_rp_with_imaging_train(angle).await;
}

#[given(expr = "a running rp orchestrator whose filter roster is {string} and {string}")]
async fn rp_with_filter_roster(world: &mut UiWorld, first: String, second: String) {
    world.start_rp_with_filters(&[first, second]).await;
}

#[given(expr = "rp restarts with a filter roster of only {string}")]
async fn rp_restarts_with_filter(world: &mut UiWorld, filter: String) {
    world.restart_rp_with_filters(&[filter]).await;
}

// ---------------------------------------------------------------------------
// Given: seeding the store through rp's own tools
// ---------------------------------------------------------------------------

#[given(
    expr = "a planetarium import is pending at RA {float} hours Dec {float} degrees from client {string}"
)]
async fn pending_import(world: &mut UiWorld, ra: f64, dec: f64, client: String) {
    world
        .add_target_via_mcp(json!({
            "ra_hours": ra,
            "dec_degrees": dec,
            "source": {
                "kind": "planetarium-bridge",
                "client": client,
                "received_at": "2026-07-30T21:14:09Z"
            }
        }))
        .await;
}

#[given(expr = "I have added the pending target {string}")]
async fn pending_target(world: &mut UiWorld, name: String) {
    world
        .add_target_via_mcp(json!({
            "display_name": name,
            "ra_hours": SEED_RA_HOURS,
            "dec_degrees": SEED_DEC_DEGREES,
            "active": false
        }))
        .await;
}

#[given(expr = "I have added the pending target {string} with position angle {float}")]
async fn pending_target_with_angle(world: &mut UiWorld, name: String, angle: f64) {
    world
        .add_target_via_mcp(json!({
            "display_name": name,
            "ra_hours": SEED_RA_HOURS,
            "dec_degrees": SEED_DEC_DEGREES,
            "active": false,
            "position_angle_degrees": angle
        }))
        .await;
}

#[given(expr = "I have added the pending target {string} with a goal in filter {string}")]
async fn pending_target_with_goal(world: &mut UiWorld, name: String, filter: String) {
    world
        .add_target_via_mcp(json!({
            "display_name": name,
            "ra_hours": SEED_RA_HOURS,
            "dec_degrees": SEED_DEC_DEGREES,
            "active": false,
            "goals": [{
                "filter": filter,
                "binning": "1x1",
                "exposure_duration": "5m",
                "desired_count": 10
            }]
        }))
        .await;
}

#[given(expr = "I have added the active target {string}")]
async fn active_target(world: &mut UiWorld, name: String) {
    world
        .add_target_via_mcp(json!({
            "display_name": name,
            "ra_hours": SEED_RA_HOURS,
            "dec_degrees": SEED_DEC_DEGREES
        }))
        .await;
}

// ---------------------------------------------------------------------------
// When: driving the pages
// ---------------------------------------------------------------------------

#[when("I open the targets page")]
async fn open_targets_page(world: &mut UiWorld) {
    world.get("/targets").await;
}

#[when(expr = "I open the review page for {string}")]
async fn open_review_page(world: &mut UiWorld, slug: String) {
    world.get(&format!("/targets/{slug}")).await;
}

#[when(expr = "I save the review form with position angle {string}")]
async fn save_with_angle(world: &mut UiWorld, value: String) {
    world
        .submit_rendered_form(&[("position_angle_degrees", &value)])
        .await;
}

#[when(
    expr = "I save the review form with a goal of {int} frames in filter {string} binning {string} exposing {string}"
)]
async fn save_with_goal(
    world: &mut UiWorld,
    count: u32,
    filter: String,
    binning: String,
    exposure: String,
) {
    world
        .submit_rendered_form(&[
            ("goal_filter", &filter),
            ("goal_binning", &binning),
            ("goal_exposure", &exposure),
            ("goal_count", &count.to_string()),
        ])
        .await;
}

#[when(expr = "I press the activate button of {string}")]
async fn press_activate(world: &mut UiWorld, slug: String) {
    press_row_button(world, "#inbox", &slug, "activate").await;
}

#[when(expr = "I press the pause button of {string}")]
async fn press_pause(world: &mut UiWorld, slug: String) {
    press_row_button(world, "#active-roster", &slug, "pause").await;
}

#[when(expr = "I press the discard button of {string}")]
async fn press_discard(world: &mut UiWorld, slug: String) {
    press_row_button(world, "#inbox", &slug, "discard").await;
}

/// Follow a row affordance the way htmx would: read the button's own
/// rendered `hx-post` URL from the last response and POST it.
async fn press_row_button(world: &mut UiWorld, section: &str, slug: &str, button: &str) {
    let css = format!(r#"{section} .target-row[data-slug="{slug}"] button.{button}"#);
    let url = dom::attr(&world.last_body, &css, "hx-post")
        .unwrap_or_else(|| panic!("no {button} affordance for {slug}:\n{}", world.last_body));
    world.post_htmx(&url).await;
}

// ---------------------------------------------------------------------------
// Then: the rendered pages
// ---------------------------------------------------------------------------

#[then(expr = "the inbox section lists {int} pending target(s)")]
fn inbox_row_count(world: &mut UiWorld, expected: usize) {
    let n = dom::count(&world.last_body, "#inbox .target-row");
    assert_eq!(n, expected, "inbox rows in:\n{}", world.last_body);
}

#[then(expr = "the inbox section contains {string}")]
fn inbox_contains(world: &mut UiWorld, needle: String) {
    assert!(
        dom::text_contains(&world.last_body, "#inbox", &needle),
        "inbox does not mention {needle:?}:\n{}",
        world.last_body
    );
}

#[then(expr = "the active section contains {string}")]
fn active_contains(world: &mut UiWorld, needle: String) {
    assert!(
        dom::text_contains(&world.last_body, "#active-roster", &needle),
        "active roster does not mention {needle:?}:\n{}",
        world.last_body
    );
}

#[then(expr = "the pending row provenance mentions {string}")]
fn provenance_mentions(world: &mut UiWorld, needle: String) {
    assert!(
        dom::text_contains(&world.last_body, "#inbox .provenance", &needle),
        "no pending-row provenance mentioning {needle:?}:\n{}",
        world.last_body
    );
}

#[then(expr = "the pending row shows position angle {string}")]
fn row_position_angle(world: &mut UiWorld, needle: String) {
    assert!(
        dom::text_contains(&world.last_body, "#inbox .position-angle", &needle),
        "no pending-row position angle {needle:?}:\n{}",
        world.last_body
    );
}

#[then(expr = "the active row for {string} offers no discard button")]
fn active_row_no_discard(world: &mut UiWorld, slug: String) {
    let css = format!(r#"#active-roster .target-row[data-slug="{slug}"] button.discard"#);
    assert!(
        !dom::matches(&world.last_body, &css),
        "active row {slug} unexpectedly offers discard:\n{}",
        world.last_body
    );
}

#[then(expr = "the targets empty state mentions {string}")]
fn empty_state_mentions(world: &mut UiWorld, needle: String) {
    assert!(
        dom::text_contains(&world.last_body, ".empty-state", &needle),
        "no empty state mentioning {needle:?}:\n{}",
        world.last_body
    );
}

#[then(expr = "the goal flag for {string} mentions {string}")]
fn goal_flag_mentions(world: &mut UiWorld, slug: String, needle: String) {
    let css = format!(r#"#inbox .target-row[data-slug="{slug}"] .goal-flag"#);
    assert!(
        dom::text_contains(&world.last_body, &css, &needle),
        "no goal flag for {slug} mentioning {needle:?}:\n{}",
        world.last_body
    );
}

#[then("no goal flag is rendered")]
fn no_goal_flag(world: &mut UiWorld) {
    assert!(
        !dom::matches(&world.last_body, ".goal-flag"),
        "unexpected goal flag:\n{}",
        world.last_body
    );
}

#[then(expr = "the targets unavailable card mentions {string}")]
fn unavailable_mentions(world: &mut UiWorld, needle: String) {
    assert!(
        dom::text_contains(&world.last_body, ".targets-unavailable", &needle),
        "no unavailable card mentioning {needle:?}:\n{}",
        world.last_body
    );
}

#[then("the targets unavailable card offers a retry link")]
fn unavailable_retry(world: &mut UiWorld) {
    assert_eq!(
        dom::attr(
            &world.last_body,
            "#targets-page button.link[hx-get]",
            "hx-get"
        )
        .as_deref(),
        Some("/targets"),
        "no retry affordance in:\n{}",
        world.last_body
    );
}

// ---------------------------------------------------------------------------
// Then: the review form
// ---------------------------------------------------------------------------

#[then("the position angle field is blank")]
fn pa_field_blank(world: &mut UiWorld) {
    let input = dom::input(&world.last_body, "position_angle_degrees")
        .unwrap_or_else(|| panic!("no position angle field:\n{}", world.last_body));
    assert_eq!(input.value, "", "position angle field not blank");
}

#[then(expr = "the position angle field shows {string}")]
fn pa_field_shows(world: &mut UiWorld, expected: String) {
    let input = dom::input(&world.last_body, "position_angle_degrees")
        .unwrap_or_else(|| panic!("no position angle field:\n{}", world.last_body));
    assert_eq!(input.value, expected, "position angle field");
}

#[then(expr = "the position angle hint mentions {string}")]
fn pa_hint_mentions(world: &mut UiWorld, needle: String) {
    assert!(
        dom::text_contains(&world.last_body, ".pa-hint", &needle),
        "no position-angle hint mentioning {needle:?}:\n{}",
        world.last_body
    );
}

#[then(expr = "the position angle field has an error mentioning {string}")]
fn pa_field_error(world: &mut UiWorld, needle: String) {
    assert!(
        dom::field_error(&world.last_body, "position_angle_degrees", &needle),
        "no position-angle field error mentioning {needle:?}:\n{}",
        world.last_body
    );
}

#[then(expr = "the review page lists a goal mentioning {string}")]
fn review_lists_goal(world: &mut UiWorld, filter: String) {
    let css = format!(r#"input[name="goal_filter"][value="{filter}"]"#);
    assert!(
        dom::matches(&world.last_body, &css),
        "no goal row for filter {filter:?}:\n{}",
        world.last_body
    );
}

// ---------------------------------------------------------------------------
// Then: the store, read back through rp's own tools
// ---------------------------------------------------------------------------

#[then(expr = "the stored position angle of {string} is {float}")]
async fn stored_angle_is(world: &mut UiWorld, slug: String, expected: f64) {
    let target = world.target_via_mcp(&slug).await.unwrap();
    let angle = target["position_angle_degrees"]
        .as_f64()
        .unwrap_or_else(|| {
            panic!("no position_angle_degrees on {slug}: {target}");
        });
    assert!(
        (angle - expected).abs() < f64::EPSILON,
        "stored angle of {slug}: expected {expected}, got {angle}"
    );
}

#[then(expr = "the stored position angle of {string} is cleared")]
async fn stored_angle_cleared(world: &mut UiWorld, slug: String) {
    let target = world.target_via_mcp(&slug).await.unwrap();
    assert!(
        target["position_angle_degrees"].is_null(),
        "stored angle of {slug} not cleared: {target}"
    );
}

#[then(expr = "the stored goals of {string} have {int} entry")]
async fn stored_goal_count(world: &mut UiWorld, slug: String, expected: usize) {
    let target = world.target_via_mcp(&slug).await.unwrap();
    let goals = target["goals"].as_array().unwrap();
    assert_eq!(goals.len(), expected, "goals of {slug}: {target}");
}

#[then(expr = "the stored goal {int} of {string} wants {int} frames in filter {string}")]
async fn stored_goal_shape(
    world: &mut UiWorld,
    index: usize,
    slug: String,
    count: u64,
    filter: String,
) {
    let target = world.target_via_mcp(&slug).await.unwrap();
    let goal = &target["goals"][index];
    assert_eq!(goal["filter"].as_str(), Some(filter.as_str()), "{target}");
    assert_eq!(goal["desired_count"].as_u64(), Some(count), "{target}");
}

#[then(expr = "the stored target {string} is active")]
async fn stored_target_active(world: &mut UiWorld, slug: String) {
    let target = world.target_via_mcp(&slug).await.unwrap();
    assert_eq!(target["active"].as_bool(), Some(true), "{target}");
}

#[then(expr = "the stored target {string} is paused")]
async fn stored_target_paused(world: &mut UiWorld, slug: String) {
    let target = world.target_via_mcp(&slug).await.unwrap();
    assert_eq!(target["active"].as_bool(), Some(false), "{target}");
}

#[then(expr = "the store no longer holds {string}")]
async fn store_no_longer_holds(world: &mut UiWorld, slug: String) {
    let result = world.target_via_mcp(&slug).await;
    assert!(result.is_err(), "target {slug} still present: {result:?}");
}
