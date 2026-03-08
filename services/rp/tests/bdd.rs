//! BDD test entry point for rp service

#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

use cucumber::World as _;
use world::RpWorld;

#[tokio::main]
async fn main() {
    RpWorld::run("tests/features").await;
}
