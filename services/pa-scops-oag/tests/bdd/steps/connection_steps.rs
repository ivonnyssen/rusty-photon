//! Step definitions for connection_lifecycle.feature

use crate::world::ScopsWorld;
use cucumber::{given, then, when};

#[given("a running focuser service")]
async fn running_focuser_service(world: &mut ScopsWorld) {
    world.start_focuser().await;
}

#[when("I connect the device")]
async fn connect_device(world: &mut ScopsWorld) {
    world.focuser().set_connected(true).await.unwrap();
}

#[when("I disconnect the device")]
async fn disconnect_device(world: &mut ScopsWorld) {
    world.focuser().set_connected(false).await.unwrap();
}

#[then("the device should be disconnected")]
async fn device_should_be_disconnected(world: &mut ScopsWorld) {
    assert!(!world.focuser().connected().await.unwrap());
}

#[then("the device should be connected")]
async fn device_should_be_connected(world: &mut ScopsWorld) {
    assert!(world.focuser().connected().await.unwrap());
}
