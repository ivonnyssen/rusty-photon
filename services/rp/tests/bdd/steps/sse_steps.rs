//! Steps for the real-time SSE event stream (`event_subscribe.feature`).
//!
//! Drives rp's `GET /api/events/subscribe` via
//! `bdd_infra::rp_harness::SseClient` and asserts on the parsed frames. The
//! mount setup, slew, and park triggers are reused from `mount_steps` /
//! `operation_event_steps`.

use bdd_infra::rp_harness::SseClient;
use cucumber::{given, then, when};

use crate::world::RpWorld;

#[given("the operator is subscribed to the event stream")]
#[when("the operator subscribes to the event stream")]
async fn subscribe(world: &mut RpWorld) {
    let base = world.rp_url();
    world.sse_client = Some(SseClient::connect(&base, None).await);
}

#[when("the operator disconnects from the event stream")]
async fn disconnect(world: &mut RpWorld) {
    let cursor = world
        .sse_client
        .as_ref()
        .expect("no active SSE subscription to disconnect")
        .max_event_seq()
        .await;
    // Remember the highest event_seq seen so the reconnect resumes after it.
    world.sse_reconnect_cursor = cursor;
    // Dropping the client aborts its reader task and closes the connection.
    world.sse_client = None;
}

#[when("the operator reconnects to the event stream from the last received event id")]
async fn reconnect(world: &mut RpWorld) {
    let base = world.rp_url();
    let cursor = world.sse_reconnect_cursor;
    world.sse_client = Some(SseClient::connect(&base, cursor).await);
}

#[when(expr = "the event stream delivers the {string} event")]
#[then(expr = "the event stream delivers the {string} event")]
async fn stream_delivers(world: &mut RpWorld, event_type: String) {
    let client = world
        .sse_client
        .as_ref()
        .expect("no active SSE subscription");
    if client.wait_for_event(&event_type).await.is_none() {
        let seen: Vec<_> = client
            .frames()
            .await
            .iter()
            .filter_map(|f| f.event_type())
            .collect();
        panic!("expected the SSE stream to deliver a '{event_type}' frame; saw: {seen:?}");
    }
}

#[then(expr = "the {string} stream frame's SSE id equals its event_seq")]
async fn frame_id_equals_event_seq(world: &mut RpWorld, event_type: String) {
    let frame = world
        .sse_client
        .as_ref()
        .expect("no active SSE subscription")
        .wait_for_event(&event_type)
        .await
        .unwrap_or_else(|| panic!("no '{event_type}' frame on the stream"));
    let sse_id = frame
        .id
        .unwrap_or_else(|| panic!("'{event_type}' frame carried no SSE id"));
    let envelope_seq = frame
        .json()
        .get("event_seq")
        .and_then(|v| v.as_u64())
        .unwrap_or_else(|| panic!("'{event_type}' data carried no event_seq"));
    assert_eq!(
        sse_id, envelope_seq,
        "the SSE id must equal the envelope's event_seq (the Last-Event-ID replay key)"
    );
}

#[then(expr = "the {string} and {string} stream frames share one operation_id")]
async fn frames_share_operation_id(world: &mut RpWorld, first: String, second: String) {
    let client = world
        .sse_client
        .as_ref()
        .expect("no active SSE subscription");
    let a = client
        .wait_for_event(&first)
        .await
        .unwrap_or_else(|| panic!("no '{first}' frame on the stream"));
    let b = client
        .wait_for_event(&second)
        .await
        .unwrap_or_else(|| panic!("no '{second}' frame on the stream"));
    let a_op = a
        .operation_id()
        .unwrap_or_else(|| panic!("'{first}' frame carried no operation_id"));
    let b_op = b
        .operation_id()
        .unwrap_or_else(|| panic!("'{second}' frame carried no operation_id"));
    assert_eq!(
        a_op, b_op,
        "'{first}' and '{second}' must share one operation_id"
    );
}
