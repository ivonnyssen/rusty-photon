use ascom_alpaca::api::Device;
use filemonitor::{
    load_config, Config, DeviceConfig, FileConfig, FileMonitorDevice, ParsingConfig, ParsingRule,
    RuleType, ServerConfig,
};
use std::path::PathBuf;

#[test]
fn test_load_config() {
    let config_path = PathBuf::from("tests/config.json");
    let config = load_config(&config_path).unwrap();

    assert_eq!(config.device.name, "File Safety Monitor");
    assert_eq!(config.device.unique_id, "filemonitor-001");
    assert_eq!(config.file.polling_interval_seconds, 60);
    assert_eq!(config.parsing.rules.len(), 3);
    assert_eq!(config.server.port, 11111);
}

#[test]
fn test_config_parsing_rules() {
    let config_path = PathBuf::from("tests/config.json");
    let config = load_config(&config_path).unwrap();

    assert_eq!(config.parsing.rules[0].pattern, "CLOSED");
    assert!(config.parsing.rules[0].safe);
    assert_eq!(config.parsing.rules[1].pattern, "OPEN");
    assert!(!config.parsing.rules[1].safe);
    assert!(!config.parsing.default_safe);
    assert!(!config.parsing.case_sensitive);
}

#[test]
fn test_device_creation() {
    let config_path = PathBuf::from("tests/config.json");
    let config = load_config(&config_path).unwrap();
    let device = FileMonitorDevice::new(config);

    // Device should be created successfully
    assert!(std::mem::size_of_val(&device) > 0);
}

#[tokio::test]
async fn test_device_connected_with_existing_file() {
    let config_path = PathBuf::from("tests/config.json");
    let config = load_config(&config_path).unwrap();
    let device = FileMonitorDevice::new(config);

    // Device should start disconnected
    let connected = device.connected().await.unwrap();
    assert!(!connected);

    // After calling set_connected(true), should be connected
    device.set_connected(true).await.unwrap();
    let connected = device.connected().await.unwrap();
    assert!(connected);
}

#[tokio::test]
async fn test_device_not_connected_with_missing_file() {
    let config_path = PathBuf::from("tests/config.json");
    let mut config = load_config(&config_path).unwrap();

    // Change path to non-existent file
    config.file.path = PathBuf::from("tests/nonexistent.txt");
    let device = FileMonitorDevice::new(config);

    // Should not be connected since file doesn't exist
    let connected = device.connected().await.unwrap();
    assert!(!connected);
}

#[test]
fn test_load_invalid_json_config() {
    let config_path = PathBuf::from("tests/invalid_config.json");
    let result = load_config(&config_path);

    // Should fail to parse invalid JSON
    assert!(result.is_err());
}

#[test]
fn test_evaluate_safety_contains_rules() {
    let config = Config {
        device: DeviceConfig {
            name: "Test".to_string(),
            unique_id: "test-001".to_string(),
            description: "Test device".to_string(),
        },
        file: FileConfig {
            path: PathBuf::from("test.txt"),
            polling_interval_seconds: 60,
        },
        parsing: ParsingConfig {
            rules: vec![
                ParsingRule {
                    rule_type: RuleType::Contains,
                    pattern: "CLOSED".to_string(),
                    safe: true,
                },
                ParsingRule {
                    rule_type: RuleType::Contains,
                    pattern: "OPEN".to_string(),
                    safe: false,
                },
            ],
            default_safe: false,
            case_sensitive: false,
        },
        server: ServerConfig {
            port: 11111,
            device_number: 0,
        },
    };

    let device = FileMonitorDevice::new(config);

    // Test safe condition
    assert!(device.evaluate_safety("Roof Status: CLOSED"));

    // Test unsafe condition
    assert!(!device.evaluate_safety("Roof Status: OPEN"));

    // Test case insensitive matching
    assert!(device.evaluate_safety("roof status: closed"));
    assert!(!device.evaluate_safety("roof status: open"));

    // Test no match returns default
    assert!(!device.evaluate_safety("Unknown status"));
}

