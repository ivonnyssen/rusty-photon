#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

use cucumber::World as _;
use world::PpbaWorld;

// Use current_thread to ensure deterministic task scheduling.
// The serial manager spawns a background poller whose first tick fires
// immediately — with a multi-threaded runtime, the poller races with
// test steps for mock responses, causing non-deterministic failures.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    PpbaWorld::run("tests/features").await;
}
