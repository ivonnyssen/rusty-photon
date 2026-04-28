//! Step definitions for polling.feature

use crate::world::QhyFocuserWorld;
use cucumber::{given, when};
use qhy_focuser::Config;
use std::time::Duration;

// ============================================================================
// Given steps
// ============================================================================

#[given("a focuser service with fast polling")]
async fn focuser_with_fast_polling(world: &mut QhyFocuserWorld) {
    let mut config = Config::default();
    config.serial.polling_interval = Duration::from_millis(50);
    world.config = Some(config);
    world.start_focuser().await;
}

// ============================================================================
// When steps
// ============================================================================

#[when("I wait for polling to update")]
async fn wait_for_polling(_world: &mut QhyFocuserWorld) {
    tokio::time::sleep(Duration::from_millis(2000)).await;
}
