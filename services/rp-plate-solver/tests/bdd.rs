//! BDD test entry point for rp-plate-solver.
//!
//! Two filter conditions:
//!
//! - `@wip` — Phase 3 lands feature files and step stubs before Phase
//!   4's HTTP server exists. All scenarios are tagged `@wip` until
//!   then; Phase 4 removes the tag in the same commit that lands the
//!   implementation. Convention per `docs/skills/testing.md` §2.7.
//!
//! - `@requires-astap` — gates a small cross-platform real-ASTAP
//!   smoke that fires only when `ASTAP_BINARY` is set in the
//!   environment. PR jobs do not set it; the dedicated nightly
//!   workflow does. See `docs/plans/rp-plate-solver.md` §"Real-ASTAP
//!   coverage: cadence and gating".
//!
//! Both filter forms accept the tag with or without a leading `@`,
//! matching `services/rp/tests/bdd.rs`'s pattern (cucumber-rs may
//! strip the leading sigil depending on parser version).

#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

bdd_infra::bdd_main! {
    use cucumber::World as _;
    use world::PlateSolverWorld;

    PlateSolverWorld::cucumber()
        .after(|_feature, _rule, _scenario, _finished, maybe_world| {
            Box::pin(async move {
                if let Some(world) = maybe_world {
                    if let Some(handle) = world.service_handle.as_mut() {
                        handle.stop().await;
                    }
                }
            })
        })
        .filter_run("tests/features", |feat, _rule, sc| {
            let is_wip = feat.tags.iter().any(|t| t == "wip" || t == "@wip")
                || sc.tags.iter().any(|t| t == "wip" || t == "@wip");
            let needs_astap = feat
                .tags
                .iter()
                .any(|t| t == "requires-astap" || t == "@requires-astap")
                || sc
                    .tags
                    .iter()
                    .any(|t| t == "requires-astap" || t == "@requires-astap");
            let astap_available = std::env::var("ASTAP_BINARY").is_ok();
            !is_wip && (!needs_astap || astap_available)
        })
        .await;
}
