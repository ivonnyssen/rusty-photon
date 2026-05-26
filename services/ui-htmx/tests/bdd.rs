//! BDD entry point for `ui-htmx`. Each scenario spawns the real `ui-htmx`
//! binary and a real `dsd-fp2` driver (mock hardware) and drives the BFF over
//! HTTP, so the `bdd_infra::bdd_main!` macro is used (child-process spawning,
//! skipped under Miri). Both binaries must be pre-built with `--all-features`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]

#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

bdd_infra::bdd_main! {
    use cucumber::World as _;
    use world::UiWorld;

    UiWorld::cucumber()
        .after(|_feature, _rule, _scenario, _finished, maybe_world| {
            Box::pin(async move {
                if let Some(world) = maybe_world {
                    // Stop the BFF first so it stops calling the driver, then
                    // stop the driver.
                    if let Some(ui) = world.ui.as_mut() {
                        ui.stop().await;
                    }
                    if let Some(driver) = world.driver.as_mut() {
                        driver.stop().await;
                    }
                }
            })
        })
        .run_and_exit("tests/features")
        .await;
}
