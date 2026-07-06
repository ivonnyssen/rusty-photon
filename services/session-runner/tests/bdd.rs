//! BDD test entry point for the session-runner service.
//!
//! These tests spawn three processes — OmniSim, rp, and session-runner —
//! and drive workflow documents end-to-end via rp's REST API.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]

#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

bdd_infra::bdd_main! {
    use cucumber::World as _;
    use world::SessionRunnerWorld;

    SessionRunnerWorld::cucumber()
        .before(|_feature, _rule, _scenario, _world| {
            Box::pin(async move {
                // Reset every OmniSim device class our scenarios touch to
                // defaults before each scenario — OmniSim is a per-process
                // singleton, so scenario N's state (cover position,
                // calibrator brightness, filter slot, camera config) would
                // otherwise leak into scenario N+1. Failures from the very
                // first scenario's hook (before any Given step has called
                // `OmniSimHandle::start()`) are non-fatal: connection-
                // refused against the default port is the expected case
                // there.
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
                    if let Some(sr) = world.session_runner.as_mut() {
                        sr.stop().await;
                    }
                    if let Some(rp) = world.rp.as_mut() {
                        rp.stop().await;
                    }
                }
            })
        })
        // `@wip` scenarios are durable design artifacts for behavior that
        // is not implemented yet (testing.md §2.7) — Phase D (triggers,
        // resume, safety) lands its scenarios ahead of the engine work.
        .filter_run_and_exit("tests/features", |feat, _rule, sc| {
            let is_wip = feat.tags.iter().any(|t| t == "wip" || t == "@wip")
                || sc.tags.iter().any(|t| t == "wip" || t == "@wip");
            !is_wip
        })
        .await;
}
