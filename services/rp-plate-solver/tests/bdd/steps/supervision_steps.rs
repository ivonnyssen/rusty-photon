//! Step definitions for `subprocess_supervision.feature`.
//!
//! Phase 3 stubs. Bodies arrive in Phase 4.

use crate::world::PlateSolverWorld;
use cucumber::{then, when};

#[when(expr = "I POST two concurrent solve requests with timeout {string} each")]
async fn when_two_concurrent_solves(_world: &mut PlateSolverWorld, _timeout: String) {
    todo!("Phase 4: spawn two solve tasks via tokio::join!; capture both responses + timing into world")
}

#[then("both responses have status 504")]
async fn then_both_responses_504(_world: &mut PlateSolverWorld) {
    todo!("Phase 4")
}

#[then("the second request's spawn time is after the first request's exit time")]
async fn then_second_after_first(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: read MOCK_ASTAP_ARGV_OUT timestamps from each spawn; assert ordering")
}

#[then(expr = "the response time is at most {int} milliseconds")]
async fn then_response_time_at_most(_world: &mut PlateSolverWorld, _ms: u64) {
    todo!("Phase 4")
}

#[then(expr = "the response time is at least {int} milliseconds")]
async fn then_response_time_at_least(_world: &mut PlateSolverWorld, _ms: u64) {
    todo!("Phase 4")
}
