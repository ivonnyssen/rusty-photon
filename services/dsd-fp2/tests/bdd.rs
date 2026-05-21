//! BDD entry point for the dsd-fp2 driver.
//!
//! Scenarios run **in-process** against the `MockTransportFactory`: the
//! `World` constructs the device, manager, and factory directly and the
//! `Session` mediates wire calls through `MockFrameTransport`. No
//! subprocess is spawned, so `bdd_infra::bdd_main!` is not needed
//! (unlike `qhy-focuser`'s BDD suite, which launches the binary).

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
