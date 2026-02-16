//! Step definitions for switch_errors.feature

use crate::world::PpbaWorld;
use ascom_alpaca::api::Switch;
use cucumber::{given, then, when};

// ============================================================================
// Given steps
// ============================================================================

#[given("a switch device with set_async delegation mock responses")]
fn switch_device_with_set_async_responses(world: &mut PpbaWorld) {
    world.build_switch_device_with_responses(vec![
        "PPBA_OK".to_string(),
        // connect: status + power stats
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        // poller tick 1: status + power stats
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        // set_async -> set_switch(0, true) -> SetQuad12V command echo
        "P1:1".to_string(),
        // refresh_status after set
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        // spares
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
    ]);
}

#[given("a switch device with set_async_value delegation mock responses")]
fn switch_device_with_set_async_value_responses(world: &mut PpbaWorld) {
    world.build_switch_device_with_responses(vec![
        "PPBA_OK".to_string(),
        // connect: status + power stats
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        // poller tick 1: status + power stats
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        // set_async_value -> set_switch_value(4, 1.0) -> USB hub set echo
        "PU:1".to_string(),
        // spares
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
    ]);
}

// ============================================================================
// When steps
// ============================================================================

#[when(expr = "I try to get switch {int} value")]
async fn try_get_switch_value(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    match device.get_switch_value(id).await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when(expr = "I try to get switch {int} boolean")]
async fn try_get_switch_boolean(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    match device.get_switch(id).await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when(expr = "I try to set switch {int} boolean to true")]
async fn try_set_switch_boolean(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    match device.set_switch(id, true).await {
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

#[when(expr = "I try to query can_write for switch {int}")]
async fn try_query_can_write(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    match device.can_write(id).await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when(expr = "I try to query can_async for switch {int}")]
async fn try_query_can_async(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    match device.can_async(id).await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when(expr = "I try to query state_change_complete for switch {int}")]
async fn try_query_state_change_complete(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    match device.state_change_complete(id).await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when(expr = "I try to call cancel_async on switch {int}")]
async fn try_cancel_async(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    match device.cancel_async(id).await {
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

#[when(expr = "I try to call set_async on switch {int}")]
async fn try_set_async(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    match device.set_async(id, true).await {
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

#[when(expr = "I try to call set_async_value on switch {int}")]
async fn try_set_async_value(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    match device.set_async_value(id, 0.0).await {
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

// ============================================================================
// Then steps
// ============================================================================

#[then(expr = "all operations on switch {int} should fail")]
async fn all_operations_on_switch_should_fail(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    assert!(device.can_write(id).await.is_err());
    assert!(device.get_switch(id).await.is_err());
    assert!(device.get_switch_value(id).await.is_err());
    assert!(device.set_switch(id, true).await.is_err());
    assert!(device.set_switch_value(id, 0.0).await.is_err());
    assert!(device.get_switch_name(id).await.is_err());
    assert!(device.get_switch_description(id).await.is_err());
    assert!(device.min_switch_value(id).await.is_err());
    assert!(device.max_switch_value(id).await.is_err());
    assert!(device.switch_step(id).await.is_err());
}

#[then(expr = "switch {int} name query should fail")]
async fn switch_name_query_should_fail(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    assert!(device.get_switch_name(id).await.is_err());
}

#[then(expr = "switch {int} description query should fail")]
async fn switch_description_query_should_fail(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    assert!(device.get_switch_description(id).await.is_err());
}

#[then(expr = "switch {int} min value query should fail")]
async fn switch_min_value_query_should_fail(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    assert!(device.min_switch_value(id).await.is_err());
}

#[then(expr = "switch {int} max value query should fail")]
async fn switch_max_value_query_should_fail(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    assert!(device.max_switch_value(id).await.is_err());
}

#[then(expr = "switch {int} step query should fail")]
async fn switch_step_query_should_fail(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    assert!(device.switch_step(id).await.is_err());
}

#[then(expr = "operations on invalid switch IDs {int}, {int}, {int}, {int} should all fail")]
async fn operations_on_invalid_ids_should_fail(
    world: &mut PpbaWorld,
    id1: usize,
    id2: usize,
    id3: usize,
    id4: usize,
) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    for id in [id1, id2, id3, id4] {
        assert!(
            device.can_write(id).await.is_err(),
            "can_write should fail for ID {}",
            id
        );
        assert!(
            device.get_switch(id).await.is_err(),
            "get_switch should fail for ID {}",
            id
        );
        assert!(
            device.get_switch_value(id).await.is_err(),
            "get_switch_value should fail for ID {}",
            id
        );
        assert!(
            device.set_switch(id, true).await.is_err(),
            "set_switch should fail for ID {}",
            id
        );
        assert!(
            device.set_switch_value(id, 0.0).await.is_err(),
            "set_switch_value should fail for ID {}",
            id
        );
        assert!(
            device.get_switch_name(id).await.is_err(),
            "get_switch_name should fail for ID {}",
            id
        );
        assert!(
            device.get_switch_description(id).await.is_err(),
            "get_switch_description should fail for ID {}",
            id
        );
        assert!(
            device.min_switch_value(id).await.is_err(),
            "min_switch_value should fail for ID {}",
            id
        );
        assert!(
            device.max_switch_value(id).await.is_err(),
            "max_switch_value should fail for ID {}",
            id
        );
        assert!(
            device.switch_step(id).await.is_err(),
            "switch_step should fail for ID {}",
            id
        );
    }
}

#[then(expr = "can_async should return false for all {int} switches")]
async fn can_async_returns_false_for_all(world: &mut PpbaWorld, count: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    for id in 0..count {
        let result = device.can_async(id).await.unwrap();
        assert!(!result, "Switch {} should not support async ops", id);
    }
}

#[then(expr = "state_change_complete should return true for all {int} switches")]
async fn state_change_complete_returns_true_for_all(world: &mut PpbaWorld, count: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    for id in 0..count {
        let result = device.state_change_complete(id).await.unwrap();
        assert!(result, "Switch {} state change should be complete", id);
    }
}

#[then(expr = "cancel_async should succeed for all {int} switches")]
async fn cancel_async_succeeds_for_all(world: &mut PpbaWorld, count: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    for id in 0..count {
        device.cancel_async(id).await.unwrap();
    }
}

#[then(expr = "calling set_async on switch {int} with true should succeed")]
async fn calling_set_async_should_succeed(world: &mut PpbaWorld, id: usize) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    device.set_async(id, true).await.unwrap();
}

#[then(expr = "calling set_async_value on switch {int} with {float} should succeed")]
async fn calling_set_async_value_should_succeed(world: &mut PpbaWorld, id: usize, value: f64) {
    let device = world
        .switch_device
        .as_ref()
        .expect("switch device not created");
    device.set_async_value(id, value).await.unwrap();
}
