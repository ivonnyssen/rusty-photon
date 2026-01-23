//! Integration tests for PHD2 guider client
//!
//! These tests require PHD2 to be installed on the system.
//! Tests that require PHD2 are marked with #[ignore] by default.
//! Run them with: cargo test --test test_integration -- --ignored

use phd2_guider::{
    get_default_phd2_path, load_config, Phd2Client, Phd2Config, Phd2Event, Phd2ProcessManager,
};
use std::path::PathBuf;
use std::time::Duration;

/// Helper to check if PHD2 is available on the system
fn is_phd2_available() -> bool {
    get_default_phd2_path().is_some()
}

/// Helper to create a default test configuration
fn create_test_config() -> Phd2Config {
    Phd2Config {
        host: "localhost".to_string(),
        port: 4400,
        executable_path: get_default_phd2_path(),
        connection_timeout_seconds: 30,
        command_timeout_seconds: 30,
        auto_start: false,
        auto_connect_equipment: false,
        ..Default::default()
    }
}

/// Helper to ensure PHD2 is running for a test
/// Returns (manager, was_started) - was_started indicates if we started PHD2 (so we should stop it)
async fn ensure_phd2_running() -> Option<(Phd2ProcessManager, bool)> {
    if !is_phd2_available() {
        eprintln!("PHD2 not available, skipping test");
        return None;
    }

    let config = create_test_config();
    let manager = Phd2ProcessManager::new(config);

    if manager.is_phd2_running().await {
        // PHD2 already running, don't stop it when test ends
        Some((manager, false))
    } else {
        // Start PHD2
        match manager.start_phd2().await {
            Ok(()) => Some((manager, true)),
            Err(e) => {
                eprintln!("Failed to start PHD2: {}", e);
                None
            }
        }
    }
}

// ============================================================================
// Configuration Tests
// ============================================================================

#[test]
fn test_get_default_phd2_path() {
    // This test just verifies the function doesn't panic
    let path = get_default_phd2_path();
    if let Some(p) = path {
        assert!(p.exists(), "Default PHD2 path should exist if returned");
    }
}

#[test]
#[cfg(not(miri))]
fn test_load_config() {
    let config_path = PathBuf::from("tests/config.json");
    let result = load_config(&config_path);
    assert!(result.is_ok(), "Should load valid config file");

    let config = result.unwrap();
    assert_eq!(config.phd2.host, "localhost");
    assert_eq!(config.phd2.port, 4400);
}

#[test]
#[cfg(not(miri))]
fn test_load_config_with_defaults() {
    let config_path = PathBuf::from("tests/config_minimal.json");
    let result = load_config(&config_path);
    assert!(result.is_ok(), "Should load minimal config with defaults");

    let config = result.unwrap();
    // Defaults should be applied
    assert_eq!(config.phd2.connection_timeout_seconds, 10);
    assert_eq!(config.settling.pixels, 0.5);
}

#[test]
#[cfg(not(miri))]
fn test_load_config_file_not_found() {
    let config_path = PathBuf::from("tests/nonexistent.json");
    let result = load_config(&config_path);
    assert!(result.is_err());
}

// ============================================================================
// Client Creation Tests
// ============================================================================

#[test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning in get_default_phd2_path
fn test_client_creation() {
    let config = create_test_config();
    let client = Phd2Client::new(config);
    // Client should be created successfully
    assert!(std::mem::size_of_val(&client) > 0);
}

#[test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning in get_default_phd2_path
fn test_process_manager_creation() {
    let config = create_test_config();
    let manager = Phd2ProcessManager::new(config);
    // Manager should be created successfully
    assert!(std::mem::size_of_val(&manager) > 0);
}

// ============================================================================
// Connection Tests (require PHD2 to be running)
// ============================================================================

#[tokio::test]
#[ignore] // Run with: cargo test -- --ignored
async fn test_connect_to_running_phd2() {
    let Some((manager, was_started)) = ensure_phd2_running().await else {
        return;
    };

    let config = create_test_config();
    let client = Phd2Client::new(config);

    let result = client.connect().await;
    assert!(result.is_ok(), "Should connect to running PHD2");

    // Wait for version event
    tokio::time::sleep(Duration::from_millis(500)).await;

    let version = client.get_phd2_version().await;
    assert!(version.is_some(), "Should receive PHD2 version");

    let connected = client.is_connected().await;
    assert!(connected, "Should be connected");

    client.disconnect().await.unwrap();
    assert!(!client.is_connected().await, "Should be disconnected");

    // Clean up if we started PHD2
    if was_started {
        manager.stop_phd2(None).await.unwrap();
    }
}

