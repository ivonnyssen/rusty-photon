//! Step definitions for `configuration.feature`.
//!
//! Phase 3 stubs. Bodies arrive in Phase 4.

use crate::world::PlateSolverWorld;
use cucumber::{given, then, when};

#[given("a config without astap_binary_path")]
async fn given_config_without_binary_path(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: write a config file omitting astap_binary_path into world.temp_dir")
}

#[given("a config without astap_db_directory")]
async fn given_config_without_db_directory(_world: &mut PlateSolverWorld) {
    todo!("Phase 4")
}

#[given(expr = "a config with astap_binary_path {string}")]
async fn given_config_with_binary_path(_world: &mut PlateSolverWorld, _path: String) {
    todo!("Phase 4")
}

#[given(expr = "a config with astap_db_directory {string}")]
async fn given_config_with_db_directory(_world: &mut PlateSolverWorld, _path: String) {
    todo!("Phase 4")
}

#[given("a config with astap_binary_path pointing at a non-executable file")]
async fn given_non_executable_binary_path(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: write a 0o644 file into temp_dir, point config at it")
}

#[given("a valid astap_binary_path")]
async fn given_valid_binary_path(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: copy mock_astap into temp_dir")
}

#[given("a valid astap_db_directory")]
async fn given_valid_db_directory(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: mkdir temp_dir/d05")
}

#[given("a config with mock_astap as the binary path")]
async fn given_config_with_mock_astap(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: PlateSolverWorld::mock_astap_path() into config")
}

#[when("the wrapper starts")]
async fn when_wrapper_starts(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: spawn the wrapper binary; capture stdout/stderr")
}

#[then("the wrapper exits non-zero")]
async fn then_wrapper_exits_nonzero(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: assert world.last_wrapper_exit_code is Some(non-zero)")
}

#[then(expr = "the wrapper stderr names {string}")]
async fn then_wrapper_stderr_names(_world: &mut PlateSolverWorld, _needle: String) {
    todo!("Phase 4: assert world.last_wrapper_stderr.contains(needle)")
}

#[then("the wrapper stderr references the README")]
async fn then_wrapper_stderr_references_readme(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: assert stderr contains 'README' or 'install instructions'")
}

#[then("the wrapper prints bound_addr to stdout")]
async fn then_wrapper_prints_bound_addr(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: assert ServiceHandle parsed bound port from stdout")
}

#[then("the wrapper /health returns 200")]
async fn then_wrapper_health_returns_200(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: GET /health, assert 200")
}
