//! Steps for park.feature.

use crate::world::StarAdventurerWorld;
use cucumber::{then, when};
use std::time::Duration;

#[when("I park the mount")]
async fn park_mount(world: &mut StarAdventurerWorld) {
    world.mount().park().await.unwrap();
}

#[when("I try to park the mount")]
async fn try_park_mount(world: &mut StarAdventurerWorld) {
    match world.mount().park().await {
        Ok(()) => world.clear_error(),
        Err(e) => world.record_error(e),
    }
}

#[when("I unpark the mount")]
async fn unpark_mount(world: &mut StarAdventurerWorld) {
    world.mount().unpark().await.unwrap();
}

#[when("I try to set the park position")]
async fn try_set_park_position(world: &mut StarAdventurerWorld) {
    match world.mount().set_park().await {
        Ok(()) => world.clear_error(),
        Err(e) => world.record_error(e),
    }
}

#[when("the mount reports both axes stopped at encoder 0")]
async fn mount_reports_axes_stopped_at_zero(world: &mut StarAdventurerWorld) {
    // Push a one-shot seed via the debug endpoint — the mock simulator
    // already advances to encoder 0 once park's :S1<0>/:S2<0> arrive,
    // but tightly-timed scenarios may race the watcher's first poll.
    // Force the state to the post-slew shape so the assertion is
    // deterministic.
    let mut seed = serde_json::Map::new();
    seed.insert("ra_ticks".into(), 0.into());
    seed.insert("dec_ticks".into(), 0.into());
    seed.insert("ra_running".into(), false.into());
    seed.insert("dec_running".into(), false.into());
    let body = serde_json::Value::Object(seed);
    let url = format!(
        "http://127.0.0.1:{}/debug/v1/mock-state",
        world.service_handle.as_ref().expect("service started").port
    );
    let _ = reqwest::Client::new().post(&url).json(&body).send().await;
}

#[then("AtPark should be false")]
async fn at_park_false(world: &mut StarAdventurerWorld) {
    assert!(!world.mount().at_park().await.unwrap());
}

#[then(expr = "AtPark should eventually be true within {int} seconds")]
async fn at_park_eventually_true(world: &mut StarAdventurerWorld, secs: u64) {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        if world.mount().at_park().await.unwrap() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("AtPark did not become true within {secs} seconds");
}

#[then("the mount should have received command :K1 before any :S1")]
async fn k1_before_s1(world: &mut StarAdventurerWorld) {
    let log = world.command_log().await;
    let first_k1 = log.iter().position(|c| c == ":K1\r");
    let first_s1 = log.iter().position(|c| c.starts_with(":S1"));
    match (first_k1, first_s1) {
        (Some(k), Some(s)) => assert!(
            k < s,
            ":K1 (index {k}) must precede first :S1 (index {s}); log {log:?}"
        ),
        (Some(_), None) => {
            // No :S1 yet — the park sequence may still be in flight.
            // Treat absence of :S1 as a pass; later assertions cover
            // its arrival.
        }
        _ => panic!(":K1 not seen in log {log:?}"),
    }
}

#[then("the mount should have received a :S1 command targeting encoder 0")]
async fn s1_targeting_zero(world: &mut StarAdventurerWorld) {
    // `:S1<6-byte-bias-encoded-i32>\r`. Encoder 0 = bias 0x800000 →
    // low-byte-first hex "000080".
    let log = world.command_log().await;
    let want = ":S1000080\r";
    assert!(
        log.iter().any(|c| c == want),
        "expected {want:?} in log {log:?}"
    );
}

#[then("the mount should have received a :S2 command targeting encoder 0")]
async fn s2_targeting_zero(world: &mut StarAdventurerWorld) {
    let log = world.command_log().await;
    let want = ":S2000080\r";
    assert!(
        log.iter().any(|c| c == want),
        "expected {want:?} in log {log:?}"
    );
}

#[then("the mount should not have received a second :S1 command")]
async fn no_second_s1(world: &mut StarAdventurerWorld) {
    let log = world.command_log().await;
    let s1_count = log.iter().filter(|c| c.starts_with(":S1")).count();
    assert!(s1_count <= 1, ":S1 issued {s1_count} times; log {log:?}");
}
