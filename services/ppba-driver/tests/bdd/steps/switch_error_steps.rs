//! Step definitions for switch_errors.feature

use crate::steps::infrastructure::*;
use crate::world::PpbaWorld;
use cucumber::{then, when};

// ============================================================================
// When steps
// ============================================================================

#[when(expr = "I try to get switch {int} value")]
async fn try_get_switch_value(world: &mut PpbaWorld, id: i32) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, &format!("getswitchvalue?Id={}", id)).await;
    world.capture_response(&resp);
}

#[when(expr = "I try to get switch {int} boolean")]
async fn try_get_switch_boolean(world: &mut PpbaWorld, id: i32) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, &format!("getswitch?Id={}", id)).await;
    world.capture_response(&resp);
}

#[when(expr = "I try to set switch {int} boolean to true")]
async fn try_set_switch_boolean(world: &mut PpbaWorld, id: i32) {
    let url = world.switch_url();
    let id_str = id.to_string();
    let resp = alpaca_put(&url, "setswitch", &[("Id", &id_str), ("State", "true")]).await;
    world.capture_response(&resp);
}

#[when(expr = "I try to query can_write for switch {int}")]
async fn try_query_can_write(world: &mut PpbaWorld, id: i32) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, &format!("canwrite?Id={}", id)).await;
    world.capture_response(&resp);
}

#[when(expr = "I try to query can_async for switch {int}")]
async fn try_query_can_async(world: &mut PpbaWorld, id: i32) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, &format!("canasync?Id={}", id)).await;
    world.capture_response(&resp);
}

#[when(expr = "I try to query state_change_complete for switch {int}")]
async fn try_query_state_change_complete(world: &mut PpbaWorld, id: i32) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, &format!("statechangecomplete?Id={}", id)).await;
    world.capture_response(&resp);
}

#[when(expr = "I try to call cancel_async on switch {int}")]
async fn try_cancel_async(world: &mut PpbaWorld, id: i32) {
    let url = world.switch_url();
    let id_str = id.to_string();
    let resp = alpaca_put(&url, "cancelasync", &[("Id", &id_str)]).await;
    world.capture_response(&resp);
}

#[when(expr = "I try to call set_async on switch {int}")]
async fn try_set_async(world: &mut PpbaWorld, id: i32) {
    let url = world.switch_url();
    let id_str = id.to_string();
    let resp = alpaca_put(&url, "setasync", &[("Id", &id_str), ("State", "true")]).await;
    world.capture_response(&resp);
}

#[when(expr = "I try to call set_async_value on switch {int}")]
async fn try_set_async_value(world: &mut PpbaWorld, id: i32) {
    let url = world.switch_url();
    let id_str = id.to_string();
    let resp = alpaca_put(&url, "setasyncvalue", &[("Id", &id_str), ("Value", "0.0")]).await;
    world.capture_response(&resp);
}

// ============================================================================
// Then steps
// ============================================================================

#[then(expr = "all operations on switch {int} should fail")]
async fn all_operations_on_switch_should_fail(world: &mut PpbaWorld, id: i32) {
    let url = world.switch_url();
    let id_str = id.to_string();

    let resp = alpaca_get(&url, &format!("canwrite?Id={}", id)).await;
    assert!(
        is_alpaca_error(&resp),
        "canwrite should fail for switch {}",
        id
    );

    let resp = alpaca_get(&url, &format!("getswitch?Id={}", id)).await;
    assert!(
        is_alpaca_error(&resp),
        "getswitch should fail for switch {}",
        id
    );

    let resp = alpaca_get(&url, &format!("getswitchvalue?Id={}", id)).await;
    assert!(
        is_alpaca_error(&resp),
        "getswitchvalue should fail for switch {}",
        id
    );

    let resp = alpaca_put(&url, "setswitch", &[("Id", &id_str), ("State", "true")]).await;
    assert!(
        is_alpaca_error(&resp),
        "setswitch should fail for switch {}",
        id
    );

    let resp = alpaca_put(&url, "setswitchvalue", &[("Id", &id_str), ("Value", "0.0")]).await;
    assert!(
        is_alpaca_error(&resp),
        "setswitchvalue should fail for switch {}",
        id
    );

    let resp = alpaca_get(&url, &format!("getswitchname?Id={}", id)).await;
    assert!(
        is_alpaca_error(&resp),
        "getswitchname should fail for switch {}",
        id
    );

    let resp = alpaca_get(&url, &format!("getswitchdescription?Id={}", id)).await;
    assert!(
        is_alpaca_error(&resp),
        "getswitchdescription should fail for switch {}",
        id
    );

    let resp = alpaca_get(&url, &format!("minswitchvalue?Id={}", id)).await;
    assert!(
        is_alpaca_error(&resp),
        "minswitchvalue should fail for switch {}",
        id
    );

    let resp = alpaca_get(&url, &format!("maxswitchvalue?Id={}", id)).await;
    assert!(
        is_alpaca_error(&resp),
        "maxswitchvalue should fail for switch {}",
        id
    );

    let resp = alpaca_get(&url, &format!("switchstep?Id={}", id)).await;
    assert!(
        is_alpaca_error(&resp),
        "switchstep should fail for switch {}",
        id
    );
}

#[then(expr = "switch {int} name query should fail")]
async fn switch_name_query_should_fail(world: &mut PpbaWorld, id: i32) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, &format!("getswitchname?Id={}", id)).await;
    assert!(
        is_alpaca_error(&resp),
        "getswitchname should fail for switch {}",
        id
    );
}

