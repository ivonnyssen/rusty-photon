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
        stderr.contains("NotFound") || stderr.contains("No such file"),
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
