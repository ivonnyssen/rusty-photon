//! BDD entry point for doctor. The suite drives the real binary (built
//! with the `mock` feature) through `--platform-facts`, so every scenario
//! stages its own host state and config directory hermetically. The
//! `@pebble` scenarios additionally spawn a private Pebble ACME directory
//! and run only when `PEBBLE_PATH` and `PEBBLE_CHALLTESTSRV_PATH` are set
//! (docs/skills/testing.md §5.6) — a skip is announced loudly, never
//! silent.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]

#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/pebble.rs"]
mod pebble;

#[path = "bdd/steps/mod.rs"]
mod steps;

/// How many scenarios the `@pebble` skip drops, counted from the feature
/// sources (each scenario carries its own `@pebble` line).
fn pebble_scenario_count(features_dir: &str) -> usize {
    let mut count = 0;
    for entry in std::fs::read_dir(features_dir)
        .expect("features dir")
        .flatten()
    {
        if let Ok(content) = std::fs::read_to_string(entry.path()) {
            count += content
                .lines()
                .filter(|line| line.trim() == "@pebble")
                .count();
        }
    }
    count
}

bdd_infra::bdd_main! {
    use cucumber::World as _;
    use world::DoctorWorld;

    let pebble_available = pebble::env_paths().is_some();
    if !pebble_available {
        eprintln!(
            "skipping {} @pebble scenarios: PEBBLE_PATH and/or \
             PEBBLE_CHALLTESTSRV_PATH are not set (docs/skills/testing.md \
             section 5.6 — download a Pebble release to run them locally)",
            pebble_scenario_count("tests/features")
        );
    }

    DoctorWorld::cucumber()
        .filter_run_and_exit("tests/features", move |feat, _rule, sc| {
            let tagged = |tag: &str, at_tag: &str| {
                feat.tags.iter().chain(sc.tags.iter()).any(|t| t == tag || t == at_tag)
            };
            if tagged("wip", "@wip") {
                return false;
            }
            pebble_available || !tagged("pebble", "@pebble")
        })
        .await;
}
