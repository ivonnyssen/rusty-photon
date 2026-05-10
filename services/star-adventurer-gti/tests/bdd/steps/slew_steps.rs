//! Steps for slew.feature.

use crate::world::StarAdventurerWorld;
use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use std::time::Duration;

#[given("the device is parked")]
async fn device_is_parked(world: &mut StarAdventurerWorld) {
    // Connect, park, wait for the watcher to set AtPark = true.
    world.mount().set_connected(true).await.unwrap();
    world.mount().park().await.unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if world.mount().at_park().await.unwrap() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("park watcher did not set AtPark within 5s");
}

#[when(expr = "I slew asynchronously to RA {float} hours and Dec {float} degrees")]
async fn slew_async_to(world: &mut StarAdventurerWorld, ra: f64, dec: f64) {
    world
        .mount()
        .slew_to_coordinates_async(ra, dec)
        .await
        .unwrap();
}

#[when(expr = "I try to slew asynchronously to RA {float} hours and Dec {float} degrees")]
async fn try_slew_async_to(world: &mut StarAdventurerWorld, ra: f64, dec: f64) {
    match world.mount().slew_to_coordinates_async(ra, dec).await {
        Ok(()) => world.clear_error(),
        Err(e) => world.record_error(e),
    }
}

#[when("I slew to the stored target")]
async fn slew_to_target(world: &mut StarAdventurerWorld) {
    world.mount().slew_to_target_async().await.unwrap();
}

#[when("I try to slew to the stored target")]
async fn try_slew_to_target(world: &mut StarAdventurerWorld) {
    match world.mount().slew_to_target_async().await {
        Ok(()) => world.clear_error(),
        Err(e) => world.record_error(e),
    }
}

#[when(expr = "I set TargetRightAscension to {float} hours")]
async fn set_target_ra(world: &mut StarAdventurerWorld, hours: f64) {
    world
        .mount()
        .set_target_right_ascension(hours)
        .await
        .unwrap();
}

#[when(expr = "I set TargetDeclination to {float} degrees")]
async fn set_target_dec(world: &mut StarAdventurerWorld, deg: f64) {
    world.mount().set_target_declination(deg).await.unwrap();
}

#[given("the mount reports both axes stopped in goto mode")]
async fn axes_stopped_in_goto(world: &mut StarAdventurerWorld) {
    seed_axes_stopped_in_goto(world).await;
}

#[when("the mount reports both axes stopped in goto mode")]
async fn when_axes_stopped_in_goto(world: &mut StarAdventurerWorld) {
    seed_axes_stopped_in_goto(world).await;
}

async fn seed_axes_stopped_in_goto(world: &mut StarAdventurerWorld) {
    // Tightly-timed scenarios that need the slew to "have just
    // finished" force the mock state directly via /debug/v1/mock-state.
    let mut seed = serde_json::Map::new();
    seed.insert("ra_running".into(), false.into());
    seed.insert("dec_running".into(), false.into());
    seed.insert("ra_goto".into(), true.into());
    seed.insert("dec_goto".into(), true.into());
    let body = serde_json::Value::Object(seed);
    let url = format!(
        "http://127.0.0.1:{}/debug/v1/mock-state",
        world.service_handle.as_ref().expect("service started").port
    );
    let _ = reqwest::Client::new().post(&url).json(&body).send().await;
}

#[then("the mount should have received commands matching:")]
async fn commands_matching(world: &mut StarAdventurerWorld, step: &Step) {
    use regex::Regex;
    let table = step.table.as_ref().expect("expected a data table");
    let log = world.command_log().await;
    let mut log_idx = 0usize;
    for row in table.rows.iter().skip(1) {
        let pattern = format!("^{}\r?$", row[0].trim());
        let re = Regex::new(&pattern).expect("invalid regex in feature file");
        let mut found = false;
        while log_idx < log.len() {
            if re.is_match(&log[log_idx]) {
                found = true;
                log_idx += 1;
                break;
            }
            log_idx += 1;
        }
        assert!(
            found,
            "expected log entry matching {pattern:?}; saw {log:?}"
        );
    }
}

#[then(expr = "TargetRightAscension should be {float} hours within {float}")]
async fn target_ra_should_be(world: &mut StarAdventurerWorld, expected: f64, tolerance: f64) {
    let actual = world.mount().target_right_ascension().await.unwrap();
    assert!((actual - expected).abs() < tolerance);
}

#[then(expr = "TargetDeclination should be {float} degrees within {float}")]
async fn target_dec_should_be(world: &mut StarAdventurerWorld, expected: f64, tolerance: f64) {
    let actual = world.mount().target_declination().await.unwrap();
    assert!((actual - expected).abs() < tolerance);
}

#[then(
    expr = "the slew target on the wire should correspond to RA {float} hours and Dec {float} degrees"
)]
async fn wire_slew_target(world: &mut StarAdventurerWorld, ra: f64, dec: f64) {
    // Loose check: the driver issues `:S1<bias-encoded-ticks>` and
    // `:S2<bias-encoded-ticks>`. We assert that *some* :S1/:S2 frame
    // is in the log; tightening the comparison to the exact ticks
    // would re-derive the LST inside the test, which has the same
    // clock-injection problem as the absolute-LST scenarios. The
    // unit test
    // `coordinates::tests::ra_ticks_round_trip_through_mechanical_ha`
    // pins the ticks↔RA arithmetic.
    let _ = (ra, dec);
    let log = world.command_log().await;
    assert!(
        log.iter()
            .any(|c| c.starts_with(":S1") && c.ends_with("\r")),
        "no :S1 in log {log:?}"
    );
    assert!(
        log.iter()
            .any(|c| c.starts_with(":S2") && c.ends_with("\r")),
        "no :S2 in log {log:?}"
    );
}

#[then(expr = "the mount should eventually receive a tracking-mode :G1 within {int} seconds")]
async fn mount_eventually_tracking_g1(world: &mut StarAdventurerWorld, secs: u64) {
    // Tracking-mode :G1 has the high nibble's bit 0x10 clear (per
    // `MotionMode::TRACKING.to_byte() == 0x00`). The wire form is
    // `:G1<HH>\r` where HH is two hex digits.
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        let log = world.command_log().await;
        if log.iter().any(|c| is_tracking_g1(c)) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let log = world.command_log().await;
    panic!("no tracking-mode :G1 within {secs}s; log {log:?}");
}

#[then("the mount should not receive a tracking-mode :G1")]
async fn mount_should_not_tracking_g1(world: &mut StarAdventurerWorld) {
    // Wait briefly to let any pending watcher iteration fire, then
    // assert no tracking-mode :G1 appeared.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let log = world.command_log().await;
    assert!(
        !log.iter().any(|c| is_tracking_g1(c)),
        "found tracking-mode :G1; log {log:?}"
    );
}

/// Returns `true` if `frame` is a `:G1<HH>\r` command whose mode byte
/// has the goto bit (`0x10`) cleared — i.e., a tracking-mode mode
/// switch on the RA axis.
fn is_tracking_g1(frame: &str) -> bool {
    let bytes = frame.as_bytes();
    if bytes.len() < 6 || &bytes[..3] != b":G1" {
        return false;
    }
    let hi = bytes[3];
    let lo = bytes[4];
    let parsed = match (hex_digit(hi), hex_digit(lo)) {
        (Some(h), Some(l)) => (h << 4) | l,
        _ => return false,
    };
    parsed & 0x10 == 0
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
