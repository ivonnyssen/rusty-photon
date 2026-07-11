//! Step definitions for the Restart-via-Sentinel feature: a real sentinel is
//! spawned alongside the driver + BFF, its scripted `restart_command` proving
//! the whole BFF → sentinel → shell chain end to end.

use cucumber::{given, then, when};
use serde_json::json;

use crate::dom;
use crate::world::UiWorld;

/// A shell command that writes the marker file. The same line works under both
/// platform shells (`sh -c` on unix, `cmd /C` on windows), quoted path included.
fn write_marker_command(path: &std::path::Path) -> String {
    format!("echo ok > \"{}\"", path.display())
}

#[given(expr = "a sentinel supervising {string} with a restart command that writes a marker file")]
async fn sentinel_with_marker_command(world: &mut UiWorld, service: String) {
    let marker = world.restart_marker_path();
    let services = json!({ service: { "restart_command": write_marker_command(&marker) } });
    world.start_sentinel_and_rewire_bff(services).await;
}

#[given(expr = "a sentinel supervising {string} with a restart command that fails")]
async fn sentinel_with_failing_command(world: &mut UiWorld, service: String) {
    // `exit 1` is understood by both platform shells.
    let services = json!({ service: { "restart_command": "exit 1" } });
    world.start_sentinel_and_rewire_bff(services).await;
}

#[given("a sentinel supervising no services")]
async fn sentinel_with_no_services(world: &mut UiWorld) {
    world.start_sentinel_and_rewire_bff(json!({})).await;
}

#[when("I request a restart of the dsd-fp2 driver")]
async fn request_restart(world: &mut UiWorld) {
    world.request_restart().await;
}

#[then("the page offers to restart the driver via Sentinel")]
fn offers_restart(world: &mut UiWorld) {
    assert_eq!(
        dom::attr(&world.last_body, "button.restart-sentinel", "hx-post").as_deref(),
        Some("/config/dsd-fp2/restart"),
        "missing restart affordance:\n{}",
        world.last_body
    );
}

#[then("the page offers no restart affordance")]
fn no_restart_affordance(world: &mut UiWorld) {
    assert!(
        !dom::matches(&world.last_body, "button.restart-sentinel"),
        "unexpected restart affordance with no sentinel configured:\n{}",
        world.last_body
    );
}

#[then("the restart marker file exists")]
fn restart_marker_exists(world: &mut UiWorld) {
    let marker = world
        .restart_marker
        .as_ref()
        .expect("no marker path recorded");
    assert!(
        marker.exists(),
        "sentinel's restart command did not write {}",
        marker.display()
    );
}

#[then("the page reports the driver is restarting")]
fn reports_restarting(world: &mut UiWorld) {
    assert!(
        dom::text_contains(&world.last_body, "div.banner.applying", "restart"),
        "missing restarting banner:\n{}",
        world.last_body
    );
}

#[then("the page reports the restart failed")]
fn reports_restart_failed(world: &mut UiWorld) {
    assert!(
        dom::text_contains(
            &world.last_body,
            "div.banner.error",
            "could not restart the driver"
        ),
        "missing restart-failed banner:\n{}",
        world.last_body
    );
}

#[then("the page reports Sentinel does not supervise the driver")]
fn reports_not_supervised(world: &mut UiWorld) {
    assert!(
        dom::text_contains(&world.last_body, "div.banner.error", "does not supervise"),
        "missing not-supervised banner:\n{}",
        world.last_body
    );
}
