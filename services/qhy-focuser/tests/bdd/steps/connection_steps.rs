//! Step definitions for connection_lifecycle.feature

use crate::world::QhyFocuserWorld;
use cucumber::{given, then, when};

// ============================================================================
// Given steps
// ============================================================================

#[given("a running focuser service")]
async fn running_focuser_service(world: &mut QhyFocuserWorld) {
    world.start_focuser().await;
}

// ============================================================================
// When steps
// ============================================================================

#[when("I connect the device")]
async fn connect_device(world: &mut QhyFocuserWorld) {
    world.focuser().set_connected(true).await.unwrap();
}

#[when("I disconnect the device")]
async fn disconnect_device(world: &mut QhyFocuserWorld) {
    world.focuser().set_connected(false).await.unwrap();
}

// ============================================================================
// Then steps
// ============================================================================

#[then("the device should be disconnected")]
async fn device_should_be_disconnected(world: &mut QhyFocuserWorld) {
    assert!(!world.focuser().connected().await.unwrap());
}

#[then("the device should be connected")]
async fn device_should_be_connected(world: &mut QhyFocuserWorld) {
    assert!(world.focuser().connected().await.unwrap());
}

#[then(expr = "connecting should fail with an error containing {string}")]
fn connecting_should_fail_with(world: &mut QhyFocuserWorld, expected: String) {
    let error = world
        .last_error
        .as_ref()
        .expect("expected a connection error but none occurred");
    assert!(
        error.contains(&expected),
        "expected error containing '{}', got: {}",
        expected,
        error
    );
}
