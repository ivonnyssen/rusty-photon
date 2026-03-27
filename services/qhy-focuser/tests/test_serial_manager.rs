//! Unit tests for SerialManager internal behaviors
//!
//! These tests cover ref-counting, cached state, and direct SerialManager
//! methods that are not observable through the ASCOM Alpaca HTTP API.
//!
//! Requires the `mock` feature (uses src/mock.rs MockSerialPortFactory).
#![cfg(feature = "mock")]

use std::sync::Arc;

use qhy_focuser::config::{FocuserConfig, SerialConfig, ServerConfig};
use qhy_focuser::protocol::Command;
use qhy_focuser::{Config, MockSerialPortFactory, SerialManager, SerialPortFactory};

fn test_config() -> Config {
    Config {
        serial: SerialConfig {
            port: "/dev/mock".to_string(),
            polling_interval_ms: 60_000,
            ..Default::default()
        },
        server: ServerConfig {
            port: 0,
            discovery_port: None,
            tls: None,
        },
        focuser: FocuserConfig::default(),
    }
}

fn create_manager() -> Arc<SerialManager> {
    let factory: Arc<dyn SerialPortFactory> = Arc::new(MockSerialPortFactory::default());
    Arc::new(SerialManager::new(test_config(), factory))
}

// ============================================================================
// Ref-counting
// ============================================================================

#[tokio::test]
async fn test_connect_makes_available() {
    let manager = create_manager();
    assert!(!manager.is_available());

    manager.connect().await.unwrap();
    assert!(manager.is_available());

    manager.disconnect().await;
}

#[tokio::test]
async fn test_double_connect_keeps_available_after_single_disconnect() {
    let manager = create_manager();

    manager.connect().await.unwrap();
    manager.connect().await.unwrap();
    manager.disconnect().await;
    assert!(
        manager.is_available(),
        "should still be available with one ref remaining"
    );

    manager.disconnect().await;
    assert!(
        !manager.is_available(),
        "should not be available after all refs released"
    );
}

#[tokio::test]
async fn test_disconnect_at_zero_ref_count_is_noop() {
    let manager = create_manager();
    manager.disconnect().await; // should not panic
    assert!(!manager.is_available());
}

// ============================================================================
// Cached state
// ============================================================================

#[tokio::test]
async fn test_cached_state_empty_before_connection() {
    let manager = create_manager();
    let state = manager.get_cached_state().await;
    assert_eq!(state.position, None);
    assert_eq!(state.outer_temp, None);
    assert_eq!(state.chip_temp, None);
    assert_eq!(state.voltage, None);
    assert_eq!(state.firmware_version, None);
    assert_eq!(state.board_version, None);
    assert!(!state.is_moving);
}

#[tokio::test]
async fn test_cached_state_populated_after_handshake() {
    let manager = create_manager();
    manager.connect().await.unwrap();

    let state = manager.get_cached_state().await;
    assert_eq!(state.firmware_version, Some("2.1.0".to_string()));
    assert_eq!(state.board_version, Some("1.0".to_string()));
    assert_eq!(state.position, Some(0));
    assert!(
        (state.outer_temp.unwrap() - 25.0).abs() < 0.001,
        "expected ~25.0, got {:?}",
        state.outer_temp
    );
    assert!(
        (state.chip_temp.unwrap() - 30.0).abs() < 0.001,
        "expected ~30.0, got {:?}",
        state.chip_temp
    );
    assert!(
        (state.voltage.unwrap() - 12.5).abs() < 0.001,
        "expected ~12.5, got {:?}",
        state.voltage
    );
    assert!(!state.is_moving);

    manager.disconnect().await;
}

// ============================================================================
// is_available and Debug
// ============================================================================

#[tokio::test]
async fn test_is_available_reflects_connection_state() {
    let manager = create_manager();
    assert!(!manager.is_available());

    manager.connect().await.unwrap();
    assert!(manager.is_available());

    manager.disconnect().await;
    assert!(!manager.is_available());
}

#[tokio::test]
async fn test_debug_representation_contains_serial_manager() {
    let manager = create_manager();
    let debug_str = format!("{:?}", manager);
    assert!(debug_str.contains("SerialManager"), "got: {}", debug_str);
}

// ============================================================================
// set_speed / set_reverse
// ============================================================================

#[tokio::test]
async fn test_set_speed_succeeds_when_connected() {
    let manager = create_manager();
    manager.connect().await.unwrap();
    manager.set_speed(5).await.unwrap();
    manager.disconnect().await;
}

#[tokio::test]
async fn test_set_speed_fails_when_not_connected() {
    let manager = create_manager();
    let err = manager.set_speed(5).await.unwrap_err();
    assert!(
        format!("{:?}", err).contains("NotConnected"),
        "got: {:?}",
        err
    );
}

#[tokio::test]
async fn test_set_reverse_succeeds_when_connected() {
    let manager = create_manager();
    manager.connect().await.unwrap();
    manager.set_reverse(true).await.unwrap();
    manager.disconnect().await;
}

#[tokio::test]
async fn test_set_reverse_fails_when_not_connected() {
    let manager = create_manager();
    let err = manager.set_reverse(true).await.unwrap_err();
    assert!(
        format!("{:?}", err).contains("NotConnected"),
        "got: {:?}",
        err
    );
}

// ============================================================================
// send_command / refresh_position when not connected
// ============================================================================

#[tokio::test]
async fn test_send_command_fails_when_not_connected() {
    let manager = create_manager();
    let err = manager
        .send_command(Command::GetPosition)
        .await
        .unwrap_err();
    assert!(
        format!("{:?}", err).contains("NotConnected"),
        "got: {:?}",
        err
    );
}

#[tokio::test]
async fn test_refresh_position_fails_when_not_connected() {
    let manager = create_manager();
    let err = manager.refresh_position().await.unwrap_err();
    assert!(
        format!("{:?}", err).contains("NotConnected"),
        "got: {:?}",
        err
    );
}

// ============================================================================
// Move completion detection
// ============================================================================

#[tokio::test]
async fn test_move_sets_target_and_is_moving() {
    let manager = create_manager();
    manager.connect().await.unwrap();

    manager.move_absolute(5000).await.unwrap();
    let state = manager.get_cached_state().await;
    assert_eq!(state.target_position, Some(5000));
    assert!(state.is_moving);

    manager.disconnect().await;
}

#[tokio::test]
async fn test_refresh_position_detects_move_completion() {
    let manager = create_manager();
    manager.connect().await.unwrap();

    // Mock moves 1000 steps at a time; target within 1000 resolves immediately
    manager.move_absolute(1000).await.unwrap();
    manager.refresh_position().await.unwrap();

    let state = manager.get_cached_state().await;
    assert_eq!(state.position, Some(1000));
    assert!(!state.is_moving, "should detect move completion");
    assert_eq!(state.target_position, None);

    manager.disconnect().await;
}
