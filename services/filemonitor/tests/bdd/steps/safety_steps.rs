use crate::world::FilemonitorWorld;
use ascom_alpaca::api::{Device, SafetyMonitor};
use cucumber::{given, then, when};
use filemonitor::{
    Config, DeviceConfig, FileConfig, FileMonitorDevice, ParsingConfig, ParsingRule, RuleType,
    ServerConfig,
};
use std::path::PathBuf;
use std::sync::Arc;

#[given("case-insensitive matching")]
fn case_insensitive(world: &mut FilemonitorWorld) {
    world.case_sensitive = false;
}

#[given("case-sensitive matching")]
fn case_sensitive(world: &mut FilemonitorWorld) {
    world.case_sensitive = true;
}

#[given(expr = "a contains rule with pattern {string} that evaluates to safe")]
fn contains_rule_safe(world: &mut FilemonitorWorld, pattern: String) {
    world.rules.push(ParsingRule {
        rule_type: RuleType::Contains,
        pattern,
        safe: true,
    });
}

#[given(expr = "a contains rule with pattern {string} that evaluates to unsafe")]
fn contains_rule_unsafe(world: &mut FilemonitorWorld, pattern: String) {
    world.rules.push(ParsingRule {
        rule_type: RuleType::Contains,
        pattern,
        safe: false,
    });
}

fn resolve_regex_pattern(name: &str) -> String {
    match name {
        "safe_or_ok" => r"Status:\s*(SAFE|OK)".to_string(),
        "danger_or_error" => r"Status:\s*(DANGER|ERROR)".to_string(),
        "invalid_bracket" => "[invalid(".to_string(),
        other => other.to_string(),
    }
}

#[given(expr = "a regex rule with pattern {string} that evaluates to safe")]
fn regex_rule_safe(world: &mut FilemonitorWorld, pattern: String) {
    world.rules.push(ParsingRule {
        rule_type: RuleType::Regex,
        pattern: resolve_regex_pattern(&pattern),
        safe: true,
    });
}

#[given(expr = "a regex rule with pattern {string} that evaluates to unsafe")]
fn regex_rule_unsafe(world: &mut FilemonitorWorld, pattern: String) {
    world.rules.push(ParsingRule {
        rule_type: RuleType::Regex,
        pattern: resolve_regex_pattern(&pattern),
        safe: false,
    });
}

#[given("a device configured with these rules")]
fn device_with_rules(world: &mut FilemonitorWorld) {
    let config = Config {
        device: DeviceConfig {
            name: "Test".to_string(),
            unique_id: "test-001".to_string(),
            description: "Test device".to_string(),
        },
        file: FileConfig {
            path: PathBuf::from("nonexistent.txt"),
            polling_interval_seconds: 60,
        },
        parsing: ParsingConfig {
            rules: world.rules.clone(),
            case_sensitive: world.case_sensitive,
        },
        server: ServerConfig {
            port: 0,
            device_number: 0,
        },
    };
    world.device = Some(Arc::new(FileMonitorDevice::new(config)));
}

#[given("a device configured with these rules and monitoring this file")]
fn device_with_rules_and_file(world: &mut FilemonitorWorld) {
    let path = world.temp_file_path.clone().expect("temp file not created");
    let config = Config {
        device: DeviceConfig {
            name: "Test".to_string(),
            unique_id: "test-001".to_string(),
            description: "Test device".to_string(),
        },
        file: FileConfig {
            path,
            polling_interval_seconds: 60,
        },
        parsing: ParsingConfig {
            rules: world.rules.clone(),
            case_sensitive: world.case_sensitive,
        },
        server: ServerConfig {
            port: 0,
            device_number: 0,
        },
    };
    world.device = Some(Arc::new(FileMonitorDevice::new(config)));
}

#[given(
    expr = "a device configured with these rules and monitoring this file with {int} second polling"
)]
fn device_with_rules_file_and_polling(world: &mut FilemonitorWorld, interval: u64) {
    let path = world.temp_file_path.clone().expect("temp file not created");
    let config = Config {
        device: DeviceConfig {
            name: "Test".to_string(),
            unique_id: "test-001".to_string(),
            description: "Test device".to_string(),
        },
        file: FileConfig {
            path,
            polling_interval_seconds: interval,
        },
        parsing: ParsingConfig {
            rules: world.rules.clone(),
            case_sensitive: world.case_sensitive,
        },
        server: ServerConfig {
            port: 0,
            device_number: 0,
        },
    };
    world.device = Some(Arc::new(FileMonitorDevice::new(config)));
}

#[given("a device configured with no rules and monitoring this file")]
fn device_with_no_rules_and_file(world: &mut FilemonitorWorld) {
    let path = world.temp_file_path.clone().expect("temp file not created");
    let config = Config {
        device: DeviceConfig {
            name: "Test".to_string(),
            unique_id: "test-001".to_string(),
            description: "Test device".to_string(),
        },
        file: FileConfig {
            path,
            polling_interval_seconds: 60,
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

#[when(expr = "I evaluate the safety of {string}")]
fn evaluate_safety(world: &mut FilemonitorWorld, content: String) {
    let device = world.device.as_ref().expect("device not created");
    world.safety_result = Some(device.evaluate_safety(&content));
}

#[then("the result should be safe")]
fn result_should_be_safe(world: &mut FilemonitorWorld) {
    let result = world.safety_result.expect("no safety result");
    assert!(result, "expected safe but got unsafe");
}

#[then("the result should be unsafe")]
fn result_should_be_unsafe(world: &mut FilemonitorWorld) {
    let result = world.safety_result.expect("no safety result");
    assert!(!result, "expected unsafe but got safe");
}

#[then(expr = "is_safe should return {word}")]
async fn is_safe_should_return(world: &mut FilemonitorWorld, expected: String) {
    let device = world.device.as_ref().expect("device not created");
    let result = device.is_safe().await.unwrap();
    let expected_val = expected == "true";
    assert_eq!(
        result, expected_val,
        "expected is_safe={expected_val} but got {result}"
    );
}

#[then(expr = "the static name should be {string}")]
fn static_name_should_be(world: &mut FilemonitorWorld, expected: String) {
    let device = world.device.as_ref().expect("device not created");
    assert_eq!(device.static_name(), expected);
}

#[then(expr = "the description should be {string}")]
async fn description_should_be(world: &mut FilemonitorWorld, expected: String) {
    let device = world.device.as_ref().expect("device not created");
    let description = device.description().await.unwrap();
    assert_eq!(description, expected);
}

#[then(expr = "the driver info should be {string}")]
async fn driver_info_should_be(world: &mut FilemonitorWorld, expected: String) {
    let device = world.device.as_ref().expect("device not created");
    let driver_info = device.driver_info().await.unwrap();
    assert_eq!(driver_info, expected);
}

#[then(expr = "the driver version should be {string}")]
async fn driver_version_should_be(world: &mut FilemonitorWorld, expected: String) {
    let device = world.device.as_ref().expect("device not created");
    let driver_version = device.driver_version().await.unwrap();
    assert_eq!(driver_version, expected);
}
