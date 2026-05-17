//! Step definitions for metadata.feature

use crate::world::FalconRotatorWorld;
use cucumber::then;

#[then("CanReverse should be true")]
async fn can_reverse_true(world: &mut FalconRotatorWorld) {
    let value = world.rotator().can_reverse().await.unwrap();
    assert!(value, "CanReverse must be true");
}

#[then("StepSize should be 0.01155")]
async fn step_size_is(world: &mut FalconRotatorWorld) {
    let value = world.rotator().step_size().await.unwrap();
    // Vendor product page: 86.6 steps per degree → 1.0 / 86.6 ≈ 0.01155.
    // f64 equality is exact for the literal-rounded value the driver returns.
    assert!((value - 0.01155).abs() < 1e-9, "got {value}");
}

#[then("Name should be \"Pegasus Falcon Rotator\"")]
async fn rotator_name_is(world: &mut FalconRotatorWorld) {
    let value = world.rotator().name().await.unwrap();
    assert_eq!(value, "Pegasus Falcon Rotator");
}

#[then("UniqueID should be \"pa-falcon-rotator-001\"")]
async fn rotator_unique_id_is(world: &mut FalconRotatorWorld) {
    let value = world.rotator().unique_id();
    assert_eq!(value, "pa-falcon-rotator-001");
}

#[then("InterfaceVersion should be 4")]
async fn rotator_interface_version_is(world: &mut FalconRotatorWorld) {
    let value = world.rotator().interface_version().await.unwrap();
    assert_eq!(value, 4);
}
