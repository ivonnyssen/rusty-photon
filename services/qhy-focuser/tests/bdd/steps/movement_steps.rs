//! Step definitions for movement_control.feature

use crate::world::QhyFocuserWorld;
use ascom_alpaca::ASCOMErrorCode;
use cucumber::{then, when};
use std::time::Duration;

// ============================================================================
// When steps
// ============================================================================

#[when(expr = "I move the focuser to position {int}")]
async fn move_focuser(world: &mut QhyFocuserWorld, position: i32) {
    world.focuser().move_(position).await.unwrap();
}

#[when(expr = "I try to move the focuser to position {int}")]
async fn try_move_focuser(world: &mut QhyFocuserWorld, position: i32) {
    match world.focuser().move_(position).await {
        Ok(()) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I halt the focuser")]
async fn halt_focuser(world: &mut QhyFocuserWorld) {
    world.focuser().halt().await.unwrap();
}

#[when("I try to halt the focuser")]
async fn try_halt_focuser(world: &mut QhyFocuserWorld) {
    match world.focuser().halt().await {
        Ok(()) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I wait for the move to complete")]
async fn wait_for_move_complete(world: &mut QhyFocuserWorld) {
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if !world.focuser().is_moving().await.unwrap() {
            return;
        }
    }
    panic!("focuser did not finish moving within 30 seconds");
}

// ============================================================================
// Then steps
// ============================================================================

#[then("the operation should fail with invalid-value")]
fn operation_should_fail_invalid_value(world: &mut QhyFocuserWorld) {
    let code = world
        .last_error_code
        .expect("expected an error but none occurred");
    assert_eq!(
        code,
        ASCOMErrorCode::INVALID_VALUE.raw(),
        "expected INVALID_VALUE error code, got: {}",
        code
    );
}