#[test]
fn test_evaluate_safety_case_sensitive() {
    let config = Config {
        device: DeviceConfig {
            name: "Test".to_string(),
            unique_id: "test-001".to_string(),
            description: "Test device".to_string(),
        },
        file: FileConfig {
            path: PathBuf::from("test.txt"),
            polling_interval_seconds: 60,
        },
        parsing: ParsingConfig {
            rules: vec![ParsingRule {
                rule_type: RuleType::Contains,
                pattern: "CLOSED".to_string(),
                safe: true,
            }],
            default_safe: false,
            case_sensitive: true,
        },
        server: ServerConfig {
            port: 11111,
            device_number: 0,
        },
    };

    let device = FileMonitorDevice::new(config);

    // Test exact case match
    assert!(device.evaluate_safety("Status: CLOSED"));

    // Test case mismatch doesn't match when case sensitive
    assert!(!device.evaluate_safety("Status: closed"));
}

#[test]
fn test_evaluate_safety_regex_rules() {
    let config = Config {
        device: DeviceConfig {
            name: "Test".to_string(),
            unique_id: "test-001".to_string(),
            description: "Test device".to_string(),
        },
        file: FileConfig {
            path: PathBuf::from("test.txt"),
            polling_interval_seconds: 60,
        },
        parsing: ParsingConfig {
            rules: vec![
                ParsingRule {
                    rule_type: RuleType::Regex,
                    pattern: r"Status:\s*(SAFE|OK)".to_string(),
                    safe: true,
                },
                ParsingRule {
                    rule_type: RuleType::Regex,
                    pattern: r"Status:\s*(DANGER|ERROR)".to_string(),
                    safe: false,
                },
            ],
            default_safe: false,
            case_sensitive: false,
        },
        server: ServerConfig {
            port: 11111,
            device_number: 0,
        },
    };

    let device = FileMonitorDevice::new(config);

    // Test regex matches
    assert!(device.evaluate_safety("Status: SAFE"));
    assert!(device.evaluate_safety("Status:   OK"));
    assert!(!device.evaluate_safety("Status: DANGER"));
    assert!(!device.evaluate_safety("Status:ERROR"));

    // Test no match returns default
    assert!(!device.evaluate_safety("Status: UNKNOWN"));
}

#[test]
fn test_evaluate_safety_first_match_wins() {
    let config = Config {
        device: DeviceConfig {
            name: "Test".to_string(),
            unique_id: "test-001".to_string(),
            description: "Test device".to_string(),
        },
        file: FileConfig {
            path: PathBuf::from("test.txt"),
            polling_interval_seconds: 60,
        },
        parsing: ParsingConfig {
            rules: vec![
                ParsingRule {
                    rule_type: RuleType::Contains,
                    pattern: "SAFE".to_string(),
                    safe: true,
                },
                ParsingRule {
                    rule_type: RuleType::Contains,
                    pattern: "SAFE".to_string(),
                    safe: false,
                },
            ],
            default_safe: false,
            case_sensitive: false,
        },
        server: ServerConfig {
            port: 11111,
            device_number: 0,
        },
    };

    let device = FileMonitorDevice::new(config);

    // First rule should win (safe: true)
    assert!(device.evaluate_safety("Status: SAFE"));
}

#[test]
fn test_evaluate_safety_invalid_regex() {
    let config = Config {
        device: DeviceConfig {
            name: "Test".to_string(),
            unique_id: "test-001".to_string(),
            description: "Test device".to_string(),
        },
        file: FileConfig {
            path: PathBuf::from("test.txt"),
            polling_interval_seconds: 60,
        },
        parsing: ParsingConfig {
            rules: vec![ParsingRule {
                rule_type: RuleType::Regex,
                pattern: "[invalid regex(".to_string(),
                safe: true,
            }],
            default_safe: false,
            case_sensitive: false,
        },
        server: ServerConfig {
            port: 11111,
            device_number: 0,
        },
    };

    let device = FileMonitorDevice::new(config);

    // Invalid regex should not match, return default
    assert!(!device.evaluate_safety("any content"));
}

#[tokio::test]
async fn test_is_safe_when_disconnected() {
    use ascom_alpaca::api::SafetyMonitor;

    let config_path = PathBuf::from("tests/config.json");
    let config = load_config(&config_path).unwrap();
    let device = FileMonitorDevice::new(config);

    // Device starts disconnected, is_safe should return false
    let result = device.is_safe().await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), false);
}
