//! Steps for connection_lifecycle.feature.
//!
//! Phase 2 stubs — all scenarios are tagged `@wip` so these bodies are not
//! exercised yet. Phase 3 implements them as Phase 3 wires the mock
//! transport's command-history view through the World struct.

#![allow(unused_variables)]

use crate::world::StarAdventurerWorld;
use cucumber::gherkin::Step;
use cucumber::{given, then, when};

#[given("a running star-adventurer service")]
async fn running_service(world: &mut StarAdventurerWorld) {
    world.start_service().await;
}

#[given(expr = "a mount that reports CPR {int} on both axes")]
async fn mount_reports_cpr(world: &mut StarAdventurerWorld, cpr: u32) {
    todo!("Phase 3: pre-seed mock state.cpr_ra/cpr_dec before start_service()")
}

#[given(expr = "a mount that reports timer frequency {int}")]
async fn mount_reports_tmr_freq(world: &mut StarAdventurerWorld, hz: u32) {
    todo!("Phase 3: pre-seed mock state.tmr_freq before start_service()")
}

#[given("the mount is slewing")]
async fn mount_is_slewing(world: &mut StarAdventurerWorld) {
    todo!("Phase 3: pre-seed mock axis state running=true, goto=true on both axes")
}

#[when("I connect the device")]
async fn connect_device(world: &mut StarAdventurerWorld) {
    world.mount().set_connected(true).await.unwrap();
}

#[when("I disconnect the device")]
async fn disconnect_device(world: &mut StarAdventurerWorld) {
    world.mount().set_connected(false).await.unwrap();
}

#[when("two clients connect the device")]
async fn two_clients_connect(world: &mut StarAdventurerWorld) {
    todo!("Phase 3: open a second AlpacaClient and call set_connected(true) on both")
}

#[when("one client disconnects the device")]
async fn one_client_disconnects(world: &mut StarAdventurerWorld) {
    todo!("Phase 3: drive disconnect on the first of the two clients only")
}

#[when("the remaining client disconnects the device")]
async fn remaining_client_disconnects(world: &mut StarAdventurerWorld) {
    todo!("Phase 3: drive disconnect on the second client")
}

#[then("the device should be disconnected")]
async fn device_should_be_disconnected(world: &mut StarAdventurerWorld) {
    assert!(!world.mount().connected().await.unwrap());
}

#[then("the device should be connected")]
async fn device_should_be_connected(world: &mut StarAdventurerWorld) {
    assert!(world.mount().connected().await.unwrap());
}

#[then("the device should still be connected")]
async fn device_should_still_be_connected(world: &mut StarAdventurerWorld) {
    assert!(world.mount().connected().await.unwrap());
}

#[then("the underlying transport should have been opened exactly once")]
async fn transport_opened_once(world: &mut StarAdventurerWorld) {
    todo!("Phase 3: assert mock-transport open-count == 1")
}

#[then("the mount should have received commands in order:")]
async fn commands_received_in_order(world: &mut StarAdventurerWorld, step: &Step) {
    let _rows = step.table.as_ref().expect("expected a data table");
    todo!("Phase 3: read mock command-history, assert prefix-match in order against table column")
}

#[then(expr = "the parameter cache should report CPR {int} on the RA axis")]
async fn param_cache_cpr_ra(world: &mut StarAdventurerWorld, expected: u32) {
    todo!("Phase 3: read TransportManager.parameters().cpr_ra and assert eq")
}

#[then(expr = "the parameter cache should report CPR {int} on the Dec axis")]
async fn param_cache_cpr_dec(world: &mut StarAdventurerWorld, expected: u32) {
    todo!("Phase 3: read TransportManager.parameters().cpr_dec and assert eq")
}

#[then(expr = "the parameter cache should report timer frequency {int}")]
async fn param_cache_tmr_freq(world: &mut StarAdventurerWorld, expected: u32) {
    todo!("Phase 3: read TransportManager.parameters().tmr_freq and assert eq")
}

#[then(expr = "the mount should have received command {word}")]
async fn mount_received_command(world: &mut StarAdventurerWorld, command: String) {
    todo!("Phase 3: assert mock command-history contains the exact frame")
}
