//! Configuration tests for PPBA Switch driver

use ppba_switch::{Config, DeviceConfig, SerialConfig, ServerConfig};

#[test]
fn default_config_has_expected_values() {
    let config = Config::default();

    assert_eq!(config.device.name, "Pegasus PPBA");
    assert_eq!(config.device.unique_id, "ppba-switch-001");
    assert_eq!(
        config.device.description,
        "Pegasus Astro Pocket Powerbox Advance Gen2"
    );

    assert_eq!(config.serial.port, "/dev/ttyUSB0");
    assert_eq!(config.serial.baud_rate, 9600);
    assert_eq!(config.serial.polling_interval_seconds, 5);
    assert_eq!(config.serial.timeout_seconds, 2);

    assert_eq!(config.server.port, 11112);
    assert_eq!(config.server.device_number, 0);
}

#[test]
fn device_config_default() {
    let config = DeviceConfig::default();

    assert_eq!(config.name, "Pegasus PPBA");
    assert_eq!(config.unique_id, "ppba-switch-001");
    assert!(!config.description.is_empty());
}

#[test]
fn serial_config_default() {
    let config = SerialConfig::default();

    assert_eq!(config.port, "/dev/ttyUSB0");
    assert_eq!(config.baud_rate, 9600);
    assert_eq!(config.polling_interval_seconds, 5);
    assert_eq!(config.timeout_seconds, 2);
}

#[test]
fn server_config_default() {
    let config = ServerConfig::default();

    assert_eq!(config.port, 11112);
    assert_eq!(config.device_number, 0);
}

#[test]
fn config_serializes_to_json() {
    let config = Config::default();
    let json = serde_json::to_string(&config).unwrap();

    assert!(json.contains("Pegasus PPBA"));
    assert!(json.contains("/dev/ttyUSB0"));
    assert!(json.contains("9600"));
    assert!(json.contains("11112"));
}

#[test]
fn config_deserializes_from_json() {
    let json = r#"{
        "device": {
            "name": "Test Device",
            "unique_id": "test-001",
            "description": "Test description"
        },
        "serial": {
            "port": "/dev/ttyACM0",
            "baud_rate": 115200,
            "polling_interval_seconds": 10,
            "timeout_seconds": 5
        },
        "server": {
            "port": 8080,
            "device_number": 1
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();

    assert_eq!(config.device.name, "Test Device");
    assert_eq!(config.device.unique_id, "test-001");
    assert_eq!(config.serial.port, "/dev/ttyACM0");
    assert_eq!(config.serial.baud_rate, 115200);
    assert_eq!(config.serial.polling_interval_seconds, 10);
    assert_eq!(config.server.port, 8080);
    assert_eq!(config.server.device_number, 1);
}

#[test]
fn config_deserializes_with_defaults() {
    // Minimal JSON with only required fields
    let json = r#"{
        "device": {
            "name": "Minimal",
            "unique_id": "min-001",
            "description": "Minimal config"
        },
        "serial": {
            "port": "/dev/ttyUSB1"
        },
        "server": {
            "port": 9000
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();

    assert_eq!(config.device.name, "Minimal");
    assert_eq!(config.serial.port, "/dev/ttyUSB1");
    // These should have defaults
    assert_eq!(config.serial.baud_rate, 9600);
    assert_eq!(config.serial.polling_interval_seconds, 5);
    assert_eq!(config.serial.timeout_seconds, 2);
    assert_eq!(config.server.device_number, 0);
}

#[test]
fn config_clone_works() {
    let config = Config::default();
    let cloned = config.clone();

    assert_eq!(config.device.name, cloned.device.name);
    assert_eq!(config.serial.port, cloned.serial.port);
    assert_eq!(config.server.port, cloned.server.port);
}

#[test]
fn config_debug_works() {
    let config = Config::default();
    let debug_str = format!("{:?}", config);

    assert!(debug_str.contains("Config"));
    assert!(debug_str.contains("DeviceConfig"));
    assert!(debug_str.contains("SerialConfig"));
    assert!(debug_str.contains("ServerConfig"));
}
