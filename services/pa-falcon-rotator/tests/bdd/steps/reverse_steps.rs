//! Step definitions for reverse.feature

use crate::world::FalconRotatorWorld;
use cucumber::{given, then, when};

fn parse_bool(s: &str) -> bool {
    match s {
        "true" => true,
        "false" => false,
        other => panic!("expected 'true' or 'false', got '{other}'"),
    }
}

// `the device's motor_reverse is currently …` appears as `And` under
// `When` in every reverse scenario, so register the step under both Given
// and When — Gherkin resolves the keyword from the preceding step.
#[given(expr = "the device's motor_reverse is currently {word}")]
#[when(expr = "the device's motor_reverse is currently {word}")]
async fn given_motor_reverse(world: &mut FalconRotatorWorld, value: String) {
    world.mock().set_motor_reverse(parse_bool(&value)).await;
}

#[when(expr = "I set Reverse to {word}")]
async fn set_reverse(world: &mut FalconRotatorWorld, value: String) {
    match world.rotator().set_reverse(parse_bool(&value)).await {
        Ok(()) => world.last_error_code = None,
        Err(e) => world.last_error_code = Some(e.code.raw()),
    }
}

#[when("I read Reverse")]
async fn read_reverse(world: &mut FalconRotatorWorld) {
    match world.rotator().reverse().await {
        Ok(v) => {
            world.reverse_result = Some(v);
            world.last_error_code = None;
        }
        Err(e) => world.last_error_code = Some(e.code.raw()),
    }
}

#[then(expr = "Reverse should be {word}")]
async fn reverse_should_be(world: &mut FalconRotatorWorld, value: String) {
    let expected = parse_bool(&value);
    let actual = world.reverse_result.expect("no Reverse captured");
    assert_eq!(actual, expected);
}

#[then("no FN command should have been sent")]
async fn no_fn_command(world: &mut FalconRotatorWorld) {
    let log = world.mock().command_log().await;
    assert!(
        !log.iter().any(|c| c.starts_with("FN")),
        "unexpected FN in wire log: {log:?}"
    );
}

#[then(expr = "FN:{word} should have been sent")]
async fn fn_command_sent(world: &mut FalconRotatorWorld, value: String) {
    let expected = format!("FN:{value}");
    let log = world.mock().command_log().await;
    assert!(
        log.iter().any(|c| c == &expected),
        "expected {expected} in wire log, got: {log:?}"
    );
}
