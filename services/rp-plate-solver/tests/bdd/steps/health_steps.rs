//! Step definitions for `health.feature`.
//!
//! Phase 3 stubs. Bodies arrive in Phase 4.

use crate::world::PlateSolverWorld;
use cucumber::{given, when};

#[given("the wrapper is running with a temp-dir copy of mock_astap as its binary path")]
async fn given_wrapper_running_with_temp_copy(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: copy mock_astap into world.temp_dir, point config at the copy, spawn wrapper")
}

#[given("the wrapper is running with a temp astap_db_directory")]
async fn given_wrapper_running_with_temp_db(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: mkdir temp_dir/d05, point config there, spawn wrapper")
}

#[when("I delete the configured astap_binary_path")]
async fn when_delete_configured_binary(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: std::fs::remove_file(world.astap_binary_path)")
}

#[when("I delete the configured astap_db_directory")]
async fn when_delete_configured_db(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: std::fs::remove_dir_all(world.astap_db_directory)")
}

#[when("I GET /health")]
async fn when_get_health(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: GET wrapper.base_url + /health, capture into world.last_response")
}
