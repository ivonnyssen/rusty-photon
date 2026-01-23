//! Unit tests for PHD2 configuration

use phd2_guider::{Phd2Config, ReconnectConfig, SettleParams};

#[test]
fn test_settle_params_default() {
    let params = SettleParams::default();
    assert_eq!(params.pixels, 0.5);
    assert_eq!(params.time, 10);
    assert_eq!(params.timeout, 60);
}

#[test]
fn test_phd2_config_default() {
    let config = Phd2Config::default();
    assert_eq!(config.host, "localhost");
    assert_eq!(config.port, 4400);
    assert_eq!(config.connection_timeout_seconds, 10);
    assert_eq!(config.command_timeout_seconds, 30);
    assert!(!config.auto_start);
    assert!(!config.auto_connect_equipment);
    assert!(config.reconnect.enabled);
    assert_eq!(config.reconnect.interval_seconds, 5);
    assert!(config.reconnect.max_retries.is_none());
}

#[test]
fn test_reconnect_config_default() {
    let config = ReconnectConfig::default();
    assert!(config.enabled);
    assert_eq!(config.interval_seconds, 5);
    assert!(config.max_retries.is_none());
}

#[test]
fn test_reconnect_config_serialization() {
    let config = ReconnectConfig {
        enabled: true,
        interval_seconds: 10,
        max_retries: Some(5),
    };
    let json = serde_json::to_value(&config).unwrap();
    assert_eq!(json["enabled"], true);
    assert_eq!(json["interval_seconds"], 10);
    assert_eq!(json["max_retries"], 5);
}

#[test]
fn test_settle_params_serialization() {
    let params = SettleParams {
        pixels: 1.5,
        time: 15,
        timeout: 120,
    };
    let json = serde_json::to_value(&params).unwrap();
    assert_eq!(json["pixels"], 1.5);
    assert_eq!(json["time"], 15);
    assert_eq!(json["timeout"], 120);
}
