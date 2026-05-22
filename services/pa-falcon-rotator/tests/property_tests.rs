//! Property tests for the Falcon Rotator wire protocol.
//!
//! Round-trip: build a valid `FA` wire payload from random fields, parse it
//! back via `parse_full_status`, and assert every field survives. The
//! degree field is generated as integer hundredths so the `{:.2}` write
//! format and the `f64` parse can compare exactly without epsilon.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]

use pa_falcon_rotator::protocol::{parse_full_status, FalconStatus};
use proptest::prelude::*;

proptest! {
    #[test]
    fn full_status_wire_format_round_trips(
        steps in 0u32..1_000_000,
        deg_cents in 0u32..36_000,
        is_moving in any::<bool>(),
        limit_detect in any::<bool>(),
        do_derotation in any::<bool>(),
        motor_reverse in any::<bool>(),
    ) {
        let deg = f64::from(deg_cents) / 100.0;
        let wire = format!(
            "FR_OK:{steps}:{deg:.2}:{}:{}:{}:{}",
            u8::from(is_moving),
            u8::from(limit_detect),
            u8::from(do_derotation),
            u8::from(motor_reverse),
        );
        let parsed = parse_full_status(&wire).unwrap();
        prop_assert_eq!(parsed, FalconStatus {
            position_steps: steps,
            position_deg: deg,
            is_moving,
            limit_detect,
            do_derotation,
            motor_reverse,
        });
    }
}
