#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

use cucumber::World as _;
use world::QhyCameraWorld;

#[tokio::main]
async fn main() {
    QhyCameraWorld::run("tests/features").await;
}
