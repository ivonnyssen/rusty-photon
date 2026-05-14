//! Steps for pulse_guide.feature.

use crate::world::StarAdventurerWorld;
use cucumber::{then, when};
use std::time::{Duration, Instant};

fn parse_direction(s: &str) -> ascom_alpaca::api::telescope::GuideDirection {
    use ascom_alpaca::api::telescope::GuideDirection;
    match s {
        "North" => GuideDirection::North,
        "South" => GuideDirection::South,
        "East" => GuideDirection::East,
        "West" => GuideDirection::West,
        other => panic!("unknown guide direction {other:?}"),
    }
}

#[when(expr = "I pulse guide {word} for {int} ms")]
async fn pulse_guide(world: &mut StarAdventurerWorld, direction: String, ms: u64) {
    let dir = parse_direction(&direction);
    world
        .mount()
        .pulse_guide(dir, Duration::from_millis(ms))
        .await
        .unwrap();
}

#[when(expr = "I try to pulse guide {word} for {int} ms")]
async fn try_pulse_guide(world: &mut StarAdventurerWorld, direction: String, ms: u64) {
    let dir = parse_direction(&direction);
    match world
        .mount()
        .pulse_guide(dir, Duration::from_millis(ms))
        .await
    {
        Ok(()) => world.clear_error(),
        Err(e) => world.record_error(e),
    }
}

#[when(expr = "I set GuideRateRightAscension to {float}")]
async fn set_guide_rate_ra(world: &mut StarAdventurerWorld, value: f64) {
    world
        .mount()
        .set_guide_rate_right_ascension(value)
        .await
        .unwrap();
}

#[when(expr = "I set GuideRateDeclination to {float}")]
async fn set_guide_rate_dec(world: &mut StarAdventurerWorld, value: f64) {
    world
        .mount()
        .set_guide_rate_declination(value)
        .await
        .unwrap();
}

#[when(expr = "I try to set GuideRateRightAscension to {float}")]
async fn try_set_guide_rate_ra(world: &mut StarAdventurerWorld, value: f64) {
    match world.mount().set_guide_rate_right_ascension(value).await {
        Ok(()) => world.clear_error(),
        Err(e) => world.record_error(e),
    }
}

#[when(expr = "I try to set GuideRateDeclination to {float}")]
async fn try_set_guide_rate_dec(world: &mut StarAdventurerWorld, value: f64) {
    match world.mount().set_guide_rate_declination(value).await {
        Ok(()) => world.clear_error(),
        Err(e) => world.record_error(e),
    }
}

#[then("CanPulseGuide should be true")]
async fn can_pulse_guide_true(world: &mut StarAdventurerWorld) {
    assert!(world.mount().can_pulse_guide().await.unwrap());
}

#[then("CanSetGuideRates should be true")]
async fn can_set_guide_rates_true(world: &mut StarAdventurerWorld) {
    assert!(world.mount().can_set_guide_rates().await.unwrap());
}

#[then("IsPulseGuiding should be true")]
async fn is_pulse_guiding_true(world: &mut StarAdventurerWorld) {
    assert!(world.mount().is_pulse_guiding().await.unwrap());
}

#[then("IsPulseGuiding should be false")]
async fn is_pulse_guiding_false(world: &mut StarAdventurerWorld) {
    assert!(!world.mount().is_pulse_guiding().await.unwrap());
}

#[then(expr = "IsPulseGuiding should become false within {int} ms")]
async fn is_pulse_guiding_becomes_false(world: &mut StarAdventurerWorld, ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        if !world.mount().is_pulse_guiding().await.unwrap() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("IsPulseGuiding did not clear within {ms} ms");
}

#[then(expr = "GuideRateRightAscension should be approximately {float} within {float}")]
async fn guide_rate_ra_approx(world: &mut StarAdventurerWorld, expected: f64, tolerance: f64) {
    let actual = world.mount().guide_rate_right_ascension().await.unwrap();
    assert!(
        (actual - expected).abs() < tolerance,
        "GuideRateRightAscension: got {actual}, expected {expected} ± {tolerance}"
    );
}

#[then(expr = "GuideRateDeclination should be approximately {float} within {float}")]
async fn guide_rate_dec_approx(world: &mut StarAdventurerWorld, expected: f64, tolerance: f64) {
    let actual = world.mount().guide_rate_declination().await.unwrap();
    assert!(
        (actual - expected).abs() < tolerance,
        "GuideRateDeclination: got {actual}, expected {expected} ± {tolerance}"
    );
}

#[then(expr = "the RA tracking-mode :G110 frame count should be exactly {int}")]
async fn ra_g110_count(world: &mut StarAdventurerWorld, expected: usize) {
    // Each pulse start emits one `:G110\r`. A restored sidereal-tracking
    // re-issue (the watcher's post-sleep branch when `tracking_was_on`)
    // emits a second `:G110\r`. Counting frames in the log lets a
    // scenario assert whether the restore step fired without trying to
    // identify which frame came from which call site.
    let log = world.command_log().await;
    let count = log.iter().filter(|c| c.as_str() == ":G110\r").count();
    assert_eq!(
        count, expected,
        "expected exactly {expected} :G110 frames, saw {count} in log {log:?}"
    );
}
