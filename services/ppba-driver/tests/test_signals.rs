//! End-to-end tests for ppba-driver's `shutdown_signal()`.
//!
//! Spawns the real binary, waits for `bound_addr=...` on stdout, then sends
//! a signal and asserts the process exits cleanly within the grace period.
//! Exercises both the `SIGTERM` and `SIGINT` arms of the `select!` in
//! `shutdown_signal()`, which no other test covers directly.
//!
//! Unix-only: on Windows the services only wire up `ctrl_c`, and signalling
//! a single child process without disrupting the test harness is not possible
//! via `CTRL_C_EVENT` / `CTRL_BREAK_EVENT` without a shared console setup.

#![cfg(all(unix, feature = "mock"))]

use std::io::Write;
use std::time::Duration;

use bdd_infra::{send_sigint, send_sigterm, ServiceHandle};

fn write_config(path: &std::path::Path, suffix: &str) {
    let config = serde_json::json!({
        "serial": { "port": "/dev/null" },
        "server": { "port": 0, "discovery_port": null },
        "switch": {
            "name": "Test Switch",
            "unique_id": format!("test-switch-{}", suffix),
            "description": "Test switch"
        },
        "observingconditions": {
            "name": "Test Weather",
            "unique_id": format!("test-weather-{}", suffix),
            "description": "Test weather",
            "enabled": false
        }
    });
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(config.to_string().as_bytes()).unwrap();
}

async fn spawn_and_signal<F: FnOnce(u32)>(suffix: &str, signal_fn: F) {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    write_config(&config_path, suffix);

    let mut handle = ServiceHandle::try_start(
        env!("CARGO_MANIFEST_DIR"),
        env!("CARGO_PKG_NAME"),
        config_path.to_str().unwrap(),
    )
    .await
    .expect("service failed to start");

    // `bound_addr=` is printed inside `ServerBuilder::build()`, before
    // `run_server_loop` enters its `tokio::select!`. The signal handlers live
    // inside the `stop()` closure and are only installed the first time that
    // branch is polled. Without a brief pause here, the signal can arrive
    // during the gap between the stdout flush and the select! entry, where the
    // default handler terminates the process.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let pid = handle.pid().expect("child has no pid");
    signal_fn(pid);

    let status = handle
        .wait_for_exit(Duration::from_secs(5))
        .await
        .expect("service did not exit gracefully");
    assert!(status.success(), "expected clean exit, got {:?}", status);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_sigterm_shuts_down_gracefully() {
    spawn_and_signal("sigterm", send_sigterm).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_sigint_shuts_down_gracefully() {
    spawn_and_signal("sigint", send_sigint).await;
}