#[tokio::test]
#[ignore]
async fn test_get_app_state() {
    let Some((manager, was_started)) = ensure_phd2_running().await else {
        return;
    };

    let config = create_test_config();
    let client = Phd2Client::new(config);

    client.connect().await.unwrap();

    let state = client.get_app_state().await;
    assert!(state.is_ok(), "Should get app state");

    client.disconnect().await.unwrap();

    if was_started {
        manager.stop_phd2(None).await.unwrap();
    }
}

#[tokio::test]
#[ignore]
async fn test_get_profiles() {
    let Some((manager, was_started)) = ensure_phd2_running().await else {
        return;
    };

    let config = create_test_config();
    let client = Phd2Client::new(config);

    client.connect().await.unwrap();

    let profiles = client.get_profiles().await;
    assert!(profiles.is_ok(), "Should get profiles");

    let profiles = profiles.unwrap();
    assert!(!profiles.is_empty(), "Should have at least one profile");

    client.disconnect().await.unwrap();

    if was_started {
        manager.stop_phd2(None).await.unwrap();
    }
}

#[tokio::test]
#[ignore]
async fn test_get_current_profile() {
    let Some((manager, was_started)) = ensure_phd2_running().await else {
        return;
    };

    let config = create_test_config();
    let client = Phd2Client::new(config);

    client.connect().await.unwrap();

    let profile = client.get_current_profile().await;
    assert!(profile.is_ok(), "Should get current profile");

    let profile = profile.unwrap();
    assert!(!profile.name.is_empty(), "Profile should have a name");

    client.disconnect().await.unwrap();

    if was_started {
        manager.stop_phd2(None).await.unwrap();
    }
}

#[tokio::test]
#[ignore]
async fn test_equipment_connection_status() {
    let Some((manager, was_started)) = ensure_phd2_running().await else {
        return;
    };

    let config = create_test_config();
    let client = Phd2Client::new(config);

    client.connect().await.unwrap();

    let connected = client.is_equipment_connected().await;
    assert!(connected.is_ok(), "Should get equipment connection status");

    client.disconnect().await.unwrap();

    if was_started {
        manager.stop_phd2(None).await.unwrap();
    }
}

#[tokio::test]
#[ignore]
async fn test_event_subscription() {
    let Some((manager, was_started)) = ensure_phd2_running().await else {
        return;
    };

    let config = create_test_config();
    let client = Phd2Client::new(config);

    let mut receiver = client.subscribe();

    client.connect().await.unwrap();

    // We should receive a Version event on connect
    let event = tokio::time::timeout(Duration::from_secs(5), receiver.recv()).await;

    assert!(event.is_ok(), "Should receive event within timeout");
    let event = event.unwrap();
    assert!(event.is_ok(), "Should receive event successfully");

    match event.unwrap() {
        Phd2Event::Version { phd_version, .. } => {
            assert!(!phd_version.is_empty(), "Version should not be empty");
        }
        _ => {
            // Other events might come first depending on PHD2 state
        }
    }

    client.disconnect().await.unwrap();

    if was_started {
        manager.stop_phd2(None).await.unwrap();
    }
}

// ============================================================================
// Process Management Tests (require PHD2 to be installed)
// ============================================================================

#[tokio::test]
#[ignore]
async fn test_is_phd2_running() {
    if !is_phd2_available() {
        eprintln!("PHD2 not available, skipping test");
        return;
    }

    let config = create_test_config();
    let manager = Phd2ProcessManager::new(config);

    // This just tests the detection, doesn't matter if PHD2 is running or not
    let _running = manager.is_phd2_running().await;
}

