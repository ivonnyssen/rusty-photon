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
use std::io::{BufRead, BufReader};
#[cfg_attr(miri, allow(unused_imports))]
use std::net::TcpListener;
#[cfg_attr(miri, allow(unused_imports))]
use std::path::PathBuf;
#[cfg_attr(miri, allow(unused_imports))]
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

/// Get an available TCP port by binding to port 0
fn get_available_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind to port 0");
    listener.local_addr().unwrap().port()
}

/// Fixed port for error-path tests (exit_immediately, connection_timeout).
/// These tests require that nothing is listening on the port, so they use a
/// dedicated fixed port and serialize via ERROR_PATH_LOCK to prevent any
/// interference from parallel tests whose mock servers use auto-assigned ports.
#[cfg(not(miri))]
const ERROR_PATH_PORT: u16 = 19876;

/// Mutex to serialize error-path process tests that share ERROR_PATH_PORT.
#[cfg(not(miri))]
static ERROR_PATH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
        connection_timeout: Duration::from_secs(30),
        command_timeout: Duration::from_secs(30),
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
#[cfg(not(miri))] // get_default_phd2_path() spawns a process via Command::output()
fn test_get_default_phd2_path() {
    // This test just verifies the function doesn't panic
    let path = get_default_phd2_path();
    if let Some(p) = path {
        assert!(p.exists(), "Default PHD2 path should exist if returned");
    }
}

#[test]
fn test_load_config() {
    let config_path = PathBuf::from("tests/config.json");
    let result = load_config(&config_path);
    assert!(result.is_ok(), "Should load valid config file");

    let config = result.unwrap();
    assert_eq!(config.phd2.host, "localhost");
    assert_eq!(config.phd2.port, 4400);
}

#[test]
fn test_load_config_with_defaults() {
    let config_path = PathBuf::from("tests/config_minimal.json");
    let result = load_config(&config_path);
    assert!(result.is_ok(), "Should load minimal config with defaults");

    let config = result.unwrap();
    // Defaults should be applied
    assert_eq!(config.phd2.connection_timeout, Duration::from_secs(10));
    assert_eq!(config.settling.pixels, 0.5);
}

#[test]
fn test_load_config_file_not_found() {
    let config_path = PathBuf::from("tests/nonexistent.json");
    let result = load_config(&config_path);
    assert!(result.is_err());
}

// ============================================================================
// Client Creation Tests
// ============================================================================

#[test]
#[cfg(not(miri))] // create_test_config() calls get_default_phd2_path() which spawns a process
fn test_client_creation() {
    let config = create_test_config();
    let client = Phd2Client::new(config);
    // Client should be created successfully
    assert!(std::mem::size_of_val(&client) > 0);
}

