//! BDD test entry point for sentinel service

#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

bdd_infra::bdd_main! {
    use cucumber::World as _;
    use world::SentinelWorld;

    SentinelWorld::cucumber()
        .after(|_feature, _rule, _scenario, _finished, maybe_world| {
            Box::pin(async move {
                if let Some(world) = maybe_world {
                    // Stop sentinel first so it can disconnect from filemonitor
                    if let Some(sentinel) = world.sentinel.as_mut() {
                        sentinel.stop().await;
                    }
                    if let Some(fm) = world.filemonitor.as_mut() {
                        fm.stop().await;
                    }
                }
            })
        })
        .run_and_exit("tests/features")
        .await;
}
