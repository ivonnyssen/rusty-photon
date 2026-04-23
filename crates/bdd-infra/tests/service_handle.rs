//! Integration tests for ServiceHandle using the test_service binary.

use bdd_infra::ServiceHandle;
use std::sync::Once;

/// Point `TEST_SERVICE_BINARY` at the test_service binary exactly once.
///
/// Under Cargo, `option_env!("CARGO_BIN_EXE_test_service")` resolves at
/// compile time to the path of the test_service binary that Cargo builds
/// alongside these integration tests. Under Bazel it may not be defined at
/// compile time (depending on rules_rust's data-dep propagation) — that's
/// why we use `option_env!` rather than `env!`. Bazel instead sets
/// `TEST_SERVICE_BINARY` to the runfiles path of `:test_service` via the
/// `rust_test.env` attribute, so the init is a no-op there.
/// `ServiceHandle::start("test-service", …)` derives `TEST_SERVICE_BINARY`
/// from the package name in both cases.
fn init_test_binary_env() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        if std::env::var_os("TEST_SERVICE_BINARY").is_none() {
            if let Some(path) = option_env!("CARGO_BIN_EXE_test_service") {
                std::env::set_var("TEST_SERVICE_BINARY", path);
            }
        }
    });
}

fn empty_config() -> tempfile::NamedTempFile {
    tempfile::NamedTempFile::new().unwrap()
}

/// Write a config file containing "fail" to trigger test_service exit.
fn fail_config() -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), "fail").unwrap();
    file
}

#[tokio::test]
async fn test_start_discovers_port_and_base_url() {
    init_test_binary_env();
    let config = empty_config();

    let mut handle = ServiceHandle::start("test-service", config.path().to_str().unwrap()).await;

    assert!(handle.port > 0);
    assert_eq!(handle.base_url, format!("http://127.0.0.1:{}", handle.port));

    handle.stop().await;
}

#[tokio::test]
async fn test_is_running_reflects_process_state() {
    init_test_binary_env();
    let config = empty_config();

    let mut handle = ServiceHandle::start("test-service", config.path().to_str().unwrap()).await;

    assert!(handle.is_running());
    handle.stop().await;
    assert!(!handle.is_running());
}

#[tokio::test]
async fn test_stop_is_idempotent() {
    init_test_binary_env();
    let config = empty_config();

    let mut handle = ServiceHandle::start("test-service", config.path().to_str().unwrap()).await;

    handle.stop().await;
    // Second stop should not panic
    handle.stop().await;
    assert!(!handle.is_running());
}

#[tokio::test]
async fn test_try_start_succeeds_with_valid_binary() {
    init_test_binary_env();
    let config = empty_config();

    let result = ServiceHandle::try_start("test-service", config.path().to_str().unwrap()).await;

    let mut handle = result.unwrap();
    assert!(handle.port > 0);
    assert!(handle.is_running());
    handle.stop().await;
}

#[tokio::test]
async fn test_try_start_returns_error_when_binary_exits_without_binding() {
    init_test_binary_env();
    let config = fail_config();

    let result = ServiceHandle::try_start("test-service", config.path().to_str().unwrap()).await;

    let err = result.unwrap_err();
    assert!(
        err.contains("exited without binding"),
        "unexpected error: {}",
        err
    );
}

#[tokio::test]
async fn test_drop_cleans_up_process() {
    init_test_binary_env();
    let config = empty_config();

    let handle = ServiceHandle::start("test-service", config.path().to_str().unwrap()).await;

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
    init_test_binary_env();
    let config = empty_config();

    let mut handle = ServiceHandle::start("test-service", config.path().to_str().unwrap()).await;

    // The test_service binary binds a TcpListener, so we should be able to connect
    let result = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", handle.port)).await;
    assert!(result.is_ok(), "should be able to connect to the service");

    handle.stop().await;
}

/// Graceful shutdown must complete well under the 5-second SIGKILL fallback
/// timeout. If the shutdown signal is not delivered (e.g. a no-op
/// `send_sigterm`), `stop()` would wait the full 5 seconds before hard-killing.
/// A 2-second budget catches such regressions on any platform while leaving
/// generous margin for slow CI runners.
#[tokio::test]
async fn test_stop_completes_via_graceful_signal() {
    init_test_binary_env();
    let config = empty_config();

    let mut handle = ServiceHandle::start("test-service", config.path().to_str().unwrap()).await;

    let start = std::time::Instant::now();
    handle.stop().await;
    let elapsed = start.elapsed();

    assert!(
        elapsed < std::time::Duration::from_millis(2000),
        "stop() took {:?}, expected < 2s — graceful shutdown signal may not be working",
        elapsed
    );
}
