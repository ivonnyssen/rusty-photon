//! Steps for abort.feature.

#![allow(unused_variables)]

use crate::world::StarAdventurerWorld;
use cucumber::{then, when};

#[when("I abort the slew")]
async fn abort_slew(world: &mut StarAdventurerWorld) {
    world.mount().abort_slew().await.unwrap();
}

#[when("I try to abort the slew")]
async fn try_abort_slew(world: &mut StarAdventurerWorld) {
    match world.mount().abort_slew().await {
        Ok(()) => world.last_error = None,
        Err(e) => {
            world.last_error_code = Some(e.code.raw());
            world.last_error = Some(e.message.to_string());
        }
    }
}

#[then("the operation should succeed")]
async fn operation_should_succeed(world: &mut StarAdventurerWorld) {
    assert!(
        world.last_error.is_none(),
        "expected success, got error: {:?}",
        world.last_error
    );
}

#[then("the operation should fail with not-connected")]
async fn fail_not_connected(world: &mut StarAdventurerWorld) {
    use ascom_alpaca::ASCOMErrorCode;
    let code = world.last_error_code.expect("no error captured");
    assert_eq!(code, ASCOMErrorCode::NOT_CONNECTED.raw());
}

#[then("the operation should fail with invalid-value")]
async fn fail_invalid_value(world: &mut StarAdventurerWorld) {
    use ascom_alpaca::ASCOMErrorCode;
    let code = world.last_error_code.expect("no error captured");
    assert_eq!(code, ASCOMErrorCode::INVALID_VALUE.raw());
}

#[then("the operation should fail with invalid-while-parked")]
async fn fail_invalid_while_parked(world: &mut StarAdventurerWorld) {
    use ascom_alpaca::ASCOMErrorCode;
    let code = world.last_error_code.expect("no error captured");
    assert_eq!(code, ASCOMErrorCode::INVALID_WHILE_PARKED.raw());
}

#[then("the operation should fail with invalid-operation")]
async fn fail_invalid_operation(world: &mut StarAdventurerWorld) {
    use ascom_alpaca::ASCOMErrorCode;
    let code = world.last_error_code.expect("no error captured");
    assert_eq!(code, ASCOMErrorCode::INVALID_OPERATION.raw());
}

#[then("the operation should fail with not-implemented")]
async fn fail_not_implemented(world: &mut StarAdventurerWorld) {
    use ascom_alpaca::ASCOMErrorCode;
    let code = world.last_error_code.expect("no error captured");
    assert_eq!(code, ASCOMErrorCode::NOT_IMPLEMENTED.raw());
}
