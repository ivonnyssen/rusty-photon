//! Integration tests for ServiceHandle using test_service binary.

use bdd_infra::ServiceHandle;

/// Create a temp manifest dir with a unique env var pointing to the given binary.
/// Each test gets its own env var name to avoid parallel test interference.
fn setup_manifest(env_var_name: &str, binary_path: &str) -> tempfile::TempDir {
    std::env::set_var(env_var_name, binary_path);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        format!(
            r#"
[package]
name = "test"
version = "0.1.0"
edition = "2021"

[package.metadata.bdd]
env_var = "{env_var_name}"
"#
        ),
    )
    .unwrap();
    dir
}

/// Create a temp manifest dir with no env var set, so find_binary falls back
/// to cargo run. Uses the real bdd-infra package name so `cargo run --package
/// bdd-infra` resolves to the test_service binary.
fn setup_manifest_no_binary(env_var_name: &str) -> tempfile::TempDir {
    std::env::remove_var(env_var_name);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        format!(
            r#"
[package]
name = "test"
version = "0.1.0"
edition = "2021"

[package.metadata.bdd]
env_var = "{env_var_name}"
"#
        ),
    )
    .unwrap();
    dir
}

/// Write an empty config file and return it.
fn empty_config() -> tempfile::NamedTempFile {
    tempfile::NamedTempFile::new().unwrap()
}

/// Write a config file containing "fail" to trigger test_service exit.
fn fail_config() -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), "fail").unwrap();
    file
}

// ---------------------------------------------------------------------------
// Pre-built binary tests (env var points directly at the binary)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_start_discovers_port_and_base_url() {
    let manifest = setup_manifest("BDD_TEST_START_PORT", env!("CARGO_BIN_EXE_test_service"));
    let config = empty_config();

    let mut handle = ServiceHandle::start(
        manifest.path().to_str().unwrap(),
        "test-service",
        config.path().to_str().unwrap(),
    )
    .await;

    assert!(handle.port > 0);
    assert_eq!(handle.base_url, format!("http://127.0.0.1:{}", handle.port));

    handle.stop().await;
}

#[tokio::test]
async fn test_is_running_reflects_process_state() {
    let manifest = setup_manifest("BDD_TEST_IS_RUNNING", env!("CARGO_BIN_EXE_test_service"));
    let config = empty_config();

    let mut handle = ServiceHandle::start(
        manifest.path().to_str().unwrap(),
        "test-service",
        config.path().to_str().unwrap(),
    )
    .await;

    assert!(handle.is_running());
    handle.stop().await;
    assert!(!handle.is_running());
}

#[tokio::test]
async fn test_stop_is_idempotent() {
    let manifest = setup_manifest("BDD_TEST_STOP_IDEM", env!("CARGO_BIN_EXE_test_service"));
    let config = empty_config();

    let mut handle = ServiceHandle::start(
        manifest.path().to_str().unwrap(),
        "test-service",
        config.path().to_str().unwrap(),
    )
    .await;

    handle.stop().await;
    // Second stop should not panic
    handle.stop().await;
    assert!(!handle.is_running());
}

#[tokio::test]
async fn test_try_start_succeeds_with_valid_binary() {
    let manifest = setup_manifest("BDD_TEST_TRY_START_OK", env!("CARGO_BIN_EXE_test_service"));
    let config = empty_config();

    let result = ServiceHandle::try_start(
        manifest.path().to_str().unwrap(),
        "test-service",
        config.path().to_str().unwrap(),
    )
    .await;

    let mut handle = result.unwrap();
    assert!(handle.port > 0);
    assert!(handle.is_running());
    handle.stop().await;
}

#[tokio::test]
async fn test_try_start_returns_error_when_binary_exits_without_binding() {
    let manifest = setup_manifest(
        "BDD_TEST_TRY_START_FAIL",
        env!("CARGO_BIN_EXE_test_service"),
    );
    let config = fail_config();

    let result = ServiceHandle::try_start(
        manifest.path().to_str().unwrap(),
        "test-service",
        config.path().to_str().unwrap(),
    )
    .await;

    let err = result.unwrap_err();
    assert!(
        err.contains("exited without binding"),
        "unexpected error: {}",
        err
    );
}

#[tokio::test]
async fn test_drop_cleans_up_process() {
    let manifest = setup_manifest("BDD_TEST_DROP_CLEANUP", env!("CARGO_BIN_EXE_test_service"));
    let config = empty_config();

    let handle = ServiceHandle::start(
        manifest.path().to_str().unwrap(),
        "test-service",
        config.path().to_str().unwrap(),
    )
    .await;

    let port = handle.port;

    // Drop the handle — should send SIGTERM
    drop(handle);

    // Give the process a moment to exit
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // The port should no longer be in use (connection should be refused)
    let result = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port)).await;
    assert!(result.is_err(), "port {} should be free after drop", port);
}

#[tokio::test]
async fn test_port_is_actually_listening() {
    let manifest = setup_manifest("BDD_TEST_PORT_LISTEN", env!("CARGO_BIN_EXE_test_service"));
    let config = empty_config();

    let mut handle = ServiceHandle::start(
        manifest.path().to_str().unwrap(),
        "test-service",
        config.path().to_str().unwrap(),
    )
    .await;

    // The test_service binary binds a TcpListener, so we should be able to connect
    let result = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", handle.port)).await;
    assert!(result.is_ok(), "should be able to connect to the service");

    handle.stop().await;
}

// ---------------------------------------------------------------------------
// Cargo run fallback tests (no env var set, exercises `cargo run --package`)
// ---------------------------------------------------------------------------

// Skip on Windows: `cargo run` recompiles test_service.exe which races with
// parallel tests that hold the binary open. The cargo-run fallback is
// OS-agnostic logic; Linux CI coverage is sufficient.
#[cfg(not(windows))]
#[tokio::test]
async fn test_start_via_cargo_run() {
    let manifest = setup_manifest_no_binary("BDD_TEST_CARGO_RUN");
    let config = empty_config();

    // No env var set and no binary in target dir for "bdd-infra",
    // so find_binary returns None → falls back to cargo run.
    // cargo run --package bdd-infra runs the test_service binary.
    let mut handle = ServiceHandle::start(
        manifest.path().to_str().unwrap(),
        "bdd-infra",
        config.path().to_str().unwrap(),
    )
    .await;

    assert!(handle.port > 0);
    assert!(handle.is_running());

    let result = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", handle.port)).await;
    assert!(result.is_ok(), "should be able to connect to the service");

    handle.stop().await;
    assert!(!handle.is_running());
}
