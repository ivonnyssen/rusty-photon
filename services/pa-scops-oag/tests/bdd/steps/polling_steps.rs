//! Step definitions for polling.feature

use crate::world::ScopsWorld;
use cucumber::{given, when};
use pa_scops_oag::Config;
use std::time::Duration;

#[given("a focuser service with fast polling")]
async fn focuser_with_fast_polling(world: &mut ScopsWorld) {
    let mut config = Config::default();
    config.serial.polling_interval = Duration::from_millis(50);
    world.config = Some(config);
    world.start_focuser().await;
}

#[when("I wait for polling to update")]
async fn wait_for_polling(_world: &mut ScopsWorld) {
    tokio::time::sleep(Duration::from_millis(2000)).await;
}
