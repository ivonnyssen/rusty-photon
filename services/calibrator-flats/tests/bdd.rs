//! BDD test entry point for the calibrator-flats service.
//!
//! These tests spawn three processes — OmniSim, rp, and calibrator-flats —
//! and drive the flat calibration workflow end-to-end via rp's REST API.

#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

bdd_infra::bdd_main! {
    use cucumber::World as _;
    use world::CalibratorFlatsWorld;

    CalibratorFlatsWorld::cucumber()
        .after(|_feature, _rule, _scenario, _finished, maybe_world| {
            Box::pin(async move {
                if let Some(world) = maybe_world {
                    if let Some(cf) = world.calibrator_flats.as_mut() {
                        cf.stop().await;
                    }
                    if let Some(rp) = world.rp.as_mut() {
                        rp.stop().await;
                    }
                }
            })
        })
        .run_and_exit("tests/features")
        .await;
}
