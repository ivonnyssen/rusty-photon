//! Step definitions for connection_lifecycle.feature

use crate::world::FalconRotatorWorld;
use cucumber::{given, then, when};

#[given("a running pa-falcon-rotator service")]
async fn running_service(world: &mut FalconRotatorWorld) {
    world.start_service().await;
}

#[when("I connect the rotator")]
async fn connect_rotator(world: &mut FalconRotatorWorld) {
    world.rotator().set_connected(true).await.unwrap();
}

#[when("I disconnect the rotator")]
async fn disconnect_rotator(world: &mut FalconRotatorWorld) {
    world.rotator().set_connected(false).await.unwrap();
}

#[when("I connect the status switch")]
async fn connect_status_switch(world: &mut FalconRotatorWorld) {
    world.status_switch().set_connected(true).await.unwrap();
}

#[when("I disconnect the status switch")]
async fn disconnect_status_switch(world: &mut FalconRotatorWorld) {
    world.status_switch().set_connected(false).await.unwrap();
}

#[then("the rotator should be connected")]
async fn rotator_should_be_connected(world: &mut FalconRotatorWorld) {
    assert!(world.rotator().connected().await.unwrap());
}

#[then("the rotator should be disconnected")]
async fn rotator_should_be_disconnected(world: &mut FalconRotatorWorld) {
    assert!(!world.rotator().connected().await.unwrap());
}

#[then("the handshake should have issued F# before any other command")]
async fn handshake_issued_ping_first(world: &mut FalconRotatorWorld) {
    let log = world.mock().command_log().await;
    let first = log.first().expect("no commands on the wire");
    assert_eq!(first, "F#", "first command must be F#, got log: {log:?}");
}

#[then("the status switch should be connected")]
async fn status_switch_should_be_connected(world: &mut FalconRotatorWorld) {
    assert!(world.status_switch().connected().await.unwrap());
}

#[then("the status switch should be disconnected")]
async fn status_switch_should_be_disconnected(world: &mut FalconRotatorWorld) {
    assert!(!world.status_switch().connected().await.unwrap());
}

#[then("the handshake should have run exactly once")]
async fn handshake_ran_exactly_once(world: &mut FalconRotatorWorld) {
    // F# is unique to the handshake (no later code path issues it), so
    // counting F# occurrences is a faithful proxy for handshake count.
    let log = world.mock().command_log().await;
    let pings = log.iter().filter(|c| c.as_str() == "F#").count();
    assert_eq!(
        pings, 1,
        "expected exactly one handshake, saw {pings} (log: {log:?})"
    );
}

#[then(expr = "the operation should fail with code {int}")]
async fn operation_should_fail_with(world: &mut FalconRotatorWorld, code: u16) {
    let actual = world
        .last_error_code
        .expect("no error captured — the operation succeeded unexpectedly");
    assert_eq!(
        actual, code,
        "expected ASCOM error code {code}, got {actual}"
    );
}

#[then(expr = "the switch read should fail with code {int}")]
async fn switch_read_should_fail_with(world: &mut FalconRotatorWorld, code: u16) {
    let actual = world
        .last_error_code
        .expect("no error captured — the switch read succeeded unexpectedly");
    assert_eq!(
        actual, code,
        "expected ASCOM error code {code}, got {actual}"
    );
}
