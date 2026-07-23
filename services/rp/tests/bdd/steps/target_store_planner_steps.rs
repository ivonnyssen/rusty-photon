//! BDD step definitions for planner altitude-gating parity against the
//! target store (`target_store_planner.feature`, Decision 9 —
//! *(planned, P1)*, not yet implemented; scenarios are tagged `@wip`).
//!
//! Unlike the other target_store_*.feature suites these scenarios boot
//! rp the ordinary OmniSim/mount way (`tool_steps::start_rp`), matching
//! `planner.feature`'s convention, since they exercise `get_next_target`
//! against a real site/ephemeris — `write_target_store_config`'s
//! lightweight no-OmniSim launcher is for the pure plan-data CRUD
//! suites that don't need a mount or site.

use cucumber::given;

use crate::steps::target_store_crud_steps::add_target_fixture;
use crate::world::RpWorld;

#[given(expr = "rp is configured with a target-store default minimum altitude of {float} degrees")]
fn default_min_altitude(world: &mut RpWorld, degrees: f64) {
    world.target_store_config = Some(serde_json::json!({
        "default_scheduling": { "min_altitude_degrees": degrees }
    }));
}

#[given(
    expr = "the MCP client has added a target named {string} at ra_hours {float} dec_degrees {float} with min_altitude_degrees {float}"
)]
async fn added_target_with_altitude_floor(
    world: &mut RpWorld,
    display_name: String,
    ra_hours: f64,
    dec_degrees: f64,
    min_altitude_degrees: f64,
) {
    add_target_fixture(
        world,
        serde_json::json!({
            "display_name": display_name,
            "ra_hours": ra_hours,
            "dec_degrees": dec_degrees,
            "scheduling": { "min_altitude_degrees": min_altitude_degrees }
        }),
    )
    .await;
}

// "the MCP client has added a target named {string} at ra_hours {float}
// dec_degrees {float}" (no altitude override) is reused from
// target_store_crud_steps.rs; "the MCP client calls \"get_next_target\""
// and "the result reason should be {string}" from ephemeris_steps.rs.
