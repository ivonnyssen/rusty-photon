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
