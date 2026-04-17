//! BDD test entry point for filemonitor service

#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

bdd_infra::bdd_main! {
    use cucumber::World as _;
    use world::FilemonitorWorld;

    FilemonitorWorld::cucumber()
        .max_concurrent_scenarios(Some(1))
        .after(|_feature, _rule, _scenario, _finished, maybe_world| {
            Box::pin(async move {
                if let Some(world) = maybe_world {
                    // Stop only directly-started servers (TLS/auth scenarios).
                    // Pool-managed servers are kept alive across scenarios.
                    if let Some(fm) = world.filemonitor.as_mut() {
                        fm.stop().await;
                    }
                }
            })
        })
        .run("tests/features")
        .await;
    steps::infrastructure::stop_all_servers().await;
}
