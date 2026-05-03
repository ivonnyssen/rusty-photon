//! Step definitions for `subprocess_supervision.feature`.

use crate::world::{ConcurrentResult, PlateSolverWorld};
use cucumber::{then, when};
use std::time::{Duration, Instant};

#[when(expr = "I POST two concurrent solve requests with timeout {string} each")]
async fn when_two_concurrent_solves(world: &mut PlateSolverWorld, timeout: String) {
    // Make sure the wrapper is up before launching parallel requests.
    if world.service_handle.is_none() {
        world.start_wrapper_with_mock().await;
    }
    let url = format!("{}/api/v1/solve", world.wrapper_url());
    let fits = world.fits_path.as_ref().expect("fits_path").clone();
    let body = serde_json::json!({ "fits_path": fits, "timeout": timeout });

    let client = reqwest::Client::new();
    let make_request = || {
        let url = url.clone();
        let body = body.clone();
        let client = client.clone();
        async move {
            let resp = client.post(&url).json(&body).send().await.expect("POST");
            ConcurrentResult {
                status: resp.status().as_u16(),
                completed_at: Instant::now(),
            }
        }
    };

    let (a, b) = tokio::join!(make_request(), make_request());
    world.concurrent_results = vec![a, b];
}

#[then("both responses have status 504")]
async fn then_both_responses_504(world: &mut PlateSolverWorld) {
    assert_eq!(world.concurrent_results.len(), 2);
    for r in &world.concurrent_results {
        assert_eq!(r.status, 504, "expected both 504, got {r:?}");
    }
}

#[then("the second request's spawn time is after the first request's exit time")]
async fn then_second_after_first(world: &mut PlateSolverWorld) {
    // The two HTTP requests are launched ~simultaneously by the test
    // — `started_at` is the test's send-time, not when the wrapper
    // started processing. The semaphore's serialization is observable
    // instead via the gap between completion times: a serialized
    // pair completes ~per_request_time apart, while a parallel pair
    // completes within a few ms.
    //
    // With `mock_astap=hang` and timeout=100ms, each request takes
    // ~100ms wall-clock (timeout fires, mock_astap exits on SIGTERM
    // promptly). Serialized → ~100ms gap; parallel → near 0.
    let mut sorted = world.concurrent_results.clone();
    sorted.sort_by_key(|r| r.completed_at);
    let first = &sorted[0];
    let second = &sorted[1];
    let gap = second.completed_at.duration_since(first.completed_at);
    assert!(
        gap >= Duration::from_millis(50),
        "single-flight failed: completion gap {gap:?} too small (parallel?)"
    );
}

#[then(expr = "the response time is at most {int} milliseconds")]
async fn then_response_time_at_most(world: &mut PlateSolverWorld, ms: u64) {
    let elapsed = world.last_response_elapsed.expect("no elapsed");
    assert!(
        elapsed <= Duration::from_millis(ms),
        "elapsed {elapsed:?} > {ms}ms"
    );
}

#[then(expr = "the response time is at least {int} milliseconds")]
async fn then_response_time_at_least(world: &mut PlateSolverWorld, ms: u64) {
    let elapsed = world.last_response_elapsed.expect("no elapsed");
    assert!(
        elapsed >= Duration::from_millis(ms),
        "elapsed {elapsed:?} < {ms}ms"
    );
}
