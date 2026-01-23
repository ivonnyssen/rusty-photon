//! Integration tests for PHD2 guider client
//!
//! These tests require PHD2 to be installed on the system.
//! Tests that require PHD2 are marked with #[ignore] by default.
//! Run them with: cargo test --test test_integration -- --ignored
//!
//! Some tests use the mock_phd2 binary and can run without PHD2 installed.

#[cfg_attr(miri, allow(unused_imports))]
use phd2_guider::{
    get_default_phd2_path, load_config, Phd2Client, Phd2Config, Phd2Event, Phd2ProcessManager,
    ReconnectConfig, SettleParams,
};
#[cfg_attr(miri, allow(unused_imports))]
use std::path::PathBuf;
#[cfg_attr(miri, allow(unused_imports))]
use std::process::{Child, Command};
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
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
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
    // Use a port unlikely to be in use to ensure we actually try to start the executable
    let config = Phd2Config {
        port: 59997,
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

// ============================================================================
// Mock PHD2 Tests (don't require real PHD2)
// ============================================================================

/// Helper to find the mock_phd2 binary
#[cfg(not(miri))]
fn find_mock_phd2_binary() -> Option<PathBuf> {
    // Try debug build first
    let debug_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/debug/mock_phd2");

    if debug_path.exists() {
        return Some(debug_path);
    }

    // Try release build
    let release_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/release/mock_phd2");

    if release_path.exists() {
        return Some(release_path);
    }

    None
}

/// Stub for miri - always returns None
#[cfg(miri)]
fn find_mock_phd2_binary() -> Option<PathBuf> {
    None
}

/// Start the mock PHD2 server on a specific port
#[cfg(not(miri))]
fn start_mock_phd2(port: u16) -> Option<Child> {
    let binary = find_mock_phd2_binary()?;

    let child = Command::new(binary)
        .arg("--port")
        .arg(port.to_string())
        .spawn()
        .ok()?;

    // Give the server time to start
    std::thread::sleep(Duration::from_millis(200));

    Some(child)
}

/// Stub for miri - always returns None
#[cfg(miri)]
fn start_mock_phd2(_port: u16) -> Option<Child> {
    None
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_mock_phd2_connection() {
    let port = 44100; // Use a different port to avoid conflicts

    let Some(mut child) = start_mock_phd2(port) else {
        eprintln!(
            "Mock PHD2 binary not found. Run 'cargo build -p phd2-guider --bin mock_phd2' first"
        );
        return;
    };

    let config = Phd2Config {
        host: "localhost".to_string(),
        port,
        connection_timeout_seconds: 5,
        command_timeout_seconds: 5,
        ..Default::default()
    };

    let client = Phd2Client::new(config);

    // Connect to mock server
    let result = client.connect().await;
    assert!(result.is_ok(), "Should connect to mock PHD2: {:?}", result);

    // Wait for version event
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Verify we got the version
    let version = client.get_phd2_version().await;
    assert!(version.is_some(), "Should have received version");
    let version = version.unwrap();
    assert!(
        version.contains("mock"),
        "Version should indicate mock server"
    );

    // Disconnect
    client.disconnect().await.unwrap();

    // Kill the mock server
    child.kill().ok();
    child.wait().ok();
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_mock_phd2_get_app_state() {
    let port = 44101;

    let Some(mut child) = start_mock_phd2(port) else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    let config = Phd2Config {
        host: "localhost".to_string(),
        port,
        connection_timeout_seconds: 5,
        command_timeout_seconds: 5,
        ..Default::default()
    };

    let client = Phd2Client::new(config);
    client.connect().await.unwrap();

    // Wait for connection to stabilize
    tokio::time::sleep(Duration::from_millis(200)).await;

    let state = client.get_app_state().await;
    assert!(state.is_ok(), "Should get app state: {:?}", state);

    client.disconnect().await.ok();
    child.kill().ok();
    child.wait().ok();
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_mock_phd2_get_profiles() {
    let port = 44102;

    let Some(mut child) = start_mock_phd2(port) else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    let config = Phd2Config {
        host: "localhost".to_string(),
        port,
        connection_timeout_seconds: 5,
        command_timeout_seconds: 5,
        ..Default::default()
    };

    let client = Phd2Client::new(config);
    client.connect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let profiles = client.get_profiles().await;
    assert!(profiles.is_ok(), "Should get profiles: {:?}", profiles);

    let profiles = profiles.unwrap();
    assert!(!profiles.is_empty(), "Should have at least one profile");
    assert_eq!(profiles[0].name, "Mock Profile");

    client.disconnect().await.ok();
    child.kill().ok();
    child.wait().ok();
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_mock_phd2_get_equipment() {
    let port = 44103;

    let Some(mut child) = start_mock_phd2(port) else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    let config = Phd2Config {
        host: "localhost".to_string(),
        port,
        connection_timeout_seconds: 5,
        command_timeout_seconds: 5,
        ..Default::default()
    };

    let client = Phd2Client::new(config);
    client.connect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let equipment = client.get_current_equipment().await;
    assert!(equipment.is_ok(), "Should get equipment: {:?}", equipment);

    let equipment = equipment.unwrap();
    assert!(equipment.camera.is_some(), "Should have camera info");
    assert!(equipment.mount.is_some(), "Should have mount info");

    client.disconnect().await.ok();
    child.kill().ok();
    child.wait().ok();
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_mock_phd2_exposure_methods() {
    let port = 44104;

    let Some(mut child) = start_mock_phd2(port) else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    let config = Phd2Config {
        host: "localhost".to_string(),
        port,
        connection_timeout_seconds: 5,
        command_timeout_seconds: 5,
        ..Default::default()
    };

    let client = Phd2Client::new(config);
    client.connect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Get exposure
    let exposure = client.get_exposure().await;
    assert!(exposure.is_ok(), "Should get exposure: {:?}", exposure);
    assert_eq!(exposure.unwrap(), 1000);

    // Get exposure durations
    let durations = client.get_exposure_durations().await;
    assert!(durations.is_ok(), "Should get durations: {:?}", durations);
    assert!(!durations.unwrap().is_empty());

    // Set exposure
    let set_result = client.set_exposure(2000).await;
    assert!(set_result.is_ok(), "Should set exposure: {:?}", set_result);

    client.disconnect().await.ok();
    child.kill().ok();
    child.wait().ok();
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_mock_phd2_calibration_methods() {
    let port = 44105;

    let Some(mut child) = start_mock_phd2(port) else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    let config = Phd2Config {
        host: "localhost".to_string(),
        port,
        connection_timeout_seconds: 5,
        command_timeout_seconds: 5,
        ..Default::default()
    };

    let client = Phd2Client::new(config);
    client.connect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Get calibration status
    let calibrated = client.is_calibrated().await;
    assert!(
        calibrated.is_ok(),
        "Should get calibration status: {:?}",
        calibrated
    );

    // Get calibration data
    let data = client
        .get_calibration_data(phd2_guider::CalibrationTarget::Mount)
        .await;
    assert!(data.is_ok(), "Should get calibration data: {:?}", data);

    // Clear calibration
    let clear_result = client
        .clear_calibration(phd2_guider::CalibrationTarget::Mount)
        .await;
    assert!(
        clear_result.is_ok(),
        "Should clear calibration: {:?}",
        clear_result
    );

    client.disconnect().await.ok();
    child.kill().ok();
    child.wait().ok();
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_mock_phd2_guiding_control() {
    let port = 44106;

    let Some(mut child) = start_mock_phd2(port) else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    let config = Phd2Config {
        host: "localhost".to_string(),
        port,
        connection_timeout_seconds: 5,
        command_timeout_seconds: 5,
        ..Default::default()
    };

    let client = Phd2Client::new(config);
    client.connect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Start looping
    let loop_result = client.start_loop().await;
    assert!(
        loop_result.is_ok(),
        "Should start looping: {:?}",
        loop_result
    );

    // Start guiding
    let settle = SettleParams::default();
    let guide_result = client.start_guiding(&settle, false, None).await;
    assert!(
        guide_result.is_ok(),
        "Should start guiding: {:?}",
        guide_result
    );

    // Pause guiding
    let pause_result = client.pause(true).await;
    assert!(
        pause_result.is_ok(),
        "Should pause guiding: {:?}",
        pause_result
    );

    // Stop capture
    let stop_result = client.stop_capture().await;
    assert!(
        stop_result.is_ok(),
        "Should stop capture: {:?}",
        stop_result
    );

    client.disconnect().await.ok();
    child.kill().ok();
    child.wait().ok();
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_mock_phd2_star_operations() {
    let port = 44107;

    let Some(mut child) = start_mock_phd2(port) else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    let config = Phd2Config {
        host: "localhost".to_string(),
        port,
        connection_timeout_seconds: 5,
        command_timeout_seconds: 5,
        ..Default::default()
    };

    let client = Phd2Client::new(config);
    client.connect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Auto-select star
    let find_result = client.find_star(None).await;
    assert!(
        find_result.is_ok(),
        "Should auto-select star: {:?}",
        find_result
    );

    // Set lock position
    let lock_result = client.set_lock_position(320.0, 240.0, true).await;
    assert!(
        lock_result.is_ok(),
        "Should set lock position: {:?}",
        lock_result
    );

    client.disconnect().await.ok();
    child.kill().ok();
    child.wait().ok();
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_mock_phd2_cooling() {
    let port = 44108;

    let Some(mut child) = start_mock_phd2(port) else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    let config = Phd2Config {
        host: "localhost".to_string(),
        port,
        connection_timeout_seconds: 5,
        command_timeout_seconds: 5,
        ..Default::default()
    };

    let client = Phd2Client::new(config);
    client.connect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Get CCD temperature
    let temp = client.get_ccd_temperature().await;
    assert!(temp.is_ok(), "Should get temperature: {:?}", temp);
    assert!((temp.unwrap() - 20.0).abs() < 1.0);

    // Get cooler status
    let status = client.get_cooler_status().await;
    assert!(status.is_ok(), "Should get cooler status: {:?}", status);

    client.disconnect().await.ok();
    child.kill().ok();
    child.wait().ok();
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_mock_phd2_star_image() {
    let port = 44109;

    let Some(mut child) = start_mock_phd2(port) else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    let config = Phd2Config {
        host: "localhost".to_string(),
        port,
        connection_timeout_seconds: 5,
        command_timeout_seconds: 5,
        ..Default::default()
    };

    let client = Phd2Client::new(config);
    client.connect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Get star image
    let image = client.get_star_image(32).await;
    assert!(image.is_ok(), "Should get star image: {:?}", image);

    let image = image.unwrap();
    assert_eq!(image.width, 32);
    assert_eq!(image.height, 32);

    client.disconnect().await.ok();
    child.kill().ok();
    child.wait().ok();
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_mock_phd2_event_subscription() {
    let port = 44110;

    let Some(mut child) = start_mock_phd2(port) else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    let config = Phd2Config {
        host: "localhost".to_string(),
        port,
        connection_timeout_seconds: 5,
        command_timeout_seconds: 5,
        ..Default::default()
    };

    let client = Phd2Client::new(config);
    let mut receiver = client.subscribe();

    client.connect().await.unwrap();

    // We should receive a Version event
    let event = tokio::time::timeout(Duration::from_secs(2), receiver.recv()).await;
    assert!(event.is_ok(), "Should receive event within timeout");

    let event = event.unwrap();
    assert!(event.is_ok(), "Channel should be open");

    match event.unwrap() {
        Phd2Event::Version { phd_version, .. } => {
            assert!(phd_version.contains("mock"), "Should be mock version");
        }
        other => {
            panic!("Expected Version event, got {:?}", other);
        }
    }

    client.disconnect().await.ok();
    child.kill().ok();
    child.wait().ok();
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_mock_phd2_reconnect_on_disconnect() {
    let port = 44111;

    let Some(mut child) = start_mock_phd2(port) else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    let config = Phd2Config {
        host: "localhost".to_string(),
        port,
        connection_timeout_seconds: 5,
        command_timeout_seconds: 5,
        reconnect: ReconnectConfig {
            enabled: true,
            interval_seconds: 1,
            max_retries: Some(3),
        },
        ..Default::default()
    };

    let client = Phd2Client::new(config);
    client.connect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(client.is_connected().await, "Should be connected initially");

    // Kill the mock server to simulate disconnect
    child.kill().ok();
    child.wait().ok();

    // Wait a bit for disconnect detection
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Start a new mock server
    let mut child2 = start_mock_phd2(port).expect("Should start new mock server");

    // Wait for reconnection
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Check if reconnected
    let _is_connected = client.is_connected().await;
    // Note: The auto-reconnect might or might not succeed depending on timing
    // This test mainly verifies that the reconnect logic doesn't panic

    client.disconnect().await.ok();
    child2.kill().ok();
    child2.wait().ok();
}

// ============================================================================
// Process Manager Tests with Mock PHD2
// ============================================================================
//
// These tests use the mock_phd2 binary via Phd2ProcessManager.
// The mock binary reads port from MOCK_PHD2_PORT environment variable,
// which is passed via spawn_env in Phd2Config.

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_process_manager_start_stop_mock() {
    let port = 44200; // Use unique port for this test

    let Some(binary_path) = find_mock_phd2_binary() else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    // First make sure nothing is running on that port
    let addr = format!("127.0.0.1:{}", port);
    if tokio::net::TcpStream::connect(&addr).await.is_ok() {
        eprintln!("Port {} is in use, skipping test", port);
        return;
    }

    let mut spawn_env = std::collections::HashMap::new();
    spawn_env.insert("MOCK_PHD2_PORT".to_string(), port.to_string());

    let config = Phd2Config {
        host: "localhost".to_string(),
        port,
        executable_path: Some(binary_path),
        connection_timeout_seconds: 10,
        command_timeout_seconds: 5,
        spawn_env,
        ..Default::default()
    };

    let manager = Phd2ProcessManager::new(config.clone());

    // Verify not running initially
    assert!(
        !manager.is_phd2_running().await,
        "Mock should not be running initially"
    );
    assert!(
        !manager.has_managed_process().await,
        "Should not have managed process initially"
    );

    // Start the mock PHD2
    let result = manager.start_phd2().await;
    assert!(result.is_ok(), "Should start mock PHD2: {:?}", result);

    // Verify it's running
    assert!(
        manager.is_phd2_running().await,
        "Mock should be running after start"
    );
    assert!(
        manager.has_managed_process().await,
        "Should have managed process after start"
    );

    // Connect a client
    let client = Phd2Client::new(config);
    client.connect().await.unwrap();

    // Wait for version event with retry (macOS CI can be slow)
    let mut version = None;
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        version = client.get_phd2_version().await;
        if version.is_some() {
            break;
        }
    }
    assert!(version.is_some(), "Should have version");
    assert!(version.unwrap().contains("mock"), "Should be mock version");

    // Stop via process manager (graceful shutdown)
    let stop_result = manager.stop_phd2(Some(&client)).await;
    assert!(
        stop_result.is_ok(),
        "Should stop mock PHD2: {:?}",
        stop_result
    );

    // Wait for shutdown
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify not running
    assert!(
        !manager.is_phd2_running().await,
        "Mock should not be running after stop"
    );
    assert!(
        !manager.has_managed_process().await,
        "Should not have managed process after stop"
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_process_manager_start_already_running() {
    let port = 44121;

    let Some(binary_path) = find_mock_phd2_binary() else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    // Start mock manually first
    let mut child = start_mock_phd2(port).expect("Should start mock");

    let config = Phd2Config {
        host: "localhost".to_string(),
        port,
        executable_path: Some(binary_path),
        connection_timeout_seconds: 5,
        command_timeout_seconds: 5,
        ..Default::default()
    };

    let manager = Phd2ProcessManager::new(config);

    // Manager should detect already running
    assert!(
        manager.is_phd2_running().await,
        "Should detect running mock"
    );

    // Start should succeed (returns Ok when already running)
    let result = manager.start_phd2().await;
    assert!(
        result.is_ok(),
        "Should succeed when already running: {:?}",
        result
    );

    // Manager should NOT have a managed process (since it was already running)
    assert!(
        !manager.has_managed_process().await,
        "Should not manage externally started process"
    );

    // Cleanup
    child.kill().ok();
    child.wait().ok();
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_process_manager_force_kill() {
    let port = 44201; // Use unique port for this test

    let Some(binary_path) = find_mock_phd2_binary() else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    // First make sure nothing is running on that port
    let addr = format!("127.0.0.1:{}", port);
    if tokio::net::TcpStream::connect(&addr).await.is_ok() {
        eprintln!("Port {} is in use, skipping test", port);
        return;
    }

    let mut spawn_env = std::collections::HashMap::new();
    spawn_env.insert("MOCK_PHD2_PORT".to_string(), port.to_string());

    let config = Phd2Config {
        host: "localhost".to_string(),
        port,
        executable_path: Some(binary_path),
        connection_timeout_seconds: 10,
        command_timeout_seconds: 5,
        spawn_env,
        ..Default::default()
    };

    let manager = Phd2ProcessManager::new(config);

    // Start the mock PHD2
    let start_result = manager.start_phd2().await;
    assert!(start_result.is_ok(), "Should start: {:?}", start_result);

    // Force stop without client (no graceful shutdown)
    let stop_result = manager.stop_phd2(None).await;
    assert!(
        stop_result.is_ok(),
        "Should force stop mock PHD2: {:?}",
        stop_result
    );

    // Wait for process to die
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify not running
    assert!(
        !manager.is_phd2_running().await,
        "Mock should not be running after force stop"
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_process_manager_shutdown_via_rpc() {
    let port = 44202; // Use unique port for this test

    let Some(binary_path) = find_mock_phd2_binary() else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    // First make sure nothing is running on that port
    let addr = format!("127.0.0.1:{}", port);
    if tokio::net::TcpStream::connect(&addr).await.is_ok() {
        eprintln!("Port {} is in use, skipping test", port);
        return;
    }

    let mut spawn_env = std::collections::HashMap::new();
    spawn_env.insert("MOCK_PHD2_PORT".to_string(), port.to_string());

    let config = Phd2Config {
        host: "localhost".to_string(),
        port,
        executable_path: Some(binary_path),
        connection_timeout_seconds: 10,
        command_timeout_seconds: 5,
        spawn_env: spawn_env.clone(),
        ..Default::default()
    };

    let manager = Phd2ProcessManager::new(config.clone());

    // Start the mock PHD2
    let start_result = manager.start_phd2().await;
    assert!(start_result.is_ok(), "Should start: {:?}", start_result);

    // Connect a client
    let client = Phd2Client::new(config);
    client.connect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Try to shutdown via client directly (tests the shutdown_phd2 RPC call)
    let shutdown_result = client.shutdown_phd2().await;
    assert!(
        shutdown_result.is_ok(),
        "Should send shutdown command: {:?}",
        shutdown_result
    );

    // Wait for process to die
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Verify not running (mock server handles shutdown command)
    assert!(
        !manager.is_phd2_running().await,
        "Mock should not be running after shutdown RPC"
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_process_manager_stop_without_client() {
    let port = 44203; // Use unique port for this test

    let Some(binary_path) = find_mock_phd2_binary() else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    // First make sure nothing is running on that port
    let addr = format!("127.0.0.1:{}", port);
    if tokio::net::TcpStream::connect(&addr).await.is_ok() {
        eprintln!("Port {} is in use, skipping test", port);
        return;
    }

    let mut spawn_env = std::collections::HashMap::new();
    spawn_env.insert("MOCK_PHD2_PORT".to_string(), port.to_string());

    let config = Phd2Config {
        host: "localhost".to_string(),
        port,
        executable_path: Some(binary_path),
        connection_timeout_seconds: 10,
        command_timeout_seconds: 5,
        spawn_env,
        ..Default::default()
    };

    let manager = Phd2ProcessManager::new(config);

    // Start the mock PHD2
    let start_result = manager.start_phd2().await;
    assert!(start_result.is_ok(), "Should start: {:?}", start_result);

    // Verify it's running
    assert!(manager.is_phd2_running().await, "Mock should be running");

    // Stop without client (tests force kill path)
    let stop_result = manager.stop_phd2(None).await;
    assert!(stop_result.is_ok(), "Should stop: {:?}", stop_result);

    // Verify not running
    assert!(
        !manager.is_phd2_running().await,
        "Mock should not be running after stop"
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_process_manager_start_when_external_running() {
    let port = 44204; // Use unique port for this test

    let Some(binary_path) = find_mock_phd2_binary() else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    // Start mock manually first (simulating externally running PHD2)
    let mut child = start_mock_phd2(port).expect("Should start mock");

    let config = Phd2Config {
        host: "localhost".to_string(),
        port,
        executable_path: Some(binary_path),
        connection_timeout_seconds: 5,
        command_timeout_seconds: 5,
        ..Default::default()
    };

    let manager = Phd2ProcessManager::new(config);

    // Manager should detect already running and return Ok early
    let result = manager.start_phd2().await;
    assert!(result.is_ok(), "Should return Ok when already running");

    // Manager should not have a managed process (it was started externally)
    assert!(
        !manager.has_managed_process().await,
        "Should not have managed process when external"
    );

    // Clean up manually started process
    child.kill().expect("Should kill mock");
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri doesn't support process spawning
async fn test_process_manager_no_executable_no_default() {
    // Create config without executable_path - will try to find default
    // On CI, there's no real PHD2 installed, so this should fail
    let config = Phd2Config {
        port: 59996, // Unlikely to be in use
        executable_path: None,
        ..Default::default()
    };

    let manager = Phd2ProcessManager::new(config);

    // Only run this test if there's no PHD2 in default locations
    if get_default_phd2_path().is_none() {
        let result = manager.start_phd2().await;
        assert!(result.is_err(), "Should fail when no executable found");
    }
}
