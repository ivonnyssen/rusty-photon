//! Step definitions for movement.feature

use crate::world::FalconRotatorWorld;
use cucumber::{then, when};

#[when(expr = "I call MoveAbsolute with {float}")]
async fn move_absolute(world: &mut FalconRotatorWorld, position: f64) {
    match world.rotator().move_absolute(position).await {
        Ok(()) => world.last_error_code = None,
        Err(e) => world.last_error_code = Some(e.code.raw()),
    }
}

#[when(expr = "I call Move with {float}")]
async fn move_relative(world: &mut FalconRotatorWorld, delta: f64) {
    match world.rotator().move_(delta).await {
        Ok(()) => world.last_error_code = None,
        Err(e) => world.last_error_code = Some(e.code.raw()),
    }
}

#[when(expr = "I call MoveMechanical with {float}")]
async fn move_mechanical(world: &mut FalconRotatorWorld, mech: f64) {
    match world.rotator().move_mechanical(mech).await {
        Ok(()) => world.last_error_code = None,
        Err(e) => world.last_error_code = Some(e.code.raw()),
    }
}

// Cucumber's `{float}` regex already matches "NaN" (and `f64::from_str`
// parses it), so a dedicated NaN step would create an ambiguous match.
// "Infinity" and "-Infinity" are NOT matched by `{float}` (its regex only
// has `inf`, not `infinity`), so they need explicit steps — `from_str`
// still parses them correctly inside the body.

#[when("I call MoveAbsolute with Infinity")]
async fn move_absolute_infinity(world: &mut FalconRotatorWorld) {
    capture_move_absolute(world, f64::INFINITY).await;
}

#[when("I call MoveAbsolute with -Infinity")]
async fn move_absolute_neg_infinity(world: &mut FalconRotatorWorld) {
    capture_move_absolute(world, f64::NEG_INFINITY).await;
}

#[when("I call Move with Infinity")]
async fn move_infinity(world: &mut FalconRotatorWorld) {
    capture_move(world, f64::INFINITY).await;
}

#[when("I call Move with -Infinity")]
async fn move_neg_infinity(world: &mut FalconRotatorWorld) {
    capture_move(world, f64::NEG_INFINITY).await;
}

#[when("I call MoveMechanical with Infinity")]
async fn move_mechanical_infinity(world: &mut FalconRotatorWorld) {
    capture_move_mechanical(world, f64::INFINITY).await;
}

#[when("I call MoveMechanical with -Infinity")]
async fn move_mechanical_neg_infinity(world: &mut FalconRotatorWorld) {
    capture_move_mechanical(world, f64::NEG_INFINITY).await;
}

async fn capture_move_absolute(world: &mut FalconRotatorWorld, value: f64) {
    match world.rotator().move_absolute(value).await {
        Ok(()) => world.last_error_code = None,
        Err(e) => world.last_error_code = Some(e.code.raw()),
    }
}

async fn capture_move(world: &mut FalconRotatorWorld, value: f64) {
    match world.rotator().move_(value).await {
        Ok(()) => world.last_error_code = None,
        Err(e) => world.last_error_code = Some(e.code.raw()),
    }
}

async fn capture_move_mechanical(world: &mut FalconRotatorWorld, value: f64) {
    match world.rotator().move_mechanical(value).await {
        Ok(()) => world.last_error_code = None,
        Err(e) => world.last_error_code = Some(e.code.raw()),
    }
}

#[when("I read IsMoving")]
async fn read_is_moving(world: &mut FalconRotatorWorld) {
    match world.rotator().is_moving().await {
        Ok(v) => {
            world.is_moving_result = Some(v);
            world.last_error_code = None;
        }
        Err(e) => world.last_error_code = Some(e.code.raw()),
    }
}

#[then(expr = "MD:{float} should have been sent")]
async fn md_command_sent(world: &mut FalconRotatorWorld, value: f64) {
    let expected = format!("MD:{value:.2}");
    let log = world.mock().command_log().await;
    assert!(
        log.iter().any(|c| c == &expected),
        "expected {expected} in wire log, got: {log:?}"
    );
}

#[then(expr = "IsMoving should be {word}")]
async fn is_moving_should_be(world: &mut FalconRotatorWorld, value: String) {
    let expected = match value.as_str() {
        "true" => true,
        "false" => false,
        other => panic!("unexpected IsMoving value '{other}'"),
    };
    let actual = world.is_moving_result.expect("no IsMoving captured");
    assert_eq!(
        actual, expected,
        "expected IsMoving {expected}, got {actual}"
    );
}

#[then(expr = "the move should fail with code {int}")]
async fn move_should_fail_with(world: &mut FalconRotatorWorld, code: u16) {
    let actual = world
        .last_error_code
        .expect("no error captured — the move succeeded unexpectedly");
    assert_eq!(
        actual, code,
        "expected ASCOM error code {code}, got {actual}"
    );
}
