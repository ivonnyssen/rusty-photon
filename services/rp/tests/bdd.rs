//! BDD test entry point for rp service

#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

bdd_infra::bdd_main! {
    use cucumber::World as _;
    use world::RpWorld;

    // Skip scenarios tagged @wip — used to land BDD specs on a feature branch
    // before the implementation exists, without breaking the green suite.
    // Remove the tag once the corresponding implementation is in place.
    RpWorld::cucumber()
        .before(|_feature, _rule, _scenario, _world| {
            Box::pin(async move {
                // Reset every OmniSim device class our scenarios touch
                // (telescope, camera, filter wheel, focuser, cover
                // calibrator) to defaults before each scenario. The
                // shared OmniSim is a singleton across the BDD process,
                // so device state leaks between scenarios; the mount
                // leak that hung `park` in issue #143 is the case we
                // already hit. Each reset is a localhost PUT, all run
                // in parallel, so the per-scenario overhead is one
                // round-trip. No-op the first time through (OmniSim
                // hasn't been spawned yet).
                bdd_infra::rp_harness::OmniSimHandle::reset_all_devices().await;
            })
        })
        .after(|_feature, _rule, _scenario, _finished, maybe_world| {
            Box::pin(async move {
                if let Some(world) = maybe_world {
                    // Drop the MCP client first — its streaming HTTP
                    // connection would otherwise keep axum's graceful
                    // shutdown blocked, causing rp to time out and SIGKILL,
                    // which loses LLVM coverage profraw data.
                    world.mcp_client = None;
                    if let Some(rp) = world.rp.as_mut() {
                        rp.stop().await;
                    }
                }
            })
        })
        .filter_run("tests/features", |feat, _rule, sc| {
            let is_wip = feat.tags.iter().any(|t| t == "wip" || t == "@wip")
                || sc.tags.iter().any(|t| t == "wip" || t == "@wip");
            !is_wip
        })
        .await;
}
