//! Integration tests for the filemonitor service.
//!
//! Combines server startup, run-loop reload/stop signals, and CLI argument
//! handling into a single test binary so cargo only links one integration
//! target. Coverage of invalid-configuration rejection lives in the BDD
//! scenario *Reject invalid configuration sources* in
//! `tests/features/configuration.feature`.

#[cfg(not(miri))]
use filemonitor::{Config, DeviceConfig, FileConfig, ParsingConfig, ServerConfig};
#[cfg(not(miri))]
use std::path::PathBuf;

#[tokio::test]
#[cfg(not(miri))]
async fn test_start_server_creation() {
    use filemonitor::start_server;
    use std::time::Duration;
    use tokio::time::timeout;

    let config = Config {
        device: DeviceConfig {
            name: "Test Server".to_string(),
            unique_id: "test-server-001".to_string(),
            description: "Test server device".to_string(),
        },
        file: FileConfig {
            path: PathBuf::from("test_server_file.txt"),
            polling_interval: Duration::from_secs(1),
        },
        parsing: ParsingConfig {
            rules: vec![],
            case_sensitive: false,
        },
        server: ServerConfig {
            port: 0,
            discovery_port: None,
            tls: None,
            auth: None,
        },
    };

    std::fs::write(&config.file.path, "test").unwrap();

    let server_future = start_server(config.clone());
    let result = timeout(Duration::from_millis(100), server_future).await;

    std::fs::remove_file(&config.file.path).unwrap();

    // We expect timeout since server.start() would block indefinitely
    assert!(result.is_err());
}

// Both halves of this test bind the ASCOM Alpaca discovery port, so they must
// run sequentially. We combine them into a single test to avoid parallel port
// conflicts.
#[tokio::test(flavor = "multi_thread")]
#[cfg(not(miri))]
async fn test_server_loop_stop_and_reload() {
    use filemonitor::run_server_loop;
    use std::io::Write;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    // --- Part 1: stop signal ---
    {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let monitor_file = dir.path().join("monitor.txt");

        std::fs::write(&monitor_file, "SAFE").unwrap();

        let config = serde_json::json!({
            "device": {
                "name": "Test",
                "unique_id": "test-stop-001",
                "description": "Test device"
            },
            "file": {
                "path": monitor_file.to_str().unwrap(),
                "polling_interval": "60s"
            },
            "parsing": {
                "rules": [],
                "case_sensitive": false
            },
            "server": {
                "port": 0
            }
        });

        let mut f = std::fs::File::create(&config_path).unwrap();
        f.write_all(config.to_string().as_bytes()).unwrap();

        let result = run_server_loop(
            &config_path,
            || {
                Box::pin(async {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                })
            },
            || Box::pin(std::future::pending()),
        )
        .await;

        assert!(result.is_ok(), "stop test failed: {:?}", result.err());
    }

    // --- Part 2: reload signal ---
    {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let monitor_file = dir.path().join("monitor.txt");

        std::fs::write(&monitor_file, "SAFE").unwrap();

        let config = serde_json::json!({
            "device": {
                "name": "Test",
                "unique_id": "test-reload-001",
                "description": "Test device"
            },
            "file": {
                "path": monitor_file.to_str().unwrap(),
                "polling_interval": "60s"
            },
            "parsing": {
                "rules": [],
                "case_sensitive": false
            },
            "server": {
                "port": 0
            }
        });

        let mut f = std::fs::File::create(&config_path).unwrap();
        f.write_all(config.to_string().as_bytes()).unwrap();

        let loop_count = Arc::new(AtomicU32::new(0));
        let loop_count_reload = Arc::clone(&loop_count);
        let loop_count_stop = Arc::clone(&loop_count);

        let result = run_server_loop(
            &config_path,
            move || {
                let count = loop_count_stop.clone();
                Box::pin(async move {
                    // Stop after the reload has happened (loop_count >= 2)
                    loop {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        if count.load(Ordering::Relaxed) >= 2 {
                            break;
                        }
                    }
                })
            },
            move || {
                let count = loop_count_reload.clone();
                Box::pin(async move {
                    let current = count.fetch_add(1, Ordering::Relaxed);
                    if current == 0 {
                        // First iteration: trigger a reload after a brief delay
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    } else {
                        // Subsequent iterations: don't reload again
                        std::future::pending::<()>().await;
                    }
                })
            },
        )
        .await;

        assert!(result.is_ok(), "reload test failed: {:?}", result.err());
        assert!(
            loop_count.load(Ordering::Relaxed) >= 2,
            "Server loop should have run at least twice (once initial + once after reload)"
        );
    }
}

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
    let output = bdd_infra::run_once("filemonitor", &["--help"], None);

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

/// Assert that a `Box<dyn Error>` reached main (config-load failure) and that
/// clap did *not* reject the arguments — i.e., `--log-level <variant>` parsed
/// successfully and the binary then failed opening the missing config.
#[cfg(not(miri))]
fn assert_config_not_found(output: &std::process::Output) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "expected non-zero exit");
    assert!(
        !stderr.contains("error: invalid value") && !stderr.contains("error: unexpected argument"),
        "clap rejected the arguments; stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("NotFound"),
        "expected config-not-found error; stderr:\n{}",
        stderr
    );
}

#[test]
#[cfg(not(miri))] // Skip under miri - process spawning not supported
fn test_cli_log_level_flag_accepted() {
    // Skip under sanitizers due to proc-macro compilation issues
    if std::env::var("RUSTFLAGS")
        .unwrap_or_default()
        .contains("sanitizer")
    {
        return;
    }
    // Pass --log-level alongside a nonexistent config; the binary should
    // accept the flag (clap parse OK) and then fail opening the config.
    let output = bdd_infra::run_once(
        "filemonitor",
        &["--config", "nonexistent.json", "--log-level", "debug"],
        None,
    );

    assert_config_not_found(&output);
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
    // Verify clap accepts each tracing Level variant. The nonexistent config
    // makes the binary fail fast after argument parsing.
    for log_level in &["error", "warn", "info", "debug", "trace"] {
        let output = bdd_infra::run_once(
            "filemonitor",
            &["--config", "nonexistent.json", "--log-level", log_level],
            None,
        );

        assert_config_not_found(&output);
    }
}
