use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// J2000 mean equator/equinox (ICRS) target coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IcrsCoord {
    pub ra_hours: f64,
    pub dec_degrees: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AltAz {
    pub altitude_degrees: f64,
    pub azimuth_degrees: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LocalSiderealTime {
    pub lst_hours: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RiseSet {
    pub rise_utc: DateTime<Utc>,
    pub set_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SideOfPier {
    East,
    West,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TwilightKind {
    Civil,
    Nautical,
    Astronomical,
}

impl TwilightKind {
    /// Sun-altitude threshold for this twilight kind, in degrees.
    pub fn sun_altitude_threshold_degrees(self) -> f64 {
        match self {
            TwilightKind::Civil => -6.0,
            TwilightKind::Nautical => -12.0,
            TwilightKind::Astronomical => -18.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TwilightWindow {
    /// Sun crosses the threshold going down (evening twilight begins).
    pub begin_utc: Option<DateTime<Utc>>,
    /// Sun crosses the threshold going up (morning twilight ends).
    pub end_utc: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SunInfo {
    pub coords: IcrsCoord,
    pub alt_az: AltAz,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MoonInfo {
    pub coords: IcrsCoord,
    pub alt_az: AltAz,
    /// Elongation between the Sun and Moon as seen from Earth, in
    /// degrees `[0, 180]`. 0 = new, 90 = quarter, 180 = full.
    pub phase_degrees: f64,
    /// Illuminated fraction of the disc, `[0, 1]`. Computed as
    /// `(1 - cos(phase)) / 2` over the Sun-Earth-Moon elongation —
    /// 0 at new moon (elongation 0°), 1 at full moon (elongation
    /// 180°). Good to ~1 % outside the crescent regions for amateur
    /// observing.
    pub illumination_fraction: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum EphemerisError {
    #[error("ERFA reported an unrepresentable time/date input (status {0})")]
    InvalidTimeInput(i32),
    #[error("ERFA refused the alt/az transform (status {0}); inputs out of valid range")]
    InvalidAltAzInputs(i32),
}
