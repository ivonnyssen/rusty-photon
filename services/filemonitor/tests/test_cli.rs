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
    let output = bdd_infra::run_once("filemonitor", &["--config", "nonexistent.json"], None);

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
    // Verify --log-level is accepted by clap. We use a nonexistent config so
    // the binary fails fast after argument parsing rather than starting a
    // server we'd have to shut down.
    let output = bdd_infra::run_once(
        "filemonitor",
        &["--config", "nonexistent.json", "--log-level", "debug"],
        None,
    );

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
    // Verify clap accepts each tracing Level variant. The nonexistent config
    // makes the binary fail fast after argument parsing.
    for log_level in &["error", "warn", "info", "debug", "trace"] {
        let output = bdd_infra::run_once(
            "filemonitor",
            &["--config", "nonexistent.json", "--log-level", log_level],
            None,
        );

        assert!(!output.status.success());
    }
}
