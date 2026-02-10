//! Configuration tests for QHY Q-Focuser driver

use qhy_focuser::{Config, FocuserConfig, SerialConfig, ServerConfig};

#[test]
fn default_config_has_expected_values() {
    let config = Config::default();

    assert_eq!(config.focuser.name, "QHY Q-Focuser");
    assert!(config.focuser.enabled);
    assert_eq!(config.focuser.max_step, 64_000);
    assert_eq!(config.focuser.speed, 0);
    assert!(!config.focuser.reverse);

    assert_eq!(config.serial.port, "/dev/ttyACM0");
    assert_eq!(config.serial.baud_rate, 9600);
    assert_eq!(config.serial.polling_interval_ms, 1000);
    assert_eq!(config.serial.timeout_seconds, 2);

    assert_eq!(config.server.port, 11113);
}

#[test]
fn focuser_config_default() {
    let config = FocuserConfig::default();

    assert_eq!(config.name, "QHY Q-Focuser");
    assert_eq!(config.unique_id, "qhy-focuser-001");
    assert!(!config.description.is_empty());
    assert_eq!(config.device_number, 0);
    assert!(config.enabled);
    assert_eq!(config.max_step, 64_000);
    assert_eq!(config.speed, 0);
    assert!(!config.reverse);
}

#[test]
fn serial_config_default() {
    let config = SerialConfig::default();

    assert_eq!(config.port, "/dev/ttyACM0");
    assert_eq!(config.baud_rate, 9600);
    assert_eq!(config.polling_interval_ms, 1000);
    assert_eq!(config.timeout_seconds, 2);
}

#[test]
fn server_config_default() {
    let config = ServerConfig::default();

    assert_eq!(config.port, 11113);
}

#[test]
fn config_serializes_to_json() {
    let config = Config::default();
    let json = serde_json::to_string(&config).unwrap();

    assert!(json.contains("QHY Q-Focuser"));
    assert!(json.contains("/dev/ttyACM0"));
    assert!(json.contains("9600"));
    assert!(json.contains("11113"));
}

#[test]
fn config_deserializes_from_json() {
    let json = r#"{
        "serial": {
            "port": "/dev/ttyACM0",
            "baud_rate": 115200,
            "polling_interval_ms": 2000,
            "timeout_seconds": 5
        },
        "server": {
            "port": 8080
        },
        "focuser": {
            "name": "Test Focuser",
            "unique_id": "test-focuser-001",
            "description": "Test focuser description",
            "device_number": 1,
            "enabled": true,
            "max_step": 100000,
            "speed": 3,
            "reverse": true
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();

    assert_eq!(config.focuser.name, "Test Focuser");
    assert_eq!(config.focuser.unique_id, "test-focuser-001");
    assert_eq!(config.focuser.device_number, 1);
    assert!(config.focuser.enabled);
    assert_eq!(config.focuser.max_step, 100000);
    assert_eq!(config.focuser.speed, 3);
    assert!(config.focuser.reverse);

    assert_eq!(config.serial.port, "/dev/ttyACM0");
    assert_eq!(config.serial.baud_rate, 115200);
    assert_eq!(config.serial.polling_interval_ms, 2000);
    assert_eq!(config.server.port, 8080);
}

#[test]
fn config_deserializes_with_defaults() {
    let json = r#"{
        "serial": {
            "port": "/dev/ttyUSB1"
        },
        "server": {
            "port": 9000
        },
        "focuser": {
            "name": "Minimal Focuser",
            "unique_id": "min-focuser-001",
            "description": "Minimal config"
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();

    assert_eq!(config.focuser.name, "Minimal Focuser");
    assert_eq!(config.serial.port, "/dev/ttyUSB1");
    assert_eq!(config.serial.baud_rate, 9600);
    assert_eq!(config.serial.polling_interval_ms, 1000);
    assert_eq!(config.serial.timeout_seconds, 2);
    assert_eq!(config.focuser.device_number, 0);
    assert!(config.focuser.enabled);
    assert_eq!(config.focuser.max_step, 64_000);
    assert_eq!(config.focuser.speed, 0);
    assert!(!config.focuser.reverse);
}

#[test]
fn config_clone_works() {
    let config = Config::default();
    let cloned = config.clone();

    assert_eq!(config.focuser.name, cloned.focuser.name);
    assert_eq!(config.serial.port, cloned.serial.port);
    assert_eq!(config.server.port, cloned.server.port);
}

#[test]
fn config_debug_works() {
    let config = Config::default();
    let debug_str = format!("{:?}", config);

    assert!(debug_str.contains("Config"));
    assert!(debug_str.contains("FocuserConfig"));
    assert!(debug_str.contains("SerialConfig"));
    assert!(debug_str.contains("ServerConfig"));
}
