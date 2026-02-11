#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

use cucumber::World as _;
use world::FilemonitorWorld;

#[tokio::main]
async fn main() {
    FilemonitorWorld::run("tests/features").await;
}
