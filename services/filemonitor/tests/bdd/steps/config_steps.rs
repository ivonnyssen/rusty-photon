use crate::steps::infrastructure::ServiceHandle;
use crate::world::FilemonitorWorld;
use cucumber::{given, then, when};
use serde_json::Value;

#[given(expr = "a configuration file at {string}")]
fn config_file_at(world: &mut FilemonitorWorld, path: String) {
    world.config_path = Some(path.clone());
    world.last_error = None;
    match std::fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str::<Value>(&content) {
            Ok(config) => world.loaded_config = Some(config),
            Err(e) => world.last_error = Some(e.to_string()),
        },
        Err(e) => world.last_error = Some(e.to_string()),
    }
}

#[given(expr = "filemonitor is running with configuration {string}")]
async fn filemonitor_running_with_config(world: &mut FilemonitorWorld, path: String) {
    world.start_filemonitor_with_config(&path).await;
}

#[when("I load the configuration")]
fn load_configuration(_world: &mut FilemonitorWorld) {
    // Config was already loaded in the given step
}

#[when("I try to load the configuration")]
fn try_load_configuration(_world: &mut FilemonitorWorld) {
    // Same as load - error was already captured in given step
}

#[when("I try to start filemonitor with this configuration")]
async fn try_start_with_config(world: &mut FilemonitorWorld) {
    let path = world
        .config_path
        .as_ref()
        .expect("config path not set")
        .clone();
    match ServiceHandle::try_start(env!("CARGO_PKG_NAME"), &path).await {
        Ok(handle) => {
            let monitor = world.acquire_monitor(&handle).await;
            world.monitor = Some(monitor);
            world.filemonitor = Some(handle);
            world.last_error = None;
        }
        Err(e) => world.last_error = Some(e),
    }
}

#[then(expr = "the device name should be {string}")]
fn device_name_should_be(world: &mut FilemonitorWorld, expected: String) {
    let config = world.loaded_config.as_ref().expect("config not loaded");
    assert_eq!(config["device"]["name"].as_str().unwrap(), expected);
}

#[then(expr = "the unique ID should be {string}")]
fn unique_id_should_be(world: &mut FilemonitorWorld, expected: String) {
    let config = world.loaded_config.as_ref().expect("config not loaded");
    assert_eq!(config["device"]["unique_id"].as_str().unwrap(), expected);
}

#[then(expr = "the polling interval should be {int} seconds")]
fn polling_interval_should_be(world: &mut FilemonitorWorld, expected: u64) {
    let config = world.loaded_config.as_ref().expect("config not loaded");
    assert_eq!(
        config["file"]["polling_interval_seconds"].as_u64().unwrap(),
        expected
    );
}

#[then(expr = "there should be {int} parsing rules")]
fn parsing_rules_count(world: &mut FilemonitorWorld, expected: usize) {
    let config = world.loaded_config.as_ref().expect("config not loaded");
    assert_eq!(
        config["parsing"]["rules"].as_array().unwrap().len(),
        expected
    );
}

#[then(expr = "the server port should be {int}")]
fn server_port_should_be(world: &mut FilemonitorWorld, expected: u64) {
    let config = world.loaded_config.as_ref().expect("config not loaded");
    assert_eq!(config["server"]["port"].as_u64().unwrap(), expected);
}

#[then(expr = "rule {int} should have pattern {string} and be safe")]
fn rule_should_be_safe(world: &mut FilemonitorWorld, index: usize, pattern: String) {
    let config = world.loaded_config.as_ref().expect("config not loaded");
    let rule = &config["parsing"]["rules"].as_array().unwrap()[index - 1];
    assert_eq!(rule["pattern"].as_str().unwrap(), pattern);
    assert!(rule["safe"].as_bool().unwrap());
}

#[then(expr = "rule {int} should have pattern {string} and be unsafe")]
fn rule_should_be_unsafe(world: &mut FilemonitorWorld, index: usize, pattern: String) {
    let config = world.loaded_config.as_ref().expect("config not loaded");
    let rule = &config["parsing"]["rules"].as_array().unwrap()[index - 1];
    assert_eq!(rule["pattern"].as_str().unwrap(), pattern);
    assert!(!rule["safe"].as_bool().unwrap());
}

#[then("case sensitivity should be disabled")]
fn case_sensitivity_disabled(world: &mut FilemonitorWorld) {
    let config = world.loaded_config.as_ref().expect("config not loaded");
    assert!(!config["parsing"]["case_sensitive"].as_bool().unwrap());
}

#[then("the binary should fail to start")]
fn binary_should_fail(world: &mut FilemonitorWorld) {
    assert!(
        world.last_error.is_some(),
        "expected the binary to fail but it started successfully"
    );
}

#[then("loading should fail with an error")]
fn loading_should_fail(world: &mut FilemonitorWorld) {
    assert!(
        world.last_error.is_some(),
        "expected an error but config loaded successfully"
    );
}