#[then(expr = "switch {int} description query should fail")]
async fn switch_description_query_should_fail(world: &mut PpbaWorld, id: i32) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, &format!("getswitchdescription?Id={}", id)).await;
    assert!(
        is_alpaca_error(&resp),
        "getswitchdescription should fail for switch {}",
        id
    );
}

#[then(expr = "switch {int} min value query should fail")]
async fn switch_min_value_query_should_fail(world: &mut PpbaWorld, id: i32) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, &format!("minswitchvalue?Id={}", id)).await;
    assert!(
        is_alpaca_error(&resp),
        "minswitchvalue should fail for switch {}",
        id
    );
}

#[then(expr = "switch {int} max value query should fail")]
async fn switch_max_value_query_should_fail(world: &mut PpbaWorld, id: i32) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, &format!("maxswitchvalue?Id={}", id)).await;
    assert!(
        is_alpaca_error(&resp),
        "maxswitchvalue should fail for switch {}",
        id
    );
}

#[then(expr = "switch {int} step query should fail")]
async fn switch_step_query_should_fail(world: &mut PpbaWorld, id: i32) {
    let url = world.switch_url();
    let resp = alpaca_get(&url, &format!("switchstep?Id={}", id)).await;
    assert!(
        is_alpaca_error(&resp),
        "switchstep should fail for switch {}",
        id
    );
}

#[then(expr = "operations on invalid switch IDs {int}, {int}, {int}, {int} should all fail")]
async fn operations_on_invalid_ids_should_fail(
    world: &mut PpbaWorld,
    id1: i32,
    id2: i32,
    id3: i32,
    id4: i32,
) {
    let url = world.switch_url();
    for id in [id1, id2, id3, id4] {
        let id_str = id.to_string();

        let resp = alpaca_get(&url, &format!("canwrite?Id={}", id)).await;
        assert!(is_alpaca_error(&resp), "canwrite should fail for ID {}", id);

        let resp = alpaca_get(&url, &format!("getswitch?Id={}", id)).await;
        assert!(
            is_alpaca_error(&resp),
            "getswitch should fail for ID {}",
            id
        );

        let resp = alpaca_get(&url, &format!("getswitchvalue?Id={}", id)).await;
        assert!(
            is_alpaca_error(&resp),
            "getswitchvalue should fail for ID {}",
            id
        );

        let resp = alpaca_put(&url, "setswitch", &[("Id", &id_str), ("State", "true")]).await;
        assert!(
            is_alpaca_error(&resp),
            "setswitch should fail for ID {}",
            id
        );

        let resp = alpaca_put(&url, "setswitchvalue", &[("Id", &id_str), ("Value", "0.0")]).await;
        assert!(
            is_alpaca_error(&resp),
            "setswitchvalue should fail for ID {}",
            id
        );

        let resp = alpaca_get(&url, &format!("getswitchname?Id={}", id)).await;
        assert!(
            is_alpaca_error(&resp),
            "getswitchname should fail for ID {}",
            id
        );

        let resp = alpaca_get(&url, &format!("getswitchdescription?Id={}", id)).await;
        assert!(
            is_alpaca_error(&resp),
            "getswitchdescription should fail for ID {}",
            id
        );

        let resp = alpaca_get(&url, &format!("minswitchvalue?Id={}", id)).await;
        assert!(
            is_alpaca_error(&resp),
            "minswitchvalue should fail for ID {}",
            id
        );

        let resp = alpaca_get(&url, &format!("maxswitchvalue?Id={}", id)).await;
        assert!(
            is_alpaca_error(&resp),
            "maxswitchvalue should fail for ID {}",
            id
        );

        let resp = alpaca_get(&url, &format!("switchstep?Id={}", id)).await;
        assert!(
            is_alpaca_error(&resp),
            "switchstep should fail for ID {}",
            id
        );
    }
}

#[then(expr = "can_async should return false for all {int} switches")]
async fn can_async_returns_false_for_all(world: &mut PpbaWorld, count: i32) {
    let url = world.switch_url();
    for id in 0..count {
        let resp = alpaca_get(&url, &format!("canasync?Id={}", id)).await;
        assert!(!is_alpaca_error(&resp), "canasync failed for switch {}", id);
        assert_eq!(
            alpaca_value(&resp),
            false,
            "switch {} should not support async ops",
            id
        );
    }
}

#[then(expr = "state_change_complete should return true for all {int} switches")]
async fn state_change_complete_returns_true_for_all(world: &mut PpbaWorld, count: i32) {
    let url = world.switch_url();
    for id in 0..count {
        let resp = alpaca_get(&url, &format!("statechangecomplete?Id={}", id)).await;
        assert!(
            !is_alpaca_error(&resp),
            "statechangecomplete failed for switch {}",
            id
        );
        assert_eq!(
            alpaca_value(&resp),
            true,
            "switch {} state change should be complete",
            id
        );
    }
}

#[then(expr = "cancel_async should succeed for all {int} switches")]
async fn cancel_async_succeeds_for_all(world: &mut PpbaWorld, count: i32) {
    let url = world.switch_url();
    for id in 0..count {
        let id_str = id.to_string();
        let resp = alpaca_put(&url, "cancelasync", &[("Id", &id_str)]).await;
        assert!(
            !is_alpaca_error(&resp),
            "cancel_async should succeed for switch {}",
            id
        );
    }
}
