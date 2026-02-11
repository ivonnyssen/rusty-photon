use crate::world::FilemonitorWorld;
use ascom_alpaca::api::Device;
use cucumber::{given, then, when};
use filemonitor::{
    Config, DeviceConfig, FileConfig, FileMonitorDevice, ParsingConfig, ServerConfig,
};
use std::path::PathBuf;
use std::sync::Arc;

#[given(expr = "a monitoring file containing {string}")]
fn monitoring_file_containing(world: &mut FilemonitorWorld, content: String) {
    world.create_temp_file(&content);
}

#[given("a device configured to monitor this file")]
fn device_configured_to_monitor_file(world: &mut FilemonitorWorld) {
    world.build_device();
}

#[given(expr = "a device configured to monitor {string}")]
fn device_configured_to_monitor_path(world: &mut FilemonitorWorld, path: String) {
    let config = Config {
        device: DeviceConfig {
            name: "Test".to_string(),
            unique_id: "test-001".to_string(),
            description: "Test device".to_string(),
        },
        file: FileConfig {
            path: PathBuf::from(path),
            polling_interval_seconds: 1,
        },
        parsing: ParsingConfig {
            rules: vec![],
            case_sensitive: false,
        },
        server: ServerConfig {
            port: 0,
            device_number: 0,
        },
    };
    world.device = Some(Arc::new(FileMonitorDevice::new(config)));
}

#[when("I connect the device")]
async fn connect_device(world: &mut FilemonitorWorld) {
    let device = world.device.as_ref().expect("device not created");
    device.set_connected(true).await.unwrap();
}

#[when("I disconnect the device")]
async fn disconnect_device(world: &mut FilemonitorWorld) {
    let device = world.device.as_ref().expect("device not created");
    device.set_connected(false).await.unwrap();
}

#[when("I try to connect the device")]
async fn try_connect_device(world: &mut FilemonitorWorld) {
    let device = world.device.as_ref().expect("device not created");
    match device.set_connected(true).await {
        Ok(()) => world.last_error = None,
        Err(e) => world.last_error = Some(e.to_string()),
    }
}

#[then("the device should be disconnected")]
async fn device_should_be_disconnected(world: &mut FilemonitorWorld) {
    let device = world.device.as_ref().expect("device not created");
    let connected = device.connected().await.unwrap();
    assert!(!connected);
}

#[then("the device should be connected")]
async fn device_should_be_connected(world: &mut FilemonitorWorld) {
    let device = world.device.as_ref().expect("device not created");
    let connected = device.connected().await.unwrap();
    assert!(connected);
}

#[then("connecting should fail with an error")]
fn connecting_should_fail(world: &mut FilemonitorWorld) {
    assert!(
        world.last_error.is_some(),
        "expected a connection error but none occurred"
    );
}
