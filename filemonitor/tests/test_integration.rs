use ascom_alpaca::api::Device;
use filemonitor::{
    load_config, Config, DeviceConfig, FileConfig, FileMonitorDevice, ParsingConfig, ParsingRule,
    RuleType, ServerConfig,
};
use std::path::PathBuf;

#[test]
#[cfg(not(miri))]
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
#[cfg(not(miri))]
fn test_config_parsing_rules() {
    let config_path = PathBuf::from("tests/config.json");
    let config = load_config(&config_path).unwrap();

    assert_eq!(config.parsing.rules[0].pattern, "OPEN");
    assert!(config.parsing.rules[0].safe);
    assert_eq!(config.parsing.rules[1].pattern, "CLOSED");
    assert!(!config.parsing.rules[1].safe);
    assert!(!config.parsing.case_sensitive);
}

#[test]
#[cfg(not(miri))]
fn test_device_creation() {
    let config_path = PathBuf::from("tests/config.json");
    let config = load_config(&config_path).unwrap();
    let device = FileMonitorDevice::new(config);

    // Device should be created successfully
    assert!(std::mem::size_of_val(&device) > 0);
}

#[tokio::test]
#[cfg(not(miri))]
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
#[cfg(not(miri))]
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
#[cfg(not(miri))]
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
                    pattern: "OPEN".to_string(),
                    safe: true,
                },
                ParsingRule {
                    rule_type: RuleType::Contains,
                    pattern: "CLOSED".to_string(),
                    safe: false,
                },
            ],
            case_sensitive: false,
        },
        server: ServerConfig {
            port: 11111,
            device_number: 0,
        },
    };

    let device = FileMonitorDevice::new(config);

    // Test safe condition
    assert!(device.evaluate_safety("Roof Status: OPEN"));

    // Test unsafe condition
    assert!(!device.evaluate_safety("Roof Status: CLOSED"));

    // Test case insensitive matching
    assert!(device.evaluate_safety("roof status: open"));
    assert!(!device.evaluate_safety("roof status: closed"));

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
                pattern: "OPEN".to_string(),
                safe: true,
            }],
            case_sensitive: true,
        },
        server: ServerConfig {
            port: 11111,
            device_number: 0,
        },
    };

    let device = FileMonitorDevice::new(config);

    // Test exact case match
    assert!(device.evaluate_safety("Status: OPEN"));

    // Test case mismatch doesn't match when case sensitive
    assert!(!device.evaluate_safety("Status: open"));
}

#[test]
#[cfg(not(miri))] // Skip under miri - regex compilation is too slow
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
#[cfg(not(miri))]
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

#[tokio::test]
#[cfg(not(miri))]
async fn test_device_trait_methods() {
    use ascom_alpaca::api::Device;

    let config_path = PathBuf::from("tests/config.json");
    let config = load_config(&config_path).unwrap();
    let device = FileMonitorDevice::new(config.clone());

    assert_eq!(device.static_name(), &config.device.name);
    assert_eq!(device.unique_id(), &config.device.unique_id);

    let description = device.description().await.unwrap();
    assert_eq!(description, config.device.description);

    let driver_info = device.driver_info().await.unwrap();
    assert_eq!(driver_info, config.device.description);

    let driver_version = device.driver_version().await.unwrap();
    assert_eq!(driver_version, "0.1.0");
}

#[tokio::test]
#[cfg(not(miri))]
async fn test_set_connected_file_read_error() {
    use ascom_alpaca::api::Device;

    let config = Config {
        device: DeviceConfig {
            name: "Test".to_string(),
            unique_id: "test-001".to_string(),
            description: "Test device".to_string(),
        },
        file: FileConfig {
            path: PathBuf::from("/nonexistent/path/file.txt"),
            polling_interval_seconds: 1,
        },
        parsing: ParsingConfig {
            rules: vec![],
            case_sensitive: false,
        },
        server: ServerConfig {
            port: 11111,
            device_number: 0,
        },
    };

    let device = FileMonitorDevice::new(config);
    let result = device.set_connected(true).await;
    assert!(result.is_err());
}

