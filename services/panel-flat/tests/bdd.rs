//! BDD test entry point for panel-flat service

#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

bdd_infra::bdd_main! {
    use cucumber::World as _;
    use world::PanelFlatWorld;

    PanelFlatWorld::cucumber()
        .run_and_exit("tests/features")
        .await;
}
