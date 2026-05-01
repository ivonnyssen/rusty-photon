//! BDD test entry point for sky-survey-camera service.

#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

bdd_infra::bdd_main! {
    use cucumber::World as _;
    use world::SkySurveyCameraWorld;

    SkySurveyCameraWorld::cucumber()
        .after(|_feature, _rule, _scenario, _finished, maybe_world| {
            Box::pin(async move {
                if let Some(world) = maybe_world {
                    if let Some(handle) = world.service.as_mut() {
                        handle.stop().await;
                    }
                }
            })
        })
        .run_and_exit("tests/features")
        .await;
}
