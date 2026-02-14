//! Step definitions for polling.feature

use crate::world::QhyFocuserWorld;
use cucumber::{given, when};
use std::time::Duration;

// ============================================================================
// Given steps
// ============================================================================

#[given("a serial manager with fast polling and updated values")]
fn manager_with_fast_polling_and_updated_values(world: &mut QhyFocuserWorld) {
    let mut responses = vec![
        // Handshake
        r#"{"idx": 1, "firmware_version": "2.1.0", "board_version": "1.0"}"#.to_string(),
        r#"{"idx": 13}"#.to_string(),
        r#"{"idx": 5, "pos": 1000}"#.to_string(),
        r#"{"idx": 4, "o_t": 20000, "c_t": 25000, "c_r": 120}"#.to_string(),
    ];
    // Polling responses — updated values that differ from handshake
    for _ in 0..10 {
        responses.push(r#"{"idx": 5, "pos": 2000}"#.to_string());
        responses.push(r#"{"idx": 4, "o_t": 28000, "c_t": 33000, "c_r": 130}"#.to_string());
    }
    world.build_manager_with_fast_polling(responses);
}

// ============================================================================
// When steps
// ============================================================================

#[when("I wait for polling to update")]
async fn wait_for_polling(_world: &mut QhyFocuserWorld) {
    tokio::time::sleep(Duration::from_millis(200)).await;
}
