//! Step definitions for `solve_request.feature` — and shared HTTP /
//! response-assertion phrases reused by other feature files.
//!
//! Phase 3 stubs. Bodies arrive in Phase 4.

use crate::world::PlateSolverWorld;
use cucumber::{given, then, when};

// ----- shared "Given the wrapper is running" used across features -----

#[given("the wrapper is running with mock_astap as its solver")]
async fn given_wrapper_running_with_mock(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: write config pointing at mock_astap, spawn wrapper, store handle in world")
}

// ----- mock_astap mode setup -----

#[given(expr = "mock_astap is configured for {string} mode")]
async fn given_mock_astap_mode(_world: &mut PlateSolverWorld, _mode: String) {
    todo!("Phase 4: store the mode for the next solve request to set MOCK_ASTAP_MODE on the spawned child")
}

#[given("mock_astap is configured to write argv to a side-channel file")]
async fn given_mock_astap_argv_out(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: set MOCK_ASTAP_ARGV_OUT on the spawned child to a temp file path; store path in world.argv_out_path")
}

#[given("a writable FITS path")]
async fn given_writable_fits_path(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: create a unique FITS file under world.temp_dir, store path in world")
}

#[given("a non-existent FITS path")]
async fn given_non_existent_fits_path(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: build an absolute path under world.temp_dir that we never create")
}

// ----- when: HTTP requests -----
//
// Note on the `\\/` escapes in `expr = "..."` patterns below: cucumber
// expressions interpret bare `/` as alternation (e.g., `red/blue`
// matches `red` or `blue`). A literal `/api/v1/solve` triggers a
// compile error: "An alternation can not be empty." `\\/` in the Rust
// string yields `\/` in the cucumber expression, the documented
// escape for a literal `/`. The literal-string form (no `expr =`) is
// not a cucumber expression and accepts bare `/`, which is why the
// next step works without escaping.

#[when("I POST to /api/v1/solve with that fits_path")]
async fn when_post_solve_with_fits_path(_world: &mut PlateSolverWorld) {
    todo!(
        "Phase 4: build solve request, POST to wrapper, capture response into world.last_response"
    )
}

#[when(expr = "I POST to \\/api\\/v1\\/solve with fits_path {string}")]
async fn when_post_solve_with_explicit_path(_world: &mut PlateSolverWorld, _path: String) {
    todo!("Phase 4")
}

#[when(expr = "I POST to \\/api\\/v1\\/solve with raw body {string}")]
async fn when_post_solve_with_raw_body(_world: &mut PlateSolverWorld, _body: String) {
    todo!("Phase 4: POST raw bytes (no JSON wrapping) to test invalid_request decode path")
}

#[when(expr = "I POST to \\/api\\/v1\\/solve with that fits_path and timeout {string}")]
async fn when_post_solve_with_timeout(_world: &mut PlateSolverWorld, _timeout: String) {
    todo!("Phase 4")
}

#[when(expr = "I POST to \\/api\\/v1\\/solve with that fits_path and hint {word} set to {word}")]
async fn when_post_solve_with_hint(_world: &mut PlateSolverWorld, _field: String, _value: String) {
    todo!("Phase 4: parse field/value, build request with that hint set, POST")
}

// ----- then: response assertions (shared across feature files) -----

#[then(expr = "the response status is {int}")]
async fn then_response_status_is(_world: &mut PlateSolverWorld, _status: u16) {
    todo!("Phase 4: assert world.last_response.status == status")
}

#[then(expr = "the response field {string} is {string}")]
async fn then_response_field_is_string(
    _world: &mut PlateSolverWorld,
    _path: String,
    _expected: String,
) {
    todo!("Phase 4: jsonpath into world.last_response.body, assert equals expected")
}

#[then(expr = "the response field {string} is {int}")]
async fn then_response_field_is_int(_world: &mut PlateSolverWorld, _path: String, _expected: i64) {
    todo!("Phase 4")
}

#[then(expr = "the response field {string} is approximately {float}")]
async fn then_response_field_approx(_world: &mut PlateSolverWorld, _path: String, _expected: f64) {
    todo!("Phase 4: assert |actual - expected| < default tolerance (e.g., 1e-3)")
}

#[then(expr = "the response field {string} is approximately {float} within {float} degrees")]
async fn then_response_field_approx_within(
    _world: &mut PlateSolverWorld,
    _path: String,
    _expected: f64,
    _tolerance: f64,
) {
    todo!("Phase 4: assert |actual - expected| < tolerance")
}

#[then(expr = "the response field {string} contains {string}")]
async fn then_response_field_contains(
    _world: &mut PlateSolverWorld,
    _path: String,
    _needle: String,
) {
    todo!("Phase 4")
}

#[then(expr = "the response field {string} contains {string} case-insensitively")]
async fn then_response_field_contains_ci(
    _world: &mut PlateSolverWorld,
    _path: String,
    _needle: String,
) {
    todo!("Phase 4: lowercase both sides and compare; matches both ASTAP and astap")
}

#[then(expr = "the spawned argv contains the flag {string}")]
async fn then_argv_contains_flag(_world: &mut PlateSolverWorld, _flag: String) {
    todo!("Phase 4: read world.argv_out_path, parse argv, assert presence of flag")
}

#[then(expr = "the spawned argv value after {string} is approximately {float}")]
async fn then_argv_value_after_flag(_world: &mut PlateSolverWorld, _flag: String, _expected: f64) {
    todo!("Phase 4: read argv, find flag, parse the next token as f64, compare")
}
