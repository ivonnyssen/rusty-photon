use crate::world::FilemonitorWorld;
use cucumber::{given, then, when};
use std::path::PathBuf;

#[given(expr = "a monitoring file containing {string}")]
fn monitoring_file_containing(world: &mut FilemonitorWorld, content: String) {
    world.create_temp_file(&content);
}

#[given("filemonitor is running")]
async fn filemonitor_running(world: &mut FilemonitorWorld) {
    world.start_filemonitor().await;
}

#[given(expr = "filemonitor is running and monitoring {string}")]
async fn filemonitor_running_monitoring(world: &mut FilemonitorWorld, path: String) {
    world.temp_file_path = Some(PathBuf::from(path));
    world.start_filemonitor().await;
}

#[when("I connect the device")]
async fn connect_device(world: &mut FilemonitorWorld) {
    world.alpaca_put_connected(true).await.unwrap();
}

#[when("I disconnect the device")]
async fn disconnect_device(world: &mut FilemonitorWorld) {
    world.alpaca_put_connected(false).await.unwrap();
}

#[when("I try to connect the device")]
async fn try_connect_device(world: &mut FilemonitorWorld) {
    match world.alpaca_put_connected(true).await {
        Ok(()) => world.last_error = None,
        Err(e) => world.last_error = Some(e),
    }
}

#[then("the device should be disconnected")]
async fn device_should_be_disconnected(world: &mut FilemonitorWorld) {
    let json = world.alpaca_get("connected").await;
    let connected = json["Value"].as_bool().unwrap_or(true);
    assert!(!connected, "expected disconnected but device is connected");
}

#[then("the device should be connected")]
async fn device_should_be_connected(world: &mut FilemonitorWorld) {
    let json = world.alpaca_get("connected").await;
    let connected = json["Value"].as_bool().unwrap_or(false);
    assert!(connected, "expected connected but device is disconnected");
}

#[then("connecting should fail with an error")]
fn connecting_should_fail(world: &mut FilemonitorWorld) {
    assert!(
        world.last_error.is_some(),
        "expected a connection error but none occurred"
    );
}
