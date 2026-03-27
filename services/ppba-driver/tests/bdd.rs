#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

bdd_infra::bdd_main! {
    use cucumber::World as _;
    use world::PpbaWorld;

    PpbaWorld::cucumber()
        // Run one scenario at a time: each spawns a ppba-driver subprocess
        // that binds the Alpaca discovery UDP port (32227). On macOS,
        // SO_REUSEADDR alone doesn't allow multiple binds to the same UDP
        // port (SO_REUSEPORT is also needed), so parallel scenarios cause
        // "Address already in use" failures.
        .max_concurrent_scenarios(Some(1))
        .after(|_feature, _rule, _scenario, _finished, maybe_world| {
            Box::pin(async move {
                if let Some(world) = maybe_world {
                    if let Some(ppba) = world.ppba.as_mut() {
                        ppba.stop().await;
                    }
                }
            })
        })
        .run_and_exit("tests/features")
        .await;
}
