//! BDD step definitions for the trigger contract (design § Triggers): the
//! safe-point interleaving, the `once`/`cooldown` gates, and poll sources,
//! observed through rp's SSE stream — every fixture's trigger action is a
//! `set_filter`, so each firing leaves a `filter_switch` frame whose
//! stream sequence number proves ordering against the exposure events.

use std::time::Duration;

use bdd_infra::rp_harness::SseClient;
use cucumber::{given, then};

use crate::world::SessionRunnerWorld;

#[given("an SSE client is watching rp's event stream")]
async fn sse_client_watching(world: &mut SessionRunnerWorld) {
    world.sse_client = Some(SseClient::connect(&world.rp_url(), None).await);
}

#[then(expr = "the SSE stream should show exactly {int} {string} event(s)")]
async fn sse_shows_exactly_n_events(
    world: &mut SessionRunnerWorld,
    expected: usize,
    event_type: String,
) {
    let client = world
        .sse_client
        .as_ref()
        .expect("no SSE client — add the 'an SSE client is watching' step");

    // The reader task consumes the stream asynchronously, so give the
    // expected frames a moment to land…
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let count = count_events(client, &event_type).await;
        if count >= expected || std::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // …then settle briefly so an over-firing straggler would be caught by
    // the exact-count assertion rather than sneak in after it.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let count = count_events(client, &event_type).await;
    assert_eq!(
        count, expected,
        "expected exactly {expected} '{event_type}' event(s) on the SSE stream, saw {count}"
    );
}

async fn count_events(client: &SseClient, event_type: &str) -> usize {
    client
        .frames()
        .await
        .iter()
        .filter(|f| f.event_type().as_deref() == Some(event_type))
        .count()
}

#[then(expr = "no {string} event should fall between an {string} and its {string}")]
async fn no_event_inside_spans(
    world: &mut SessionRunnerWorld,
    needle: String,
    span_start: String,
    span_end: String,
) {
    let client = world
        .sse_client
        .as_ref()
        .expect("no SSE client — add the 'an SSE client is watching' step");
    let frames = client.frames().await;

    // Pair start/end frames by operation_id — the shared id rp stamps on
    // an operation's started/complete envelopes.
    let mut spans = Vec::new();
    for start in frames
        .iter()
        .filter(|f| f.event_type().as_deref() == Some(&span_start))
    {
        let operation = start
            .operation_id()
            .unwrap_or_else(|| panic!("a '{span_start}' frame carries no operation_id"));
        let end = frames
            .iter()
            .find(|f| {
                f.event_type().as_deref() == Some(&span_end)
                    && f.operation_id().as_deref() == Some(&operation)
            })
            .unwrap_or_else(|| {
                panic!("operation {operation} has a '{span_start}' but no '{span_end}'")
            });
        spans.push((seq_of(start, &span_start), seq_of(end, &span_end)));
    }
    assert!(
        !spans.is_empty(),
        "expected at least one '{span_start}'/'{span_end}' span on the SSE stream"
    );

    let needles: Vec<u64> = frames
        .iter()
        .filter(|f| f.event_type().as_deref() == Some(&needle))
        .map(|f| seq_of(f, &needle))
        .collect();
    assert!(
        !needles.is_empty(),
        "expected at least one '{needle}' event to check placement for"
    );
    for seq in needles {
        for (start, end) in &spans {
            assert!(
                !(*start < seq && seq < *end),
                "'{needle}' at seq {seq} fell inside the '{span_start}'/'{span_end}' span \
                 [{start}, {end}] — a trigger action ran during an in-flight instruction"
            );
        }
    }
}

fn seq_of(frame: &bdd_infra::rp_harness::SseFrame, what: &str) -> u64 {
    frame
        .id
        .unwrap_or_else(|| panic!("a '{what}' frame carries no stream sequence id"))
}
