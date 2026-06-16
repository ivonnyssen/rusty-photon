//! Steps for the operation watchdog (`operation_watchdog.feature`).
//!
//! These drive the real sentinel binary against a controllable SSE stub
//! ([`crate::world::RpEventStub`]) standing in for rp's
//! `GET /api/events/subscribe`, then assert on the watchdog escalations that
//! land in sentinel's notification history.

use cucumber::{given, then};

use crate::world::SentinelWorld;

const OP_ID: &str = "op-bdd-1";
const WATCHDOG_NAME: &str = "Operation Watchdog";

/// An SSE `slew_started` frame carrying a `max_duration_ms` deadline.
fn started_frame(max_ms: u64) -> String {
    format!(
        "event: slew_started\nid: 1\ndata: {{\"event_seq\":1,\"event\":\"slew_started\",\"operation_id\":\"{OP_ID}\",\"max_duration_ms\":{max_ms}}}"
    )
}

/// The matching `slew_complete` frame.
fn complete_frame() -> String {
    format!(
        "event: slew_complete\nid: 2\ndata: {{\"event_seq\":2,\"event\":\"slew_complete\",\"operation_id\":\"{OP_ID}\"}}"
    )
}

#[given("rp is streaming a slew operation that completes within its deadline")]
async fn slew_completes_in_time(world: &mut SentinelWorld) {
    world.sentinel_has_notifiers = true;
    // 500 ms deadline, but the completion arrives immediately after the start.
    world
        .start_rp_event_stub(vec![started_frame(500), complete_frame()])
        .await;
}

#[given("rp is streaming a slew operation that never completes")]
async fn slew_never_completes(world: &mut SentinelWorld) {
    world.sentinel_has_notifiers = true;
    // 500 ms deadline with no completion -> the watchdog must escalate.
    world.start_rp_event_stub(vec![started_frame(500)]).await;
}

#[given("rp's event stream is unreachable")]
async fn rp_unreachable(world: &mut SentinelWorld) {
    world.sentinel_has_notifiers = true;
    // Port 1 is reserved and refuses connections, so every reconnect fails.
    world.watchdog_rp_url = Some("http://127.0.0.1:1".to_string());
}

#[then(expr = "the watchdog records an escalation mentioning {string}")]
async fn watchdog_records_escalation(world: &mut SentinelWorld, needle: String) {
    let history = world
        .wait_for_history(|record| {
            record["monitor_name"].as_str() == Some(WATCHDOG_NAME)
                && record["message"]
                    .as_str()
                    .is_some_and(|m| m.contains(&needle))
        })
        .await;
    assert!(
        history.iter().any(|record| {
            record["monitor_name"].as_str() == Some(WATCHDOG_NAME)
                && record["message"]
                    .as_str()
                    .is_some_and(|m| m.contains(&needle))
        }),
        "expected a '{WATCHDOG_NAME}' escalation mentioning '{needle}'; history: {history:#?}"
    );
}

#[then("the watchdog records no escalation")]
async fn watchdog_records_no_escalation(world: &mut SentinelWorld) {
    // The deadline is 500 ms; wait well past it so an erroneous escalation
    // would have landed, then assert none did.
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    let history = world.get_history().await;
    let escalations: Vec<_> = history
        .iter()
        .filter(|record| record["monitor_name"].as_str() == Some(WATCHDOG_NAME))
        .collect();
    assert!(
        escalations.is_empty(),
        "a completed operation must not escalate; got: {escalations:#?}"
    );
}