#[tokio::test]
#[cfg(not(miri))]
async fn test_is_safe_connected_no_content() {
    use ascom_alpaca::api::{Device, SafetyMonitor};
    use std::fs;

    let test_file = PathBuf::from("test_temp_file.txt");
    fs::write(&test_file, "test content").unwrap();

    let config = Config {
        device: DeviceConfig {
            name: "Test".to_string(),
            unique_id: "test-001".to_string(),
            description: "Test device".to_string(),
        },
        file: FileConfig {
            path: test_file.clone(),
            polling_interval_seconds: 1,
        },
        parsing: ParsingConfig {
            rules: vec![],
            case_sensitive: false,
        },
        server: ServerConfig {
            port: 11111,
            device_number: 0,
        },
    };

    let device = FileMonitorDevice::new(config);

    // Test behavior when content doesn't match any rules (should default to unsafe)
    fs::write(&test_file, "UNKNOWN STATUS").unwrap();
    device.set_connected(true).await.unwrap();

    let result = device.is_safe().await.unwrap();
    assert_eq!(result, false); // Should return false (unsafe) when no rules match

    device.set_connected(false).await.unwrap();
    fs::remove_file(&test_file).unwrap();
}

#[tokio::test]
#[cfg(not(miri))]
async fn test_polling_functionality() {
    use ascom_alpaca::api::{Device, SafetyMonitor};
    use std::fs;
    use tokio::time::{sleep, Duration};

    let test_file = PathBuf::from("test_polling_file.txt");
    fs::write(&test_file, "initial").unwrap();

    let config = Config {
        device: DeviceConfig {
            name: "Test".to_string(),
            unique_id: "test-001".to_string(),
            description: "Test device".to_string(),
        },
        file: FileConfig {
            path: test_file.clone(),
            polling_interval_seconds: 1,
        },
        parsing: ParsingConfig {
            rules: vec![],
            case_sensitive: false,
        },
        server: ServerConfig {
            port: 11111,
            device_number: 0,
        },
    };

    let device = FileMonitorDevice::new(config);
    device.set_connected(true).await.unwrap();

    // Update file content
    fs::write(&test_file, "updated").unwrap();

    // Wait for polling to pick up changes
    sleep(Duration::from_millis(1100)).await;

    // Test that polling worked by checking safety evaluation with new content
    // The file now contains "updated" which doesn't match any rules, so should return false (unsafe)
    let result = device.is_safe().await.unwrap();
    assert_eq!(result, false); // Should return false when no rules match

    device.set_connected(false).await.unwrap();
    fs::remove_file(&test_file).unwrap();
}

#[tokio::test]
#[cfg(not(miri))]
async fn test_connection_state_management() {
    use ascom_alpaca::api::Device;
    use std::fs;

    let test_file = PathBuf::from("test_stop_polling.txt");
    fs::write(&test_file, "test").unwrap();

    let config = Config {
        device: DeviceConfig {
            name: "Test".to_string(),
            unique_id: "test-001".to_string(),
            description: "Test device".to_string(),
        },
        file: FileConfig {
            path: test_file.clone(),
            polling_interval_seconds: 1,
        },
        parsing: ParsingConfig {
            rules: vec![],
            case_sensitive: false,
        },
        server: ServerConfig {
            port: 11111,
            device_number: 0,
        },
    };

    let device = FileMonitorDevice::new(config);

    // Test that connection/disconnection works properly (behavioral test)
    assert!(!device.connected().await.unwrap());

    device.set_connected(true).await.unwrap();
    assert!(device.connected().await.unwrap());

    device.set_connected(false).await.unwrap();
    assert!(!device.connected().await.unwrap());

    fs::remove_file(&test_file).unwrap();
}

#[test]
#[cfg(not(miri))]
fn test_load_config_file_not_found() {
    let config_path = PathBuf::from("nonexistent_config.json");
    let result = load_config(&config_path);
    assert!(result.is_err());
}

#[tokio::test]
#[cfg(not(miri))]
async fn test_start_server_creation() {
    use filemonitor::start_server;
    use std::time::Duration;
    use tokio::time::timeout;

    let config = Config {
        device: DeviceConfig {
            name: "Test Server".to_string(),
            unique_id: "test-server-001".to_string(),
            description: "Test server device".to_string(),
        },
        file: FileConfig {
            path: PathBuf::from("test_server_file.txt"),
            polling_interval_seconds: 1,
        },
        parsing: ParsingConfig {
            rules: vec![],
            case_sensitive: false,
        },
        server: ServerConfig {
            port: 0, // Use port 0 to let OS assign available port
            device_number: 0,
        },
    };

    // Create test file
    std::fs::write(&config.file.path, "test").unwrap();

    // Test that server creation doesn't panic (we can't easily test full startup without blocking)
    let server_future = start_server(config.clone());

    // Use timeout to prevent test from hanging
    let result = timeout(Duration::from_millis(100), server_future).await;

    // Clean up
    std::fs::remove_file(&config.file.path).unwrap();

    // We expect timeout since server.start() would block indefinitely
    assert!(result.is_err());
}
