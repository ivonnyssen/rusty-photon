//! BDD test entry point for sentinel service

#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

use cucumber::World as _;
use world::SentinelWorld;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    SentinelWorld::run("tests/features").await;
}