#[test]
#[cfg(not(miri))] // create_test_config() calls get_default_phd2_path() which spawns a process
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
#[cfg(not(miri))]
async fn test_connect_to_nonexistent_server() {
    let config = Phd2Config {
        host: "localhost".to_string(),
        port: 59999, // Unlikely to be in use
        connection_timeout: Duration::from_secs(2),
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
#[cfg(not(miri))] // create_test_config() calls get_default_phd2_path() which spawns a process
async fn test_send_request_when_not_connected() {
    let config = create_test_config();
    let client = Phd2Client::new(config);

    // Don't connect, try to get state
    let result = client.get_app_state().await;
    assert!(result.is_err(), "Should fail when not connected");
}

#[tokio::test]
#[cfg(not(miri))]
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

/// Start the mock PHD2 server with auto-assigned port and specified mode.
///
/// This function spawns the mock with port 0, reads the actual assigned port
/// from stdout, and returns (port, child). This avoids port collision issues
/// in parallel test execution.
#[cfg(not(miri))]
fn start_mock_phd2_auto_port(mode: &str) -> Option<(u16, Child)> {
    let binary = find_mock_phd2_binary()?;

    let mut child = Command::new(binary)
        .env("MOCK_PHD2_PORT", "0")
        .env("MOCK_PHD2_MODE", mode)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .ok()?;

    // Read stdout to get the actual port
    let stdout = child.stdout.take()?;
    let reader = BufReader::new(stdout);

    for line in reader.lines() {
        let line = line.ok()?;
        if let Some(port_str) = line.strip_prefix("MOCK_PHD2_PORT:") {
            let port: u16 = port_str.parse().ok()?;
            return Some((port, child));
        }
    }

    // If we didn't find the port line, the mock failed to start
    let _ = child.kill();
    None
}

#[tokio::test]
#[cfg(not(miri))]
async fn test_mock_phd2_connection() {
    let Some((port, mut child)) = start_mock_phd2_auto_port("normal") else {
        eprintln!(
            "Mock PHD2 binary not found. Run 'cargo build -p phd2-guider --bin mock_phd2' first"
        );
        return;
    };

    let config = Phd2Config {
        host: "127.0.0.1".to_string(),
        port,
        connection_timeout: Duration::from_secs(5),
        command_timeout: Duration::from_secs(5),
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
#[cfg(not(miri))]
async fn test_mock_phd2_get_app_state() {
    let Some((port, mut child)) = start_mock_phd2_auto_port("normal") else {
        eprintln!("Mock PHD2 binary not found or failed to start");
        return;
    };

    let config = Phd2Config {
        host: "127.0.0.1".to_string(),
        port,
        connection_timeout: Duration::from_secs(5),
        command_timeout: Duration::from_secs(5),
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
#[cfg(not(miri))]
async fn test_mock_phd2_get_profiles() {
    let Some((port, mut child)) = start_mock_phd2_auto_port("normal") else {
        eprintln!("Mock PHD2 binary not found or failed to start");
        return;
    };

    let config = Phd2Config {
        host: "127.0.0.1".to_string(),
        port,
        connection_timeout: Duration::from_secs(5),
        command_timeout: Duration::from_secs(5),
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
#[cfg(not(miri))]
async fn test_mock_phd2_get_equipment() {
    let Some((port, mut child)) = start_mock_phd2_auto_port("normal") else {
        eprintln!("Mock PHD2 binary not found or failed to start");
        return;
    };

    let config = Phd2Config {
        host: "127.0.0.1".to_string(),
        port,
        connection_timeout: Duration::from_secs(5),
        command_timeout: Duration::from_secs(5),
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
#[cfg(not(miri))]
async fn test_mock_phd2_exposure_methods() {
    let Some((port, mut child)) = start_mock_phd2_auto_port("normal") else {
        eprintln!("Mock PHD2 binary not found or failed to start");
        return;
    };

    let config = Phd2Config {
        host: "127.0.0.1".to_string(),
        port,
        connection_timeout: Duration::from_secs(5),
        command_timeout: Duration::from_secs(5),
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
#[cfg(not(miri))]
async fn test_mock_phd2_calibration_methods() {
    let Some((port, mut child)) = start_mock_phd2_auto_port("normal") else {
        eprintln!("Mock PHD2 binary not found or failed to start");
        return;
    };

    let config = Phd2Config {
        host: "127.0.0.1".to_string(),
        port,
        connection_timeout: Duration::from_secs(5),
        command_timeout: Duration::from_secs(5),
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
#[cfg(not(miri))]
async fn test_mock_phd2_guiding_control() {
    let Some((port, mut child)) = start_mock_phd2_auto_port("normal") else {
        eprintln!("Mock PHD2 binary not found or failed to start");
        return;
    };

    let config = Phd2Config {
        host: "127.0.0.1".to_string(),
        port,
        connection_timeout: Duration::from_secs(5),
        command_timeout: Duration::from_secs(5),
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
#[cfg(not(miri))]
async fn test_mock_phd2_star_operations() {
    let Some((port, mut child)) = start_mock_phd2_auto_port("normal") else {
        eprintln!("Mock PHD2 binary not found or failed to start");
        return;
    };

    let config = Phd2Config {
        host: "127.0.0.1".to_string(),
        port,
        connection_timeout: Duration::from_secs(5),
        command_timeout: Duration::from_secs(5),
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
#[cfg(not(miri))]
async fn test_mock_phd2_cooling() {
    let Some((port, mut child)) = start_mock_phd2_auto_port("normal") else {
        eprintln!("Mock PHD2 binary not found or failed to start");
        return;
    };

    let config = Phd2Config {
        host: "127.0.0.1".to_string(),
        port,
        connection_timeout: Duration::from_secs(5),
        command_timeout: Duration::from_secs(5),
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
#[cfg(not(miri))]
async fn test_mock_phd2_star_image() {
    let Some((port, mut child)) = start_mock_phd2_auto_port("normal") else {
        eprintln!("Mock PHD2 binary not found or failed to start");
        return;
    };

    let config = Phd2Config {
        host: "127.0.0.1".to_string(),
        port,
        connection_timeout: Duration::from_secs(5),
        command_timeout: Duration::from_secs(5),
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
#[cfg(not(miri))]
async fn test_mock_phd2_event_subscription() {
    let Some((port, mut child)) = start_mock_phd2_auto_port("normal") else {
        eprintln!("Mock PHD2 binary not found or failed to start");
        return;
    };

    let config = Phd2Config {
        host: "127.0.0.1".to_string(),
        port,
        connection_timeout: Duration::from_secs(5),
        command_timeout: Duration::from_secs(5),
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
#[cfg(not(miri))]
async fn test_mock_phd2_reconnect_on_disconnect() {
    let Some((port, mut child)) = start_mock_phd2_auto_port("normal") else {
        eprintln!("Mock PHD2 binary not found or failed to start");
        return;
    };

    let config = Phd2Config {
        host: "127.0.0.1".to_string(),
        port,
        connection_timeout: Duration::from_secs(5),
        command_timeout: Duration::from_secs(5),
        reconnect: ReconnectConfig {
            enabled: true,
            interval: Duration::from_secs(1),
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
#[cfg(not(miri))]
async fn test_process_manager_start_stop_mock() {
    let port = get_available_port();

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
        host: "127.0.0.1".to_string(),
        port,
        executable_path: Some(binary_path),
        connection_timeout: Duration::from_secs(10),
        command_timeout: Duration::from_secs(5),
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
#[cfg(not(miri))]
async fn test_process_manager_start_already_running() {
    let Some(binary_path) = find_mock_phd2_binary() else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    // Start mock manually first with auto-assigned port
    let Some((port, mut child)) = start_mock_phd2_auto_port("normal") else {
        eprintln!("Mock PHD2 failed to start");
        return;
    };

    let config = Phd2Config {
        host: "127.0.0.1".to_string(),
        port,
        executable_path: Some(binary_path),
        connection_timeout: Duration::from_secs(5),
        command_timeout: Duration::from_secs(5),
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
#[cfg(not(miri))]
async fn test_process_manager_force_kill() {
    let port = get_available_port();

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
        host: "127.0.0.1".to_string(),
        port,
        executable_path: Some(binary_path),
        connection_timeout: Duration::from_secs(10),
        command_timeout: Duration::from_secs(5),
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
#[cfg(not(miri))]
async fn test_process_manager_shutdown_via_rpc() {
    let port = get_available_port();

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
        host: "127.0.0.1".to_string(),
        port,
        executable_path: Some(binary_path),
        connection_timeout: Duration::from_secs(10),
        command_timeout: Duration::from_secs(5),
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
#[cfg(not(miri))]
async fn test_process_manager_stop_without_client() {
    let port = get_available_port();

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
        host: "127.0.0.1".to_string(),
        port,
        executable_path: Some(binary_path),
        connection_timeout: Duration::from_secs(10),
        command_timeout: Duration::from_secs(5),
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
#[cfg(not(miri))]
async fn test_process_manager_start_when_external_running() {
    let Some(binary_path) = find_mock_phd2_binary() else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    // Start mock manually first (simulating externally running PHD2) with auto-assigned port
    let Some((port, mut child)) = start_mock_phd2_auto_port("normal") else {
        eprintln!("Mock PHD2 failed to start");
        return;
    };

    let config = Phd2Config {
        host: "127.0.0.1".to_string(),
        port,
        executable_path: Some(binary_path),
        connection_timeout: Duration::from_secs(5),
        command_timeout: Duration::from_secs(5),
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
#[cfg(not(miri))]
async fn test_process_manager_no_executable_no_default() {
    // Verify that start_phd2() fails with ExecutableNotFound when no
    // executable_path is configured and PHD2 isn't installed.
    // Use port 1 (privileged) so is_phd2_running() can't get a false
    // positive from a random service listening on the port.
    let config = Phd2Config {
        port: 1,
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

// ============================================================================
// Error Path Tests (using mock_phd2 modes)
// ============================================================================

#[tokio::test]
#[cfg(not(miri))]
async fn test_process_exit_immediately() {
    let _lock = ERROR_PATH_LOCK.lock().await;
    let port = ERROR_PATH_PORT;

    let Some(binary_path) = find_mock_phd2_binary() else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    let mut spawn_env = std::collections::HashMap::new();
    spawn_env.insert("MOCK_PHD2_PORT".to_string(), port.to_string());
    spawn_env.insert("MOCK_PHD2_MODE".to_string(), "exit_immediately".to_string());

    let config = Phd2Config {
        host: "127.0.0.1".to_string(),
        port,
        executable_path: Some(binary_path),
        connection_timeout: Duration::from_secs(5),
        command_timeout: Duration::from_secs(5),
        spawn_env,
        ..Default::default()
    };

    let manager = Phd2ProcessManager::new(config);

    // Start should fail because the process exits immediately
    let result = manager.start_phd2().await;
    assert!(
        result.is_err(),
        "Should fail when process exits immediately"
    );

    // Verify the error message mentions premature exit
    let err_msg = format!("{:?}", result.unwrap_err());
    assert!(
        err_msg.contains("exited prematurely") || err_msg.contains("ProcessStartFailed"),
        "Error should indicate premature exit: {}",
        err_msg
    );
}

#[tokio::test]
#[cfg(not(miri))]
async fn test_process_connection_timeout() {
    let _lock = ERROR_PATH_LOCK.lock().await;
    let port = ERROR_PATH_PORT;

    let Some(binary_path) = find_mock_phd2_binary() else {
        eprintln!("Mock PHD2 binary not found");
        return;
    };

    let mut spawn_env = std::collections::HashMap::new();
    spawn_env.insert("MOCK_PHD2_PORT".to_string(), port.to_string());
    spawn_env.insert("MOCK_PHD2_MODE".to_string(), "no_listen".to_string());

    let config = Phd2Config {
        host: "127.0.0.1".to_string(),
        port,
        executable_path: Some(binary_path),
        connection_timeout: Duration::from_secs(2), // Short timeout for faster test
        command_timeout: Duration::from_secs(5),
        spawn_env,
        ..Default::default()
    };

    let manager = Phd2ProcessManager::new(config);

    // Start should fail due to timeout (process doesn't listen)
    let result = manager.start_phd2().await;
    assert!(result.is_err(), "Should fail when connection times out");

    // Verify the error is a timeout
    let err_msg = format!("{:?}", result.unwrap_err());
    assert!(
        err_msg.contains("Timeout") || err_msg.contains("did not become ready"),
        "Error should indicate timeout: {}",
        err_msg
    );

    // Clean up - force kill the no_listen process
    manager.stop_phd2(None).await.ok();
}

#[tokio::test]
#[cfg(not(miri))]
async fn test_graceful_shutdown_fails_fallback_to_kill() {
    // Use auto-assigned port to avoid port collision with parallel tests
    let Some((port, mut child)) = start_mock_phd2_auto_port("shutdown_fails") else {
        eprintln!("Mock PHD2 binary not found or failed to start");
        return;
    };

    let config = Phd2Config {
        host: "127.0.0.1".to_string(),
        port,
        connection_timeout: Duration::from_secs(10),
        command_timeout: Duration::from_secs(5),
        ..Default::default()
    };

    // Connect a client
    let client = Phd2Client::new(config);
    client.connect().await.unwrap();

    // Wait for version event
    let mut version = None;
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        version = client.get_phd2_version().await;
        if version.is_some() {
            break;
        }
    }
    assert!(version.is_some(), "Should have version");

    // Try graceful shutdown - this should "succeed" (return Ok) but
    // the process won't actually exit because it's in shutdown_fails mode
    let shutdown_result = client.shutdown_phd2().await;
    assert!(
        shutdown_result.is_ok(),
        "Shutdown command should succeed: {:?}",
        shutdown_result
    );

    // Wait a moment and verify the process is STILL running
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        child.try_wait().unwrap().is_none(),
        "Mock should still be running after ignored shutdown"
    );

    // Now force kill the process
    child.kill().unwrap();
    let _ = child.wait();

    // Verify not running (can't connect)
    let addr = format!("127.0.0.1:{}", port);
    assert!(
        tokio::net::TcpStream::connect(&addr).await.is_err(),
        "Mock should not be running after force kill"
    );
}

// ============================================================================
// CLI Subprocess Tests
// ============================================================================
//
// These tests spawn the mock_phd2 server and run the phd2-guider CLI as a
// subprocess to verify end-to-end behavior. All tests use random ports to
// allow parallel execution.

/// Wait for a TCP server to be ready on the given port
fn wait_for_server_ready(port: u16, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// Guard that kills a child process when dropped
struct ProcessGuard {
    child: Child,
    name: &'static str,
}

impl ProcessGuard {
    fn new(child: Child, name: &'static str) -> Self {
        Self { child, name }
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if let Err(e) = self.child.kill() {
            eprintln!("Failed to kill {} process: {}", self.name, e);
        }
        let _ = self.child.wait();
    }
}

/// Spawn the mock_phd2 server on a random port
fn spawn_mock_server() -> (ProcessGuard, u16) {
    spawn_mock_server_with_mode("normal")
}

/// Spawn the mock_phd2 server with a specific mode.
///
/// Uses `MOCK_PHD2_PORT=0` so the kernel assigns a port at bind time, and
/// reads the actual port back from the mock's stdout. This avoids a TOCTOU
/// race where another process could claim a previously probed "free" port
/// between probe and bind. Stderr is discarded — mock_phd2 logs every
/// request/response, and an undrained pipe buffer would deadlock the mock
/// once full.
fn spawn_mock_server_with_mode(mode: &str) -> (ProcessGuard, u16) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mock_phd2"))
        .env("MOCK_PHD2_PORT", "0")
        .env("MOCK_PHD2_MODE", mode)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start mock_phd2 server");

    let stdout = child.stdout.take().expect("stdout piped");
    let port = BufReader::new(stdout)
        .lines()
        .find_map(|line| {
            line.ok().and_then(|l| {
                l.strip_prefix("MOCK_PHD2_PORT:")
                    .and_then(|p| p.parse::<u16>().ok())
            })
        })
        .expect("Mock PHD2 did not announce its port on stdout");

    let guard = ProcessGuard::new(child, "mock_phd2");

    // Server is bound by the time the port line is printed, but wait for the
    // accept queue to settle before handing the port to the test.
    if !wait_for_server_ready(port, Duration::from_secs(5)) {
        panic!("Mock server did not start within timeout on port {}", port);
    }

    (guard, port)
}

/// Run the phd2-guider CLI with given arguments
fn run_cli(args: &[&str], port: u16) -> Output {
    run_cli_with_timeout(args, port, Duration::from_secs(10))
}

/// Run the phd2-guider CLI with a custom timeout
fn run_cli_with_timeout(args: &[&str], port: u16, timeout: Duration) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_phd2-guider"));
    cmd.args(["--port", &port.to_string()])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("Failed to spawn phd2-guider");

    // Wait with timeout
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > timeout {
                    child.kill().expect("Failed to kill timed-out process");
                    panic!("CLI command timed out after {:?}", timeout);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => panic!("Error waiting for CLI: {}", e),
        }
    }

    child.wait_with_output().expect("Failed to get CLI output")
}

/// Run the CLI without connecting to any server (for argument parsing tests)
fn run_cli_no_server(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_phd2-guider"))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to run CLI")
}

/// Check if output contains a string (case-insensitive in stdout or stderr)
fn output_contains(output: &Output, needle: &str) -> bool {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    stdout.to_lowercase().contains(&needle.to_lowercase())
        || stderr.to_lowercase().contains(&needle.to_lowercase())
}

/// Get combined output as string
fn get_output_text(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("STDOUT:\n{}\nSTDERR:\n{}", stdout, stderr)
}

// ----------------------------------------------------------------------------
// Status Command Tests
// ----------------------------------------------------------------------------

#[test]
#[cfg_attr(miri, ignore)]
fn test_status_shows_version() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["status"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
    assert!(
        output_contains(&output, "2.6.11"),
        "Should show PHD2 version: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_status_shows_state() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["status"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
    assert!(
        output_contains(&output, "Stopped"),
        "Should show app state: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_status_shows_equipment_status() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["status"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
    assert!(
        output_contains(&output, "equipment") || output_contains(&output, "connected"),
        "Should show equipment status: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_status_connection_failure() {
    // Use a port that nothing is listening on
    let port = get_available_port();
    let output = run_cli_with_timeout(&["status"], port, Duration::from_secs(5));

    assert!(
        !output.status.success(),
        "CLI should fail when server not available"
    );
}

// ----------------------------------------------------------------------------
// Equipment Command Tests
// ----------------------------------------------------------------------------

#[test]
#[cfg_attr(miri, ignore)]
fn test_connect_equipment() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["connect"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
    assert!(
        output_contains(&output, "connected") || output_contains(&output, "success"),
        "Should confirm equipment connected: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_disconnect_equipment() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["disconnect"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
    assert!(
        output_contains(&output, "disconnected") || output_contains(&output, "success"),
        "Should confirm equipment disconnected: {}",
        get_output_text(&output)
    );
}

// ----------------------------------------------------------------------------
// Profile Command Tests
// ----------------------------------------------------------------------------

#[test]
#[cfg_attr(miri, ignore)]
fn test_profiles_lists_all() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["profiles"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
    assert!(
        output_contains(&output, "Mock Profile"),
        "Should list mock profile: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_profiles_shows_current() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["profiles"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
    assert!(
        output_contains(&output, "current") || output_contains(&output, "profile"),
        "Should show current profile info: {}",
        get_output_text(&output)
    );
}

// ----------------------------------------------------------------------------
// Guiding Command Tests
// ----------------------------------------------------------------------------

#[test]
#[cfg_attr(miri, ignore)]
fn test_guide_basic() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["guide"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
    assert!(
        output_contains(&output, "guide") || output_contains(&output, "success"),
        "Should confirm guide command: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_guide_with_recalibrate() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["guide", "--recalibrate"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_guide_with_settle_params() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(
        &[
            "guide",
            "--settle-pixels",
            "1.0",
            "--settle-time",
            "15s",
            "--settle-timeout",
            "2m",
        ],
        port,
    );

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_guide_with_roi() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["guide", "--roi", "100,100,200,200"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_guide_invalid_roi_format() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["guide", "--roi", "invalid"], port);

    assert!(
        !output.status.success(),
        "CLI should fail with invalid ROI format"
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_guide_invalid_roi_not_enough_values() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["guide", "--roi", "100,100"], port);

    assert!(
        !output.status.success(),
        "CLI should fail with incomplete ROI"
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_stop_guiding() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["stop-guiding"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_stop_capture() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["stop-capture"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_loop_command() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["loop"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
}

// ----------------------------------------------------------------------------
// Pause/Resume Command Tests
// ----------------------------------------------------------------------------

#[test]
#[cfg_attr(miri, ignore)]
fn test_pause_basic() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["pause"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_pause_full() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["pause", "--full"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_resume() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["resume"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_is_paused() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["is-paused"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
    assert!(
        output_contains(&output, "paused"),
        "Should show paused status: {}",
        get_output_text(&output)
    );
}

// ----------------------------------------------------------------------------
// Dither Command Tests
// ----------------------------------------------------------------------------

#[test]
#[cfg_attr(miri, ignore)]
fn test_dither_basic() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["dither"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_dither_custom_amount() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["dither", "10.0"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_dither_ra_only() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["dither", "--ra-only"], port);

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_dither_with_settle_params() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(
        &[
            "dither",
            "5.0",
            "--settle-pixels",
            "0.3",
            "--settle-time",
            "5s",
            "--settle-timeout",
            "30s",
        ],
        port,
    );

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
}

// ----------------------------------------------------------------------------
// Argument Parsing Tests
// ----------------------------------------------------------------------------

#[test]
#[cfg_attr(miri, ignore)]
fn test_help_flag() {
    let output = run_cli_no_server(&["--help"]);

    assert!(output.status.success(), "Help should succeed");
    assert!(
        output_contains(&output, "phd2-guider"),
        "Help should mention program name: {}",
        get_output_text(&output)
    );
    assert!(
        output_contains(&output, "status") && output_contains(&output, "guide"),
        "Help should list subcommands: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_subcommand_help() {
    let output = run_cli_no_server(&["guide", "--help"]);

    assert!(output.status.success(), "Subcommand help should succeed");
    assert!(
        output_contains(&output, "recalibrate"),
        "Guide help should show options: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_custom_host_port() {
    let (_server, port) = spawn_mock_server();

    // Run CLI directly without using run_cli helper to test explicit --host and --port
    let output = Command::new(env!("CARGO_BIN_EXE_phd2-guider"))
        .args(["--host", "127.0.0.1", "--port", &port.to_string(), "status"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to run CLI");

    assert!(
        output.status.success(),
        "Custom host/port should work: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_log_level_debug() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["--log-level", "debug", "status"], port);

    assert!(
        output.status.success(),
        "Debug log level should work: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_log_level_warn() {
    let (_server, port) = spawn_mock_server();
    let output = run_cli(&["--log-level", "warn", "status"], port);

    assert!(
        output.status.success(),
        "Warn log level should work: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_invalid_subcommand() {
    let output = run_cli_no_server(&["nonexistent-command"]);

    assert!(!output.status.success(), "Invalid subcommand should fail");
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_missing_subcommand() {
    let output = run_cli_no_server(&[]);

    assert!(!output.status.success(), "Missing subcommand should fail");
}

// ----------------------------------------------------------------------------
// Config File Tests
// ----------------------------------------------------------------------------

#[test]
#[cfg_attr(miri, ignore)]
fn test_config_file_option() {
    let (_server, port) = spawn_mock_server();

    // Create a temporary config file
    let config_content = format!(
        r#"{{
            "phd2": {{
                "host": "localhost",
                "port": {}
            }}
        }}"#,
        port
    );

    let temp_dir = std::env::temp_dir();
    let config_path = temp_dir.join("test_phd2_config.json");
    std::fs::write(&config_path, config_content).expect("Failed to write config file");

    let output = Command::new(env!("CARGO_BIN_EXE_phd2-guider"))
        .args(["--config", config_path.to_str().unwrap(), "status"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to run CLI");

    // Clean up
    let _ = std::fs::remove_file(&config_path);

    assert!(
        output.status.success(),
        "Config file should be loaded: {}",
        get_output_text(&output)
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_config_file_not_found() {
    let output = run_cli_no_server(&["--config", "/nonexistent/path/config.json", "status"]);

    assert!(!output.status.success(), "Missing config file should fail");
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_invalid_config_file() {
    let temp_dir = std::env::temp_dir();
    let config_path = temp_dir.join("test_invalid_config.json");
    std::fs::write(&config_path, "{ invalid json }").expect("Failed to write config file");

    let output = run_cli_no_server(&["--config", config_path.to_str().unwrap(), "status"]);

    // Clean up
    let _ = std::fs::remove_file(&config_path);

    assert!(!output.status.success(), "Invalid config JSON should fail");
}

// ----------------------------------------------------------------------------
// Monitor Command Tests (with timeout)
// ----------------------------------------------------------------------------

#[test]
#[cfg_attr(miri, ignore)]
fn test_monitor_receives_version_event() {
    let (_server, port) = spawn_mock_server();

    // Start monitor in background and kill it after a short time
    let mut child = Command::new(env!("CARGO_BIN_EXE_phd2-guider"))
        .args(["--port", &port.to_string(), "monitor"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn monitor");

    // Give it time to connect and receive the version event.
    // Windows process startup + TCP connect is slower than Linux.
    std::thread::sleep(Duration::from_secs(3));

    // Kill the monitor
    child.kill().expect("Failed to kill monitor");
    let output = child.wait_with_output().expect("Failed to get output");

    // The version event should have been received
    assert!(
        output_contains(&output, "version") || output_contains(&output, "2.6.11"),
        "Monitor should receive version event: {}",
        get_output_text(&output)
    );
}

// ----------------------------------------------------------------------------
// CLI Error Handling Tests
// ----------------------------------------------------------------------------

#[test]
#[cfg_attr(miri, ignore)]
fn test_connection_refused() {
    // Use a port that's definitely not listening.
    // The CLI has a 10s connection timeout, so allow enough time for it to
    // fail and exit (Windows TCP refusal can be slower than Linux).
    let port = get_available_port();

    let output = run_cli_with_timeout(&["status"], port, Duration::from_secs(15));

    assert!(
        !output.status.success(),
        "Should fail when connection refused"
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_connection_timeout_message() {
    // The CLI has a 10s connection timeout; allow enough for it to fail
    // and exit on Windows where TCP refusal is slower.
    let port = get_available_port();

    let output = run_cli_with_timeout(&["status"], port, Duration::from_secs(15));

    assert!(
        !output.status.success(),
        "Should fail on connection timeout"
    );
    // Should have some error message
    assert!(
        !get_output_text(&output).trim().is_empty(),
        "Should have error output"
    );
}
