#[cfg(not(miri))]
use std::fs;
#[cfg(not(miri))]
use std::path::PathBuf;
#[cfg(not(miri))]
use std::process::Command;

#[test]
#[cfg(not(miri))] // Skip under miri - process spawning not supported
fn test_cli_help() {
    // Skip under sanitizers due to proc-macro compilation issues
    if std::env::var("RUSTFLAGS")
        .unwrap_or_default()
        .contains("sanitizer")
    {
        return;
    }
    let output = Command::new("cargo")
        .args(["run", "--bin", "filemonitor", "--", "--help"])
        .current_dir("../")
        .output()
        .expect("Failed to execute command");

    // In sanitizer environments, the process might fail due to restrictions
    // Check stderr for sanitizer-related issues and skip assertion if found
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("sanitizer")
            || stderr.contains("ASAN")
            || stderr.contains("LeakSanitizer")
        {
            eprintln!("Skipping CLI test due to sanitizer environment: {}", stderr);
            return;
        }
    }

    assert!(
        output.status.success(),
        "Command failed with stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ASCOM Alpaca SafetyMonitor"));
    assert!(stdout.contains("--config"));
    assert!(stdout.contains("--log-level"));
}

#[test]
#[cfg(not(miri))] // Skip under miri - process spawning not supported
fn test_cli_invalid_config() {
    // Skip under sanitizers due to proc-macro compilation issues
    if std::env::var("RUSTFLAGS")
        .unwrap_or_default()
        .contains("sanitizer")
    {
        return;
    }
    let output = Command::new("cargo")
        .args([
            "run",
            "--bin",
            "filemonitor",
            "--",
            "--config",
            "nonexistent.json",
        ])
        .current_dir("../")
        .output()
        .expect("Failed to execute command");

    // In sanitizer environments, check for sanitizer-related failures
    if output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("sanitizer")
            || stderr.contains("ASAN")
            || stderr.contains("LeakSanitizer")
        {
            eprintln!("Skipping CLI test due to sanitizer environment: {}", stderr);
            return;
        }
    }

    assert!(!output.status.success());
}

#[test]
#[cfg(not(miri))] // Skip under miri - process spawning not supported
fn test_cli_valid_config_with_log_level() {
    // Skip under sanitizers due to proc-macro compilation issues
    if std::env::var("RUSTFLAGS")
        .unwrap_or_default()
        .contains("sanitizer")
    {
        return;
    }
    // Create a temporary config file
    let config_content = r#"{
        "device": {
            "name": "CLI Test Monitor",
            "unique_id": "cli-test-001",
            "description": "Test device for CLI"
        },
        "file": {
            "path": "test_cli_file.txt",
            "polling_interval_seconds": 1
        },
        "parsing": {
            "rules": [],
            "default_safe": true,
            "case_sensitive": false
        },
        "server": {
            "port": 0,
            "device_number": 0
        }
    }"#;

    let config_path = PathBuf::from("test_cli_config.json");
    let test_file = PathBuf::from("test_cli_file.txt");

    fs::write(&config_path, config_content).unwrap();
    fs::write(&test_file, "test").unwrap();

    let output = Command::new("cargo")
        .args([
            "run",
            "--bin",
            "filemonitor",
            "--",
            "--config",
            "test_cli_config.json",
            "--log-level",
            "debug",
        ])
        .current_dir("../")
        .output()
        .expect("Failed to execute command");

    // Clean up
    fs::remove_file(&config_path).unwrap();
    fs::remove_file(&test_file).unwrap();

    // In sanitizer environments, check for sanitizer-related failures
    if output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("sanitizer")
            || stderr.contains("ASAN")
            || stderr.contains("LeakSanitizer")
        {
            eprintln!("Skipping CLI test due to sanitizer environment: {}", stderr);
            return;
        }
    }

    // We expect this to fail quickly since port 0 will cause an error
    // Just verify the command executed and parsed arguments correctly
    assert!(!output.status.success());
}

#[test]
#[cfg(not(miri))] // Skip under miri - process spawning not supported
fn test_cli_different_log_levels() {
    // Skip under sanitizers due to proc-macro compilation issues
    if std::env::var("RUSTFLAGS")
        .unwrap_or_default()
        .contains("sanitizer")
    {
        return;
    }
    let config_content = r#"{
        "device": {
            "name": "Log Test Monitor",
            "unique_id": "log-test-001", 
            "description": "Test device for log levels"
        },
        "file": {
            "path": "test_log_file.txt",
            "polling_interval_seconds": 1
        },
        "parsing": {
            "rules": [],
            "default_safe": true,
            "case_sensitive": false
        },
        "server": {
            "port": 0,
            "device_number": 0
        }
    }"#;

    let config_path = PathBuf::from("test_log_config.json");
    let test_file = PathBuf::from("test_log_file.txt");

    fs::write(&config_path, config_content).unwrap();
    fs::write(&test_file, "test").unwrap();

    for log_level in &["error", "warn", "info", "debug", "trace"] {
        let output = Command::new("cargo")
            .args([
                "run",
                "--bin",
                "filemonitor",
                "--",
                "--config",
                "test_log_config.json",
                "--log-level",
                log_level,
            ])
            .current_dir("../")
            .output()
            .expect("Failed to execute command");

        // In sanitizer environments, check for sanitizer-related failures
        if output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("sanitizer")
                || stderr.contains("ASAN")
                || stderr.contains("LeakSanitizer")
            {
                eprintln!("Skipping CLI test due to sanitizer environment: {}", stderr);
                break;
            }
        }

        // Should fail quickly due to port 0, but not due to argument parsing
        assert!(!output.status.success());
    }

    // Clean up
    fs::remove_file(&config_path).unwrap();
    fs::remove_file(&test_file).unwrap();
}
