//! BDD entry point for the dsd-fp2 driver.
//!
//! Scenarios run in-process against the mock `TransportFactory`; no
//! subprocess spawn, so `bdd_infra::bdd_main!` is unnecessary (same
//! pattern as the qhy-focuser BDD suite for in-process tests).

#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

use cucumber::World as _;
use world::Fp2World;

#[tokio::main]
async fn main() {
    Fp2World::cucumber().run_and_exit("tests/features").await;
}
