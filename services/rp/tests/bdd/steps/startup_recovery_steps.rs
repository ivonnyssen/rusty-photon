//! BDD step definitions for session recovery across rp restarts
//! (rp.md § Session Persistence / § Recovery Behavior): pinning the
//! session state file across an rp respawn, crashing rp mid-session,
//! and asserting what the restarted process does — and does not —
//! re-invoke.

use cucumber::{given, then, when};

use crate::steps::tool_steps::start_rp;
use crate::world::RpWorld;

#[given("rp's session state file is pinned to a fresh path")]
fn pin_session_state_file(world: &mut RpWorld) {
    let dir =
        tempfile::tempdir().expect("failed to create tempdir for the pinned session state file");
    world.pinned_session_state_file = Some(
        dir.path()
            .join("session_state.json")
            .to_string_lossy()
            .into_owned(),
    );
    world.pinned_session_state_holder = Some(dir);
}

#[when("rp is killed")]
async fn rp_is_killed(world: &mut RpWorld) {
    // Drop the clients bound to the dying process first — they cannot
    // survive the port change across the respawn anyway.
    world.mcp_client = None;
    world.sse_client = None;
    world.rp.as_mut().expect("rp is not running").kill().await;
    world.rp = None;
}

#[when("rp is restarted after the crash")]
async fn rp_is_restarted_after_crash(world: &mut RpWorld) {
    assert!(
        world.rp.is_none(),
        "rp is still running — kill it before restarting"
    );
    start_rp(world).await;
}

#[then(expr = "the test orchestrator should have been invoked exactly {int} time(s)")]
async fn orchestrator_invoked_exactly(world: &mut RpWorld, expected: usize) {
    // Settle first: a buggy startup recovery would re-invoke within the
    // invoke retry budget (3 attempts, 1 s apart) — give any such extra
    // invocation time to land before counting.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let invocations = world.orchestrator_invocations.read().await;
    assert_eq!(
        invocations.len(),
        expected,
        "expected exactly {expected} orchestrator invocation(s), got {}",
        invocations.len()
    );
}
