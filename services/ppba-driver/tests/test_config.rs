//! Configuration tests for PPBA Driver

use ppba_driver::{Config, ObservingConditionsConfig, SerialConfig, ServerConfig, SwitchConfig};

#[test]
fn default_config_has_expected_values() {
    let config = Config::default();

    assert_eq!(config.switch.name, "Pegasus PPBA Switch");
    assert!(config.switch.enabled);

    assert_eq!(config.observingconditions.name, "Pegasus PPBA Weather");
    assert!(config.observingconditions.enabled);

    assert_eq!(config.serial.port, "/dev/ttyUSB0");
    assert_eq!(config.serial.baud_rate, 9600);
    assert_eq!(config.serial.polling_interval_ms, 5000);
    assert_eq!(config.serial.timeout_seconds, 2);

    assert_eq!(config.server.port, 11112);
}

#[test]
fn switch_config_default() {
    let config = SwitchConfig::default();

    assert_eq!(config.name, "Pegasus PPBA Switch");
    assert_eq!(config.unique_id, "ppba-switch-001");
    assert!(!config.description.is_empty());
    assert_eq!(config.device_number, 0);
    assert!(config.enabled);
}

#[test]
fn observingconditions_config_default() {
    let config = ObservingConditionsConfig::default();

    assert_eq!(config.name, "Pegasus PPBA Weather");
    assert_eq!(config.unique_id, "ppba-observingconditions-001");
    assert!(!config.description.is_empty());
    assert_eq!(config.device_number, 0);
    assert!(config.enabled);
    assert_eq!(config.averaging_period_ms, 300_000); // 5 minutes
}

#[test]
fn serial_config_default() {
    let config = SerialConfig::default();

    assert_eq!(config.port, "/dev/ttyUSB0");
    assert_eq!(config.baud_rate, 9600);
    assert_eq!(config.polling_interval_ms, 5000);
    assert_eq!(config.timeout_seconds, 2);
}

#[test]
fn server_config_default() {
    let config = ServerConfig::default();

    assert_eq!(config.port, 11112);
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
        "serial": {
            "port": "/dev/ttyACM0",
            "baud_rate": 115200,
            "polling_interval_ms": 10000,
            "timeout_seconds": 5
        },
        "server": {
            "port": 8080
        },
        "switch": {
            "name": "Test Switch",
            "unique_id": "test-switch-001",
            "description": "Test switch description",
            "device_number": 1,
            "enabled": true
        },
        "observingconditions": {
            "name": "Test Weather",
            "unique_id": "test-weather-001",
            "description": "Test weather description",
            "device_number": 2,
            "enabled": false,
            "averaging_period_ms": 120000
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();

    assert_eq!(config.switch.name, "Test Switch");
    assert_eq!(config.switch.unique_id, "test-switch-001");
    assert_eq!(config.switch.device_number, 1);
    assert!(config.switch.enabled);

    assert_eq!(config.observingconditions.name, "Test Weather");
    assert_eq!(config.observingconditions.device_number, 2);
    assert!(!config.observingconditions.enabled);
    assert_eq!(config.observingconditions.averaging_period_ms, 120000);

    assert_eq!(config.serial.port, "/dev/ttyACM0");
    assert_eq!(config.serial.baud_rate, 115200);
    assert_eq!(config.serial.polling_interval_ms, 10000);
    assert_eq!(config.server.port, 8080);
}

#[test]
fn config_deserializes_with_defaults() {
    // Minimal JSON with only required fields
    let json = r#"{
        "serial": {
            "port": "/dev/ttyUSB1"
        },
        "server": {
            "port": 9000
        },
        "switch": {
            "name": "Minimal Switch",
            "unique_id": "min-switch-001",
            "description": "Minimal config"
        },
        "observingconditions": {
            "name": "Minimal Weather",
            "unique_id": "min-weather-001",
            "description": "Minimal weather config"
        }
    }"#;

    let config: Config = serde_json::from_str(json).unwrap();

    assert_eq!(config.switch.name, "Minimal Switch");
    assert_eq!(config.serial.port, "/dev/ttyUSB1");
    // These should have defaults
    assert_eq!(config.serial.baud_rate, 9600);
    assert_eq!(config.serial.polling_interval_ms, 5000);
    assert_eq!(config.serial.timeout_seconds, 2);
    assert_eq!(config.switch.device_number, 0);
    assert!(config.switch.enabled);
    assert_eq!(config.observingconditions.device_number, 0);
    assert!(config.observingconditions.enabled);
    assert_eq!(config.observingconditions.averaging_period_ms, 300_000);
}

#[test]
fn config_clone_works() {
    let config = Config::default();
    let cloned = config.clone();

    assert_eq!(config.switch.name, cloned.switch.name);
    assert_eq!(config.serial.port, cloned.serial.port);
    assert_eq!(config.server.port, cloned.server.port);
}

#[test]
fn config_debug_works() {
    let config = Config::default();
    let debug_str = format!("{:?}", config);

    assert!(debug_str.contains("Config"));
    assert!(debug_str.contains("SwitchConfig"));
    assert!(debug_str.contains("ObservingConditionsConfig"));
    assert!(debug_str.contains("SerialConfig"));
    assert!(debug_str.contains("ServerConfig"));
}
