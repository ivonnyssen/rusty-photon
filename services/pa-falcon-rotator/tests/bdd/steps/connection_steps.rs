//! Step definitions for connection_lifecycle.feature
//!
//! All scenarios are `@wip` in Phase 2; the bodies below are stubbed and
//! become real in Phase 3d. Defining them now keeps the cucumber crate happy
//! when the feature file is parsed.

use crate::world::FalconRotatorWorld;
use cucumber::{given, then, when};

#[given("a running pa-falcon-rotator service")]
async fn running_service(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("connection_steps::running_service implemented in Phase 3d")
}

#[when("I connect the rotator")]
async fn connect_rotator(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("connection_steps::connect_rotator implemented in Phase 3d")
}

#[when("I disconnect the rotator")]
async fn disconnect_rotator(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("connection_steps::disconnect_rotator implemented in Phase 3d")
}

#[then("the rotator should be connected")]
async fn rotator_should_be_connected(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("connection_steps::rotator_should_be_connected implemented in Phase 3d")
}

#[then("the rotator should be disconnected")]
async fn rotator_should_be_disconnected(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("connection_steps::rotator_should_be_disconnected implemented in Phase 3d")
}

#[then("the handshake should have issued F# before any other command")]
async fn handshake_issued_ping_first(world: &mut FalconRotatorWorld) {
    let _ = world;
    todo!("connection_steps::handshake_issued_ping_first implemented in Phase 3d")
}
