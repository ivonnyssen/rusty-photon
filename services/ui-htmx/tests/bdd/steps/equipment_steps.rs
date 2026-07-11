//! Steps for `equipment_page.feature` — the roster view, capability tiers,
//! and the add/edit/remove config-surgery flows against a real rp.

use cucumber::gherkin::Step;
use cucumber::{given, then, when};

use crate::dom;
use crate::world::UiWorld;

#[given(
    regex = r#"^a running dsd-fp2 driver registered in rp's roster as cover calibrator "([\w-]+)"$"#
)]
async fn driver_in_rp_roster(world: &mut UiWorld, id: String) {
    world.start_driver_and_rp_with_cover_calibrator(&id).await;
}

#[when("I open the equipment page")]
async fn open_equipment(world: &mut UiWorld) {
    world.get("/equipment").await;
}

#[when(regex = r#"^I open the add-equipment form for "([\w_]+)"$"#)]
async fn open_add_form(world: &mut UiWorld, kind: String) {
    world.get(&format!("/equipment/{kind}/new")).await;
}

#[when(regex = r#"^I open the edit-equipment form for cover calibrator "([\w-]+)"$"#)]
async fn open_edit_form(world: &mut UiWorld, id: String) {
    world
        .get(&format!("/equipment/cover_calibrators/{id}/edit"))
        .await;
}

/// Submit the currently-rendered equipment form with the table's field edits.
#[when("I submit the equipment form with:")]
async fn submit_equipment_form(world: &mut UiWorld, step: &Step) {
    let table = step.table.as_ref().expect("step needs a table");
    let changes: Vec<(String, String)> = table
        .rows
        .iter()
        .skip(1) // header row: | field | value |
        .map(|row| (row[0].clone(), row[1].clone()))
        .collect();
    let refs: Vec<(&str, &str)> = changes
        .iter()
        .map(|(f, v)| (f.as_str(), v.as_str()))
        .collect();
    world.submit_rendered_form(&refs).await;
}

#[when(regex = r#"^I remove cover calibrator "([\w-]+)" from the roster$"#)]
async fn remove_entry(world: &mut UiWorld, id: String) {
    // The Remove affordance is an htmx button (hx-post + hx-confirm); drive
    // the POST it would issue after the operator confirms.
    world
        .post_htmx(&format!("/equipment/cover_calibrators/{id}/delete"))
        .await;
}

#[then(regex = r#"^the roster section "([^"]+)" lists "([\w-]+)"$"#)]
fn section_lists(world: &mut UiWorld, section: String, id: String) {
    let kind_id = section_css(&section);
    assert!(
        dom::text_contains(&world.last_body, &format!("{kind_id} .svc-id"), &id),
        "section {section} does not list {id}:\n{}",
        world.last_body
    );
}

// The roster shows what rp is RUNNING; a just-persisted entry appears only
// after rp's next start (the restart callout says so).
#[then(regex = r#"^the roster section "([^"]+)" does not yet list "([\w-]+)"$"#)]
fn section_does_not_list(world: &mut UiWorld, section: String, id: String) {
    let kind_id = section_css(&section);
    assert!(
        !dom::text_contains(&world.last_body, &format!("{kind_id} .svc-id"), &id),
        "section {section} unexpectedly lists {id} before an rp restart:\n{}",
        world.last_body
    );
}

#[then(regex = r#"^the roster section "([^"]+)" is empty$"#)]
fn section_empty(world: &mut UiWorld, section: String) {
    let kind_id = section_css(&section);
    assert!(
        dom::matches(&world.last_body, &format!("{kind_id} .empty-kind")),
        "section {section} is not empty:\n{}",
        world.last_body
    );
}

#[then(regex = r#"^the roster row for "([\w-]+)" shows a connected LED$"#)]
fn row_connected(world: &mut UiWorld, id: String) {
    assert!(
        dom::matches(&world.last_body, &row_css(&id, ".led.ok")),
        "no connected LED for {id}:\n{}",
        world.last_body
    );
}

#[then(regex = r#"^the roster row for "([\w-]+)" shows an unknown LED$"#)]
fn row_unknown(world: &mut UiWorld, id: String) {
    assert!(
        dom::matches(&world.last_body, &row_css(&id, ".led.unknown")),
        "no unknown LED for {id}:\n{}",
        world.last_body
    );
}

#[then(regex = r#"^the roster row for "([\w-]+)" carries the "([\w]+)" tier$"#)]
fn row_tier(world: &mut UiWorld, id: String, tier: String) {
    assert!(
        dom::matches(
            &world.last_body,
            &row_css(&id, &format!(".tier-badge.{tier}"))
        ),
        "row {id} does not carry tier {tier}:\n{}",
        world.last_body
    );
}

#[then(regex = r#"^the equipment form shows a problem mentioning "([^"]+)"$"#)]
fn form_problem(world: &mut UiWorld, needle: String) {
    assert!(
        dom::text_contains(&world.last_body, ".banner.error", &needle),
        "no problem banner mentioning {needle:?}:\n{}",
        world.last_body
    );
}

#[then(regex = r#"^rp's config file on disk contains the string "([^"]+)" exactly once$"#)]
fn rp_config_contains_once(world: &mut UiWorld, needle: String) {
    let on_disk = world.rp_config_on_disk().to_string();
    let count = on_disk.matches(&needle).count();
    assert_eq!(
        count, 1,
        "expected exactly one {needle:?}, found {count}: {on_disk}"
    );
}

#[then("the page explains that no rp orchestrator is configured")]
fn no_rp_explained(world: &mut UiWorld) {
    assert!(
        dom::text_contains(
            &world.last_body,
            ".banner.error",
            "No rp orchestrator is configured"
        ),
        "missing no-rp explanation:\n{}",
        world.last_body
    );
}

/// CSS id of a roster kind section from its display heading.
fn section_css(section: &str) -> String {
    let key = section.to_lowercase().replace(' ', "_");
    format!("#kind-{key}")
}

/// CSS selector scoped to one roster row (rows carry `id="row-{kind}-{id}"`;
/// the id is unique enough across kinds for these scenarios' asserts).
fn row_css(id: &str, suffix: &str) -> String {
    format!("li[id$=\"-{id}\"] {suffix}")
}
