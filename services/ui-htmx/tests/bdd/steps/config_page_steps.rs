//! Step definitions for config_page.feature.
//!
//! Every scenario drives the real BFF over HTTP against a real dsd-fp2 driver
//! (see [`crate::world::UiWorld`]); the steps assert on the HTML the BFF
//! actually renders.

use cucumber::{given, then, when};

use crate::world::UiWorld;

// --- Given: stand up the real services --------------------------------------

#[given(
    regex = r#"^a dsd-fp2 driver running with serial\.port "([^"]+)" and max_brightness (\d+)$"#
)]
async fn driver_running(world: &mut UiWorld, port: String, max_brightness: u32) {
    world.start_driver_and_bff(&port, max_brightness).await;
}

#[given(
    regex = r#"^a dsd-fp2 driver running with the serial port pinned to "([^"]+)" by a command-line override$"#
)]
async fn driver_running_with_override(world: &mut UiWorld, port: String) {
    world.start_driver_with_serial_override_and_bff(&port).await;
}

#[given("the BFF is pointed at a dsd-fp2 driver that is not running")]
async fn driver_not_running(world: &mut UiWorld) {
    world.start_bff_with_unreachable_driver().await;
}

#[given("the driver's bound port is pinned so a reload keeps the same address")]
async fn pin_driver_port(world: &mut UiWorld) {
    world.pin_driver_port().await;
}

// --- When: interact with the BFF --------------------------------------------

#[when("I open the dsd-fp2 config page")]
async fn open_page(world: &mut UiWorld) {
    world.get("/config/dsd-fp2").await;
}

#[when(regex = r"^I open the dsd-fp2 config page with ([\w.]+) unlocked$")]
async fn open_page_with_unlock(world: &mut UiWorld, field: String) {
    // The escape hatch is a plain `?unlock=<field>` query — the same link the
    // "Unlock to edit" affordance points at; no client-side JS involved.
    world.get(&format!("/config/dsd-fp2?unlock={field}")).await;
}

#[when(regex = r"^I submit the config form setting max_brightness to (\d+)$")]
async fn submit_max_brightness(world: &mut UiWorld, value: String) {
    world
        .submit_form(&[("cover_calibrator.max_brightness", &value)])
        .await;
}

#[when(regex = r"^I submit the config form setting baud_rate to (\d+)$")]
async fn submit_baud_rate(world: &mut UiWorld, value: String) {
    world.submit_form(&[("serial.baud_rate", &value)]).await;
}

#[when("I submit the config form without changing anything")]
async fn submit_unchanged(world: &mut UiWorld) {
    world.submit_form(&[]).await;
}

#[when(regex = r"^I poll the reconnect status until max_brightness (\d+) is served$")]
async fn poll_until_served(world: &mut UiWorld, value: String) {
    world.poll_status_until_value(&value).await;
}

// --- Then: assert on the rendered HTML --------------------------------------

#[then(regex = r#"^the page shows the value "([^"]+)"$"#)]
fn page_shows_value(world: &mut UiWorld, expected: String) {
    assert!(
        world.last_body.contains(&expected),
        "expected page to contain {expected:?}, body was:\n{}",
        world.last_body
    );
}

#[then(regex = r"^the ([\w.]+) field is disabled$")]
fn field_disabled(world: &mut UiWorld, field: String) {
    let tag = world.input_tag(&field);
    assert!(tag.contains("disabled"), "{field} not disabled: {tag}");
}

#[then(regex = r"^the ([\w.]+) field is editable$")]
fn field_editable(world: &mut UiWorld, field: String) {
    let tag = world.input_tag(&field);
    assert!(
        !tag.contains("disabled"),
        "{field} still disabled (expected editable): {tag}"
    );
}

#[then(regex = r"^the page offers to unlock ([\w.]+) to edit it$")]
fn offers_to_unlock(world: &mut UiWorld, field: String) {
    // The identity hint plus an HTMX unlock link to `?unlock=<field>`.
    assert!(
        world.last_body.contains("Identity — the driver owns this"),
        "missing identity hint:\n{}",
        world.last_body
    );
    let link = format!(r#"hx-get="/config/dsd-fp2?unlock={field}""#);
    assert!(
        world.last_body.contains(&link),
        "missing unlock link {link:?}:\n{}",
        world.last_body
    );
}

#[then("the page explains the field is pinned by a command-line override")]
fn explains_pinned(world: &mut UiWorld) {
    assert!(
        world
            .last_body
            .contains("Pinned by a command-line override"),
        "missing pinned explanation:\n{}",
        world.last_body
    );
}

#[then("the page reports the driver is reloading")]
fn reports_reloading(world: &mut UiWorld) {
    assert!(
        world.last_body.contains("reloading") || world.last_body.contains("Reconnecting"),
        "missing reloading state:\n{}",
        world.last_body
    );
}

#[then("the page polls /config/dsd-fp2/status every 1s for reconnection")]
fn polls_for_reconnection(world: &mut UiWorld) {
    assert!(
        world
            .last_body
            .contains(r#"hx-get="/config/dsd-fp2/status""#),
        "missing poll target:\n{}",
        world.last_body
    );
    assert!(
        world.last_body.contains(r#"hx-trigger="every 1s""#),
        "missing poll trigger:\n{}",
        world.last_body
    );
}

#[then("the page reports the configuration was saved without a reload")]
fn reports_saved_no_reload(world: &mut UiWorld) {
    assert!(
        world.last_body.contains("No reload was needed"),
        "missing saved-without-reload banner:\n{}",
        world.last_body
    );
}

#[then(regex = r#"^the form shows the validation error "([^"]+)" on serial\.baud_rate$"#)]
fn shows_validation_error(world: &mut UiWorld, message: String) {
    assert!(
        world.last_body.contains(&message),
        "missing error message {message:?}:\n{}",
        world.last_body
    );
    // The field's wrapper carries the `invalid` class.
    assert!(
        world.last_body.contains("invalid"),
        "baud_rate field not flagged invalid:\n{}",
        world.last_body
    );
}

#[then(regex = r"^the submitted baud_rate value (\d+) is preserved$")]
fn baud_rate_preserved(world: &mut UiWorld, value: String) {
    let tag = world.input_tag("serial.baud_rate");
    assert!(
        tag.contains(&format!("value=\"{value}\"")),
        "submitted baud_rate not preserved: {tag}"
    );
}

#[then("the page shows a driver error")]
fn shows_driver_error(world: &mut UiWorld) {
    assert!(
        world.last_body.contains("could not reach the driver"),
        "missing driver error:\n{}",
        world.last_body
    );
}
