//! Subprocess supervision: spawn under a wall-clock deadline; on expiry,
//! escalate from a graceful signal to a force-kill.
//!
//! The graceful signal is `SIGTERM` on Unix and `CTRL_BREAK_EVENT` on
//! Windows. The Windows path requires the child to have been spawned with
//! `CREATE_NEW_PROCESS_GROUP` so the event reaches only the child's group;
//! see `runner/astap.rs::AstapCliRunner::build_command` and the bdd-infra
//! pattern this mirrors (`crates/bdd-infra/src/lib.rs` `send_sigterm`).

use std::process::ExitStatus;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// Grace period between graceful signal and force-kill. Fixed constant —
/// tuned to dominate signal-handling latency the child might exhibit while
/// staying short enough that a wedged child doesn't tie up the
/// single-flight semaphore.
pub const GRACE_PERIOD: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum SpawnOutcome {
    /// Child exited on its own within the deadline.
    Exited {
        status: ExitStatus,
        stderr_tail: String,
    },
    /// Deadline expired; child responded to the graceful signal within
    /// the grace period.
    TimedOutTerminated,
    /// Deadline expired; child ignored the graceful signal and was
    /// force-killed after the grace period.
    TimedOutKilled,
}

/// Spawn the command and race its exit against a wall-clock deadline.
///
/// On deadline expiry: send graceful signal → wait `GRACE_PERIOD` → force
/// kill. Always `wait()`s for the child fully before returning, so the
/// caller can rely on no orphaned child processes per the design contract.
pub async fn spawn_with_deadline(
    mut cmd: Command,
    deadline: Duration,
) -> std::io::Result<SpawnOutcome> {
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn()?;
    let pid = child
        .id()
        .ok_or_else(|| std::io::Error::other("spawned child has no PID"))?;

    // Take stderr now so we can collect its tail regardless of which arm
    // of the race fires.
    let stderr = child.stderr.take();

    tokio::select! {
        biased;
        result = child.wait() => {
            let status = result?;
            let stderr_tail = collect_stderr_tail(stderr).await;
            Ok(SpawnOutcome::Exited { status, stderr_tail })
        }
        _ = tokio::time::sleep(deadline) => {
            // Deadline. Send graceful signal, wait grace period, escalate.
            send_graceful(pid);
            match tokio::time::timeout(GRACE_PERIOD, child.wait()).await {
                Ok(_status) => Ok(SpawnOutcome::TimedOutTerminated),
                Err(_) => {
                    // Force-kill. tokio's Child::kill sends SIGKILL on Unix
                    // and TerminateProcess on Windows.
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    Ok(SpawnOutcome::TimedOutKilled)
                }
            }
        }
    }
}

/// Send the platform's graceful-shutdown signal to a process. Best-effort:
/// signal failures log via `tracing::debug!` and do not propagate, so a
/// caller's deadline path is not derailed by signal-delivery transients.
fn send_graceful(pid: u32) {
    #[cfg(unix)]
    {
        // SAFETY: libc::kill with a valid pid and SIGTERM is safe. This is
        // the same pattern bdd-infra uses; see send_sigterm there.
        let ret = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        if ret != 0 {
            tracing::debug!(
                "supervision: failed to send SIGTERM to pid {pid}: {}",
                std::io::Error::last_os_error()
            );
        }
    }
    #[cfg(windows)]
    {
        // SAFETY: GenerateConsoleCtrlEvent with CTRL_BREAK_EVENT and a
        // valid process-group id is the documented graceful-shutdown
        // signal for a console process on Windows. The child must have
        // been spawned with CREATE_NEW_PROCESS_GROUP for this to target
        // only its group.
        #[allow(non_snake_case)]
        extern "system" {
            fn GenerateConsoleCtrlEvent(dw_ctrl_event: u32, dw_process_group_id: u32) -> i32;
        }
        const CTRL_BREAK_EVENT: u32 = 1;
        let ret = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) };
        if ret == 0 {
            tracing::debug!(
                "supervision: failed to send CTRL_BREAK_EVENT to process group {pid}: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

/// Read up to ~4 KiB of the child's stderr. Returns an empty string if no
/// stderr was captured or the read failed.
async fn collect_stderr_tail(stderr: Option<tokio::process::ChildStderr>) -> String {
    let Some(mut s) = stderr else {
        return String::new();
    };
    let mut buf = Vec::with_capacity(4096);
    let _ = s.read_to_end(&mut buf).await;
    let text = String::from_utf8_lossy(&buf).to_string();
    // Truncate to last 4096 bytes if very long.
    if text.len() > 4096 {
        text[text.len() - 4096..].to_string()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grace_period_is_two_seconds() {
        // The constant is part of the public supervision contract — the
        // design doc and plan both name 2s. This test exists so the
        // constant doesn't drift silently.
        assert_eq!(GRACE_PERIOD, Duration::from_secs(2));
    }

    #[tokio::test]
    async fn outcome_variants_are_constructible() {
        // Smoke test that the SpawnOutcome variants compile and can be
        // matched without spawning a real subprocess (those tests live in
        // tests/supervision_integration.rs; see plan §Phase 2).
        let exited = SpawnOutcome::Exited {
            status: std::process::Command::new("true")
                .status()
                .unwrap_or_else(|_| {
                    std::process::Command::new("cmd")
                        .args(["/C", "exit", "0"])
                        .status()
                        .expect("a no-op exit-0 command must work")
                }),
            stderr_tail: String::new(),
        };
        match exited {
            SpawnOutcome::Exited { .. } => {}
            _ => panic!("wrong variant"),
        }

        let term = SpawnOutcome::TimedOutTerminated;
        let kill = SpawnOutcome::TimedOutKilled;
        match (term, kill) {
            (SpawnOutcome::TimedOutTerminated, SpawnOutcome::TimedOutKilled) => {}
            _ => panic!("wrong variants"),
        }
    }
}
