//! Step definitions for switch_errors.feature

use crate::world::PpbaWorld;
use cucumber::{then, when};

// ============================================================================
// When steps
// ============================================================================

#[when(expr = "I try to get switch {int} value")]
async fn try_get_switch_value(world: &mut PpbaWorld, id: i32) {
    let result = world.switch_ref().get_switch_value(id as usize).await;
    world.capture_result(result);
}

#[when(expr = "I try to get switch {int} boolean")]
async fn try_get_switch_boolean(world: &mut PpbaWorld, id: i32) {
    let result = world.switch_ref().get_switch(id as usize).await;
    world.capture_result(result);
}

#[when(expr = "I try to set switch {int} boolean to true")]
async fn try_set_switch_boolean(world: &mut PpbaWorld, id: i32) {
    let result = world.switch_ref().set_switch(id as usize, true).await;
    world.capture_result(result);
}

#[when(expr = "I try to query can_write for switch {int}")]
async fn try_query_can_write(world: &mut PpbaWorld, id: i32) {
    let result = world.switch_ref().can_write(id as usize).await;
    world.capture_result(result);
}

#[when(expr = "I try to query can_async for switch {int}")]
async fn try_query_can_async(world: &mut PpbaWorld, id: i32) {
    let result = world.switch_ref().can_async(id as usize).await;
    world.capture_result(result);
}

#[when(expr = "I try to query state_change_complete for switch {int}")]
async fn try_query_state_change_complete(world: &mut PpbaWorld, id: i32) {
    let result = world.switch_ref().state_change_complete(id as usize).await;
    world.capture_result(result);
}

#[when(expr = "I try to call cancel_async on switch {int}")]
async fn try_cancel_async(world: &mut PpbaWorld, id: i32) {
    let result = world.switch_ref().cancel_async(id as usize).await;
    world.capture_result(result);
}

#[when(expr = "I try to call set_async on switch {int}")]
async fn try_set_async(world: &mut PpbaWorld, id: i32) {
    let result = world.switch_ref().set_async(id as usize, true).await;
    world.capture_result(result);
}

#[when(expr = "I try to call set_async_value on switch {int}")]
async fn try_set_async_value(world: &mut PpbaWorld, id: i32) {
    let result = world.switch_ref().set_async_value(id as usize, 0.0).await;
    world.capture_result(result);
}

// ============================================================================
// Then steps
// ============================================================================

#[then(expr = "all operations on switch {int} should fail")]
async fn all_operations_on_switch_should_fail(world: &mut PpbaWorld, id: i32) {
    let switch = world.switch_ref();
    let id = id as usize;

    switch.can_write(id).await.unwrap_err();

    switch.get_switch(id).await.unwrap_err();

    switch.get_switch_value(id).await.unwrap_err();

    switch.set_switch(id, true).await.unwrap_err();

    switch.set_switch_value(id, 0.0).await.unwrap_err();

    switch.get_switch_name(id).await.unwrap_err();

    switch.get_switch_description(id).await.unwrap_err();

    switch.min_switch_value(id).await.unwrap_err();

    switch.max_switch_value(id).await.unwrap_err();

    switch.switch_step(id).await.unwrap_err();
}

#[then(expr = "switch {int} name query should fail")]
async fn switch_name_query_should_fail(world: &mut PpbaWorld, id: i32) {
    world
        .switch_ref()
        .get_switch_name(id as usize)
        .await
        .unwrap_err();
}

#[then(expr = "switch {int} description query should fail")]
async fn switch_description_query_should_fail(world: &mut PpbaWorld, id: i32) {
    world
        .switch_ref()
        .get_switch_description(id as usize)
        .await
        .unwrap_err();
}

#[then(expr = "switch {int} min value query should fail")]
async fn switch_min_value_query_should_fail(world: &mut PpbaWorld, id: i32) {
    world
        .switch_ref()
        .min_switch_value(id as usize)
        .await
        .unwrap_err();
}

#[then(expr = "switch {int} max value query should fail")]
async fn switch_max_value_query_should_fail(world: &mut PpbaWorld, id: i32) {
    world
        .switch_ref()
        .max_switch_value(id as usize)
        .await
        .unwrap_err();
}

#[then(expr = "switch {int} step query should fail")]
async fn switch_step_query_should_fail(world: &mut PpbaWorld, id: i32) {
    world
        .switch_ref()
        .switch_step(id as usize)
        .await
        .unwrap_err();
}

#[then(expr = "operations on invalid switch IDs {int}, {int}, {int}, {int} should all fail")]
async fn operations_on_invalid_ids_should_fail(
    world: &mut PpbaWorld,
    id1: i32,
    id2: i32,
    id3: i32,
    id4: i32,
) {
    let switch = world.switch_ref();
    for id in [id1, id2, id3, id4] {
        let id = id as usize;

        switch.can_write(id).await.unwrap_err();
        switch.get_switch(id).await.unwrap_err();
        switch.get_switch_value(id).await.unwrap_err();
        switch.set_switch(id, true).await.unwrap_err();
        switch.set_switch_value(id, 0.0).await.unwrap_err();
        switch.get_switch_name(id).await.unwrap_err();
        switch.get_switch_description(id).await.unwrap_err();
        switch.min_switch_value(id).await.unwrap_err();
        switch.max_switch_value(id).await.unwrap_err();
        switch.switch_step(id).await.unwrap_err();
    }
}

#[then(expr = "can_async should return false for all {int} switches")]
async fn can_async_returns_false_for_all(world: &mut PpbaWorld, count: i32) {
    let switch = world.switch_ref();
    for id in 0..count {
        assert!(
            !switch.can_async(id as usize).await.unwrap(),
            "switch {} should not support async ops",
            id
        );
    }
}

#[then(expr = "state_change_complete should return true for all {int} switches")]
async fn state_change_complete_returns_true_for_all(world: &mut PpbaWorld, count: i32) {
    let switch = world.switch_ref();
    for id in 0..count {
        assert!(
            switch.state_change_complete(id as usize).await.unwrap(),
            "switch {} state change should be complete",
            id
        );
    }
}

#[then(expr = "cancel_async should succeed for all {int} switches")]
async fn cancel_async_succeeds_for_all(world: &mut PpbaWorld, count: i32) {
    let switch = world.switch_ref();
    for id in 0..count {
        switch.cancel_async(id as usize).await.unwrap();
    }
}
