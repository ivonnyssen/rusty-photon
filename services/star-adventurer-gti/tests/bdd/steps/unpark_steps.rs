//! Steps for unpark_from_ap_position.feature.

use crate::world::StarAdventurerWorld;
use cucumber::{given, then, when};
use std::time::Duration;

/// Resolve an `ap_park_N` feature-file token to the typed [`ApPark`].
fn parse_ap_park(token: &str) -> star_adventurer_gti::ApPark {
    use star_adventurer_gti::ApPark;
    match token {
        "ap_park_0" => ApPark::ApPark0,
        "ap_park_1" => ApPark::ApPark1,
        "ap_park_2" => ApPark::ApPark2,
        "ap_park_3" => ApPark::ApPark3,
        "ap_park_4" => ApPark::ApPark4,
        "ap_park_5" => ApPark::ApPark5,
        other => panic!("unknown AP park token {other:?}"),
    }
}

#[given(expr = "a star-adventurer service configured with unpark_from_ap_position {string}")]
async fn configured_with_unpark(world: &mut StarAdventurerWorld, park: String) {
    world.config_mut().mount.unpark_from_ap_position = parse_ap_park(&park);
    world.start_service().await;
}

#[when(expr = "I run the SetUnparkFromApPosition action with {string}")]
async fn run_set_unpark(world: &mut StarAdventurerWorld, park: String) {
    world
        .mount()
        .action("SetUnparkFromApPosition".to_string(), park)
        .await
        .unwrap();
}

#[when(expr = "I run the UnparkFromApPosition action with {string}")]
async fn run_unpark_action(world: &mut StarAdventurerWorld, park: String) {
    world
        .mount()
        .action("UnparkFromApPosition".to_string(), park)
        .await
        .unwrap();
}

#[then("the mount should have received an encoder seed on both axes")]
async fn received_encoder_seed_both_axes(world: &mut StarAdventurerWorld) {
    // `:E1` / `:E2` (SetPosition) carry a hex payload, so match on the
    // frame prefix. Poll because CI runners can lag behind the wire.
    for _ in 0..60 {
        let log = world.command_log().await;
        let e1 = log.iter().any(|c| c.starts_with(":E1"));
        let e2 = log.iter().any(|c| c.starts_with(":E2"));
        if e1 && e2 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let log = world.command_log().await;
    panic!("expected :E1 and :E2 encoder-seed frames; saw {log:?}");
}

#[then("the mount should not have received an encoder-seed command")]
async fn no_encoder_seed(world: &mut StarAdventurerWorld) {
    // `set_connected` is awaited, so any connect-time seed has already
    // reached the mock log by now; absence is therefore reliable without
    // polling. `:E` frames only come from a seed / sync / reset.
    let log = world.command_log().await;
    let seeds: Vec<&String> = log.iter().filter(|c| c.starts_with(":E")).collect();
    assert!(
        seeds.is_empty(),
        "expected no :E encoder-seed frames, saw {seeds:?} in {log:?}"
    );
}

#[then(expr = "the persisted config should have unpark_from_ap_position {string}")]
async fn persisted_unpark(world: &mut StarAdventurerWorld, park: String) {
    let cfg = world.read_persisted_config();
    assert_eq!(cfg.mount.unpark_from_ap_position, parse_ap_park(&park));
}

#[then(expr = "SupportedActions should include {string}, {string}, and {string}")]
async fn supported_actions_include(
    world: &mut StarAdventurerWorld,
    first: String,
    second: String,
    third: String,
) {
    let actions = world.mount().supported_actions().await.unwrap();
    for want in [first, second, third] {
        assert!(
            actions.contains(&want),
            "SupportedActions {actions:?} missing {want:?}"
        );
    }
}