#[tokio::test]
#[ignore]
async fn test_start_and_stop_phd2() {
    if !is_phd2_available() {
        eprintln!("PHD2 not available, skipping test");
        return;
    }

    let config = create_test_config();
    let manager = Phd2ProcessManager::new(config.clone());

    // Skip if PHD2 is already running
    if manager.is_phd2_running().await {
        eprintln!("PHD2 already running, skipping start test");
        return;
    }

    // Start PHD2
    let result = manager.start_phd2().await;
    assert!(result.is_ok(), "Should start PHD2: {:?}", result.err());

    // Verify it's running
    assert!(
        manager.is_phd2_running().await,
        "PHD2 should be running after start"
    );

    // Connect and verify
    let client = Phd2Client::new(config);
    let connect_result = client.connect().await;
    assert!(connect_result.is_ok(), "Should connect to started PHD2");

    // Wait for version
    tokio::time::sleep(Duration::from_millis(500)).await;
    let version = client.get_phd2_version().await;
    assert!(version.is_some(), "Should have PHD2 version");

    // Stop PHD2
    let stop_result = manager.stop_phd2(Some(&client)).await;
    assert!(stop_result.is_ok(), "Should stop PHD2");

    // Give it time to fully stop
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Verify it's stopped
    assert!(
        !manager.is_phd2_running().await,
        "PHD2 should not be running after stop"
    );
}

#[tokio::test]
#[ignore]
async fn test_start_phd2_already_running() {
    if !is_phd2_available() {
        eprintln!("PHD2 not available, skipping test");
        return;
    }

    let config = create_test_config();
    let manager = Phd2ProcessManager::new(config);

    // Start PHD2 first time
    if !manager.is_phd2_running().await {
        manager.start_phd2().await.unwrap();
    }

    // Try to start again - should succeed (returns Ok if already running)
    let result = manager.start_phd2().await;
    assert!(result.is_ok(), "Should succeed when PHD2 already running");

    // Clean up
    manager.stop_phd2(None).await.unwrap();
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support tokio networking
async fn test_connect_to_nonexistent_server() {
    let config = Phd2Config {
        host: "localhost".to_string(),
        port: 59999, // Unlikely to be in use
        connection_timeout_seconds: 2,
        ..Default::default()
    };

    let client = Phd2Client::new(config);
    let result = client.connect().await;

    assert!(
        result.is_err(),
        "Should fail to connect to nonexistent server"
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support tokio networking
async fn test_send_request_when_not_connected() {
    let config = create_test_config();
    let client = Phd2Client::new(config);

    // Don't connect, try to get state
    let result = client.get_app_state().await;
    assert!(result.is_err(), "Should fail when not connected");
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_process_manager_executable_not_found() {
    let config = Phd2Config {
        executable_path: Some(PathBuf::from("/nonexistent/path/to/phd2")),
        ..Default::default()
    };

    let manager = Phd2ProcessManager::new(config);
    let result = manager.start_phd2().await;

    assert!(result.is_err(), "Should fail with nonexistent executable");
}

// ============================================================================
// Full Integration Workflow Test
// ============================================================================

#[tokio::test]
#[ignore]
async fn test_full_workflow() {
    if !is_phd2_available() {
        eprintln!("PHD2 not available, skipping test");
        return;
    }

    let config = create_test_config();
    let manager = Phd2ProcessManager::new(config.clone());

    // Start PHD2 if not running
    let was_already_running = manager.is_phd2_running().await;
    if !was_already_running {
        manager.start_phd2().await.unwrap();
    }

    // Create client and connect
    let client = Phd2Client::new(config);
    client.connect().await.unwrap();

    // Wait for version event
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify connection
    assert!(client.is_connected().await);
    assert!(client.get_phd2_version().await.is_some());

    // Get state
    let state = client.get_app_state().await.unwrap();
    println!("PHD2 state: {}", state);

    // Get profiles
    let profiles = client.get_profiles().await.unwrap();
    println!("Available profiles:");
    for profile in &profiles {
        println!("  [{}] {}", profile.id, profile.name);
    }

    // Get current profile
    let current = client.get_current_profile().await.unwrap();
    println!("Current profile: {} (id: {})", current.name, current.id);

    // Check equipment status
    let equipment_connected = client.is_equipment_connected().await.unwrap();
    println!("Equipment connected: {}", equipment_connected);

    // Disconnect client
    client.disconnect().await.unwrap();
    assert!(!client.is_connected().await);

    // Stop PHD2 only if we started it
    if !was_already_running {
        manager.stop_phd2(None).await.unwrap();
    }
}
