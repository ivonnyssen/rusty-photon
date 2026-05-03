//! Step definitions for `real_astap_smoke.feature`.
//!
//! Phase 3 stubs. Bodies arrive in Phase 4. Note that
//! `@requires-astap` keeps these from running in PR jobs even after
//! Phase 4 removes `@wip`.

use crate::world::PlateSolverWorld;
use cucumber::given;

#[given("the wrapper is running with the real ASTAP_BINARY as its solver")]
async fn given_wrapper_with_real_astap(_world: &mut PlateSolverWorld) {
    todo!(
        "Phase 4: read ASTAP_BINARY env var (set by the nightly install-astap workflow); \
         start the wrapper pointing at it"
    )
}

#[given("the m31_known.fits fixture is on disk")]
async fn given_m31_fixture(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: copy tests/fixtures/m31_known.fits into temp_dir, store path in world")
}

#[given("the degenerate_no_stars.fits fixture is on disk")]
async fn given_degenerate_fixture(_world: &mut PlateSolverWorld) {
    todo!("Phase 4: same shape as m31; the degenerate fixture exists for solve_failed coverage")
}
