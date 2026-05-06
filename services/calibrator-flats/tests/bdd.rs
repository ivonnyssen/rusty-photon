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
        .before(|_feature, _rule, _scenario, _world| {
            Box::pin(async move {
                // Reset every OmniSim device class our scenarios touch
                // (telescope, camera, filter wheel, focuser, cover
                // calibrator) to defaults before each scenario. OmniSim
                // is a per-process singleton; without this, state from
                // scenario N (cover position, calibrator brightness,
                // filter slot, camera config) leaks into scenario N+1.
                // Each reset is a localhost PUT, run sequentially
                // (parallel resets raced OmniSim's unsynchronised
                // `AlpacaDevices` list — see `reset_all_devices` for
                // the writeup). We panic on any reset failure so a
                // flaky reset surfaces loudly here rather than as a
                // confusing downstream step failure.
                if let Err(errors) =
                    bdd_infra::rp_harness::OmniSimHandle::reset_all_devices().await
                {
                    panic!("OmniSim device reset failed: {}", errors.join("; "));
                }
            })
        })
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
