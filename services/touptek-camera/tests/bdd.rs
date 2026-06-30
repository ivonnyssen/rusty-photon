//! BDD entry point. Spawns the touptek-camera binary (built with the
//! `simulation` backend) and drives it through the typed ASCOM Alpaca Camera
//! client. The binary must be pre-built with `--features simulation` (or
//! `--all-features`).
//!
//! The full `Camera` surface is implemented (Phase E), so the feature files run
//! for real. The `filter_run_and_exit` runner still honours `@wip` as an opt-in
//! skip for any not-yet-implemented scenario added later (see
//! `docs/skills/testing.md` §2.7 and `docs/services/touptek-camera.md` "Testing").

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]

#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

bdd_infra::bdd_main! {
    use cucumber::World as _;
    use world::CameraWorld;

    CameraWorld::cucumber()
        .after(|_feature, _rule, _scenario, _finished, maybe_world| {
            Box::pin(async move {
                if let Some(world) = maybe_world {
                    if let Some(handle) = world.handle.as_mut() {
                        handle.stop().await;
                    }
                }
            })
        })
        // Skip any scenario explicitly tagged `@wip` (an opt-in escape hatch for
        // work-in-progress scenarios), so they can land before their implementation
        // without breaking the green-suite invariant; `_and_exit` makes a scenario
        // failure a non-zero exit (testing.md §2.7).
        .filter_run_and_exit("tests/features", |feature, _rule, scenario| {
            let is_wip = feature.tags.iter().any(|t| t == "wip" || t == "@wip")
                || scenario.tags.iter().any(|t| t == "wip" || t == "@wip");
            !is_wip
        })
        .await;
}
