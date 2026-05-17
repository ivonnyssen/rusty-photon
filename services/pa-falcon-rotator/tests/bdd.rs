//! BDD entry point for pa-falcon-rotator.
//!
//! Tests run in-process: each scenario builds a `BoundServer` on an ephemeral
//! port, holds a shared `Arc<MockSerialPortFactory>` so steps can drive mock
//! state and inspect the wire-level command log, and drives the Rotator /
//! Switch trait surface via in-process Alpaca HTTP clients. The
//! `bdd_infra::bdd_main!` macro is **not** used here — it's a Miri shim for
//! BDD suites that spawn child processes via `ServiceHandle`, which we don't.

#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

#[tokio::main]
async fn main() {
    use cucumber::World as _;
    use world::FalconRotatorWorld;

    FalconRotatorWorld::cucumber()
        .after(|_feature, _rule, _scenario, _finished, maybe_world| {
            Box::pin(async move {
                if let Some(world) = maybe_world {
                    world.shutdown().await;
                }
            })
        })
        // Skip every scenario tagged `@wip` so a clean Phase 2 commit can
        // ride on `main` while Phase 3 fills the implementations in. We use
        // `filter_run_and_exit` (NOT the bare variant) per docs/skills/testing.md
        // §2.7 — without `_and_exit` the binary returns 0 even on failures and
        // CI silently passes on broken scenarios (see issue #171).
        .filter_run_and_exit("tests/features", |feat, _rule, sc| {
            let is_wip = feat.tags.iter().any(|t| t == "wip" || t == "@wip")
                || sc.tags.iter().any(|t| t == "wip" || t == "@wip");
            !is_wip
        })
        .await;
}
