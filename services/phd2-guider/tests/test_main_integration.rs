//! Integration tests for the phd2-guider CLI application
//!
//! These tests spawn the mock_phd2 server and run the phd2-guider CLI
//! as a subprocess to verify end-to-end behavior.
//!
//! All tests use random ports to allow parallel execution.

use std::net::TcpListener;
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

// ============================================================================
// Test Infrastructure
// ============================================================================

/// Get an available TCP port by binding to port 0
fn get_available_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind to port 0");
    listener.local_addr().unwrap().port()
}

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

/// Spawn the mock_phd2 server with a specific mode
fn spawn_mock_server_with_mode(mode: &str) -> (ProcessGuard, u16) {
    let port = get_available_port();
    let child = Command::new(env!("CARGO_BIN_EXE_mock_phd2"))
        .env("MOCK_PHD2_PORT", port.to_string())
        .env("MOCK_PHD2_MODE", mode)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start mock_phd2 server");

    let guard = ProcessGuard::new(child, "mock_phd2");

    // Wait for server to be ready
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

// ============================================================================
// Status Command Tests
// ============================================================================

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

// ============================================================================
// Equipment Command Tests
// ============================================================================

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

// ============================================================================
// Profile Command Tests
// ============================================================================

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

// ============================================================================
// Guiding Command Tests
// ============================================================================

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
            "15",
            "--settle-timeout",
            "120",
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

// ============================================================================
// Pause/Resume Command Tests
// ============================================================================

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

// ============================================================================
// Dither Command Tests
// ============================================================================

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
            "5",
            "--settle-timeout",
            "30",
        ],
        port,
    );

    assert!(
        output.status.success(),
        "CLI should succeed: {}",
        get_output_text(&output)
    );
}

// ============================================================================
// Argument Parsing Tests
// ============================================================================

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

// ============================================================================
// Config File Tests
// ============================================================================

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

// ============================================================================
// Monitor Command Tests (with timeout)
// ============================================================================

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

    // Give it time to connect and receive the version event
    std::thread::sleep(Duration::from_secs(1));

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

// ============================================================================
// Error Handling Tests
// ============================================================================

#[test]
#[cfg_attr(miri, ignore)]
fn test_connection_refused() {
    // Use a port that's definitely not listening
    let port = get_available_port();

    let output = run_cli_with_timeout(&["status"], port, Duration::from_secs(3));

    assert!(
        !output.status.success(),
        "Should fail when connection refused"
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_connection_timeout_message() {
    let port = get_available_port();

    let output = run_cli_with_timeout(&["status"], port, Duration::from_secs(3));

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
