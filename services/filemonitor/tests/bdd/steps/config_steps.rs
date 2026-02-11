use crate::world::FilemonitorWorld;
use cucumber::{given, then, when};
use filemonitor::{load_config, FileMonitorDevice};
use std::path::PathBuf;
use std::sync::Arc;

#[given(expr = "a configuration file at {string}")]
fn config_file_at(world: &mut FilemonitorWorld, path: String) {
    world.last_error = None;
    // Store path in config temporarily by attempting to load
    let config_path = PathBuf::from(&path);
    match load_config(&config_path) {
        Ok(config) => world.config = Some(config),
        Err(e) => world.last_error = Some(e.to_string()),
    }
}

#[when("I load the configuration")]
fn load_configuration(_world: &mut FilemonitorWorld) {
    // Config was already loaded in the given step; this is a no-op if successful
    // The given step already set config or last_error
}

#[when("I try to load the configuration")]
fn try_load_configuration(_world: &mut FilemonitorWorld) {
    // Same as load - error was already captured in given step
}

#[when("I create a device from the configuration")]
fn create_device_from_config(world: &mut FilemonitorWorld) {
    let config = world.config.as_ref().expect("config not loaded").clone();
    world.device = Some(Arc::new(FileMonitorDevice::new(config)));
}

#[then(expr = "the device name should be {string}")]
fn device_name_should_be(world: &mut FilemonitorWorld, expected: String) {
    let config = world.config.as_ref().expect("config not loaded");
    assert_eq!(config.device.name, expected);
}

#[then(expr = "the unique ID should be {string}")]
fn unique_id_should_be(world: &mut FilemonitorWorld, expected: String) {
    let config = world.config.as_ref().expect("config not loaded");
    assert_eq!(config.device.unique_id, expected);
}

#[then(expr = "the polling interval should be {int} seconds")]
fn polling_interval_should_be(world: &mut FilemonitorWorld, expected: u64) {
    let config = world.config.as_ref().expect("config not loaded");
    assert_eq!(config.file.polling_interval_seconds, expected);
}

#[then(expr = "there should be {int} parsing rules")]
fn parsing_rules_count(world: &mut FilemonitorWorld, expected: usize) {
    let config = world.config.as_ref().expect("config not loaded");
    assert_eq!(config.parsing.rules.len(), expected);
}

#[then(expr = "the server port should be {int}")]
fn server_port_should_be(world: &mut FilemonitorWorld, expected: u16) {
    let config = world.config.as_ref().expect("config not loaded");
    assert_eq!(config.server.port, expected);
}

#[then(expr = "rule {int} should have pattern {string} and be safe")]
fn rule_should_be_safe(world: &mut FilemonitorWorld, index: usize, pattern: String) {
    let config = world.config.as_ref().expect("config not loaded");
    let rule = &config.parsing.rules[index - 1];
    assert_eq!(rule.pattern, pattern);
    assert!(rule.safe);
}

#[then(expr = "rule {int} should have pattern {string} and be unsafe")]
fn rule_should_be_unsafe(world: &mut FilemonitorWorld, index: usize, pattern: String) {
    let config = world.config.as_ref().expect("config not loaded");
    let rule = &config.parsing.rules[index - 1];
    assert_eq!(rule.pattern, pattern);
    assert!(!rule.safe);
}

#[then("case sensitivity should be disabled")]
fn case_sensitivity_disabled(world: &mut FilemonitorWorld) {
    let config = world.config.as_ref().expect("config not loaded");
    assert!(!config.parsing.case_sensitive);
}

#[then("the device should exist")]
fn device_should_exist(world: &mut FilemonitorWorld) {
    assert!(world.device.is_some());
}

#[then("loading should fail with an error")]
fn loading_should_fail(world: &mut FilemonitorWorld) {
    assert!(
        world.last_error.is_some(),
        "expected an error but config loaded successfully"
    );
}
