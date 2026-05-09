//! Encoder-tick ↔ celestial-coordinate conversions.
//!
//! The mount's wire protocol speaks raw encoder ticks; ASCOM speaks RA/Dec
//! (hours and degrees). Bridging the two requires:
//!
//! * Counts-per-revolution (per axis, queried at handshake — see
//!   [`MountParameters`](crate::transport_manager::MountParameters)).
//! * The sync offset (added on read, subtracted on write — set by
//!   `SyncToCoordinates`).
//! * Local apparent sidereal time (computed from host UTC + site
//!   longitude).
//! * Site latitude (for Az/Alt and side-of-pier derivation).
//!
//! These functions are pure — given the same parameters, they always return
//! the same answer. They are unit-tested directly without the transport
//! layer in scope.

use ascom_alpaca::api::telescope::PierSide;

/// Hours in a sidereal day, rounded for round-trip-friendly arithmetic. The
/// LST math in Phase 3 uses an erfa-grade routine; this constant is here so
/// stub callers can place-hold without pulling erfa in yet.
pub const HOURS_PER_DAY: f64 = 24.0;

/// Convert RA-axis encoder ticks to a mechanical hour-angle in the range
/// `[-12, +12)` hours.
pub fn ra_ticks_to_mechanical_ha(_ticks: i32, _cpr: u32) -> f64 {
    unimplemented!("Phase 3: ticks * 24 / cpr, then wrap to [-12, +12)")
}

/// Convert Dec-axis encoder ticks to a declination angle in degrees, range
/// `[-90, +90]`.
pub fn dec_ticks_to_degrees(_ticks: i32, _cpr: u32) -> f64 {
    unimplemented!("Phase 3: ticks * 360 / cpr, fold through pole if needed")
}

/// Local apparent sidereal time in hours `[0, 24)` from the host's wall
/// clock and the configured site longitude.
pub fn local_sidereal_time_hours(_utc: std::time::SystemTime, _site_longitude_deg: f64) -> f64 {
    unimplemented!("Phase 3: erfa GMST + equation of equinoxes + longitude")
}

/// Mechanical hour angle (signed hours) → ASCOM right ascension (hours
/// `[0, 24)`), given the LST.
pub fn mechanical_ha_to_ra(_mech_ha: f64, _lst_hours: f64) -> f64 {
    unimplemented!("Phase 3: ra = lst - mech_ha, fold to [0, 24)")
}

/// Side-of-pier classification derived from the RA-axis mechanical hour
/// angle and site latitude.
///
/// In the northern hemisphere, mechanical HA in `[-6, +6)` is the East side
/// (`PierSide::East`); the rest is West. Southern hemisphere inverts.
pub fn side_of_pier(_mech_ha: f64, _site_latitude_deg: f64) -> PierSide {
    unimplemented!("Phase 3: hemisphere-aware quadrant test")
}
