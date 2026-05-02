//! Integration tests for the supervision module's spawn-based arms.
//!
//! These tests live in a `[[test]]` integration target (not in
//! `src/supervision.rs`'s `#[cfg(test)] mod tests`) because they need to
//! spawn the in-tree `mock_astap` binary, and `CARGO_BIN_EXE_*` is only
//! set by Cargo for `[[test]]` crates and is unset under Bazel.
//!
//! Discovery order (matches BDD's `world.rs` pattern):
//! 1. `MOCK_ASTAP_BINARY` env var (set by the Bazel test target).
//! 2. `option_env!("CARGO_BIN_EXE_mock_astap")` (set by Cargo for this
//!    `[[test]]` target).
//!
//! If neither resolves, the tests skip with a diagnostic naming both
//! mechanisms.

use rp_plate_solver::supervision::{spawn_with_deadline, SpawnOutcome, GRACE_PERIOD};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::process::Command;

fn mock_astap_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MOCK_ASTAP_BINARY") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }
    if let Some(p) = option_env!("CARGO_BIN_EXE_mock_astap") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

fn require_mock_astap() -> PathBuf {
    mock_astap_path().unwrap_or_else(|| {
        panic!(
            "mock_astap binary not found.\n  \
             Tried: MOCK_ASTAP_BINARY env var, then CARGO_BIN_EXE_mock_astap.\n  \
             Under Cargo: run `cargo build --tests -p rp-plate-solver` first.\n  \
             Under Bazel: set MOCK_ASTAP_BINARY in the test target's env."
        )
    })
}

fn cmd_with_mode(mode: &str) -> Command {
    let bin = require_mock_astap();
    let mut cmd = Command::new(&bin);
    cmd.env("MOCK_ASTAP_MODE", mode);
    // Mock binary doesn't actually need -f for the modes we exercise here
    // (hang / ignore_sigterm), but pass one anyway so the argv shape is
    // representative of the real call.
    cmd.arg("-f").arg("/tmp/unused.fits");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x00000200); // CREATE_NEW_PROCESS_GROUP
    }
    cmd
}

#[tokio::test]
async fn exited_when_child_exits_within_deadline() {
    // `normal` mode would try to write a .wcs sidecar; we want a clean
    // quick exit instead. Use `no_wcs` mode (exits 0 immediately, no
    // side-effects).
    let cmd = cmd_with_mode("no_wcs");
    let outcome = spawn_with_deadline(cmd, Duration::from_secs(5))
        .await
        .unwrap();
    match outcome {
        SpawnOutcome::Exited { status, .. } => {
            assert!(status.success(), "expected zero exit, got {status}");
        }
        other => panic!("expected Exited, got {other:?}"),
    }
}

#[tokio::test]
async fn timed_out_terminated_when_child_responds_to_graceful_signal() {
    let cmd = cmd_with_mode("hang");
    let start = Instant::now();
    let outcome = spawn_with_deadline(cmd, Duration::from_millis(100))
        .await
        .unwrap();
    let elapsed = start.elapsed();
    match outcome {
        SpawnOutcome::TimedOutTerminated => {}
        other => panic!("expected TimedOutTerminated, got {other:?}"),
    }
    // Should have terminated well within the grace period of being
    // signaled — assert the total wall time is bounded.
    assert!(
        elapsed < Duration::from_millis(100) + GRACE_PERIOD,
        "supervision took longer than deadline + grace: {elapsed:?}"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn timed_out_killed_when_child_ignores_graceful_signal() {
    // The Windows mock currently uses SetConsoleCtrlHandler returning TRUE
    // to swallow CTRL_BREAK_EVENT, but tokio's force-kill on Windows
    // (TerminateProcess) bypasses that handler too — semantics are the
    // same. Gating to Unix here avoids spurious flakiness on Windows
    // CI runners with quirky console-attach behavior; the contract
    // assertion holds on both platforms by design.
    let cmd = cmd_with_mode("ignore_sigterm");
    let start = Instant::now();
    let outcome = spawn_with_deadline(cmd, Duration::from_millis(100))
        .await
        .unwrap();
    let elapsed = start.elapsed();
    match outcome {
        SpawnOutcome::TimedOutKilled => {}
        other => panic!("expected TimedOutKilled, got {other:?}"),
    }
    // Total time = deadline (100ms) + grace (2s) + force-kill latency.
    // Bound generously so this isn't flaky on slow CI.
    assert!(
        elapsed >= Duration::from_millis(100) + GRACE_PERIOD,
        "force-kill should have waited at least the full grace period: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(100) + GRACE_PERIOD + Duration::from_secs(2),
        "supervision took unreasonably long: {elapsed:?}"
    );
}
