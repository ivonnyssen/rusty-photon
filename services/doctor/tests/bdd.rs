//! BDD entry point for doctor. The suite drives the real binary (built
//! with the `mock` feature) through `--platform-facts`, so every scenario
//! stages its own host state and config directory hermetically.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]

#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

bdd_infra::bdd_main! {
    use cucumber::World as _;
    use world::DoctorWorld;

    DoctorWorld::cucumber()
        .filter_run_and_exit("tests/features", |feat, _rule, sc| {
            let is_wip = feat.tags.iter().any(|t| t == "wip" || t == "@wip")
                || sc.tags.iter().any(|t| t == "wip" || t == "@wip");
            !is_wip
        })
        .await;
}
