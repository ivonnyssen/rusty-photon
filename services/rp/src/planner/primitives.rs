//! Helpers behind the primitive ephemeris MCP tools (`compute_alt_az`,
//! `compute_transit`, `compute_rise_set`, `compute_meridian_flip`,
//! `get_sun_position`, `get_twilight`, `get_moon_position`,
//! `compute_moon_separation`, `get_local_sidereal_time`).
//!
//! Each helper takes the parsed input + a borrowed `Site` (where
//! relevant) and returns a `serde_json::Value` that the MCP tool body
//! in `crate::mcp` projects onto a `CallToolResult` success or error
//! payload. Keeping the JSON-shaping in this module lets the
//! convenience tools (Phase 7) reuse the same primitive calls.

use chrono::{DateTime, NaiveDate, Utc};
use rp_ephemeris::{Ephemeris, ErfarsEphemeris, IcrsCoord, SideOfPier, Site, TwilightKind};
use serde_json::{json, Value};

/// Parse a humantime / RFC3339 timestamp, defaulting to `Utc::now()`
/// when the caller omits it.
pub fn parse_time_or_now(s: Option<&str>) -> Result<DateTime<Utc>, String> {
    match s {
        None => Ok(Utc::now()),
        Some(raw) => DateTime::parse_from_rfc3339(raw)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| {
                format!("invalid time {raw:?}: {e} (expect RFC3339, e.g. 2026-05-03T22:00:00Z)")
            }),
    }
}

/// Parse a `YYYY-MM-DD` UTC date string.
pub fn parse_date(s: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| format!("invalid date {s:?}: {e} (expect YYYY-MM-DD)"))
}

/// Parse the `kind` parameter for `get_twilight` (case-insensitive).
pub fn parse_twilight_kind(s: &str) -> Result<TwilightKind, String> {
    match s.to_ascii_lowercase().as_str() {
        "civil" => Ok(TwilightKind::Civil),
        "nautical" => Ok(TwilightKind::Nautical),
        "astronomical" => Ok(TwilightKind::Astronomical),
        other => Err(format!(
            "unknown twilight kind {other:?}; expected one of civil, nautical, astronomical"
        )),
    }
}

/// Parse the `side_of_pier` parameter for `compute_meridian_flip`.
pub fn parse_side_of_pier(s: &str) -> Result<SideOfPier, String> {
    match s.to_ascii_lowercase().as_str() {
        "east" => Ok(SideOfPier::East),
        "west" => Ok(SideOfPier::West),
        "unknown" => Ok(SideOfPier::Unknown),
        other => Err(format!(
            "unknown side_of_pier {other:?}; expected one of east, west, unknown"
        )),
    }
}

/// Validate ra (hours) ∈ [0, 24) and dec (degrees) ∈ [-90, 90].
pub fn validate_icrs(ra_hours: f64, dec_degrees: f64) -> Result<IcrsCoord, String> {
    if !(0.0..24.0).contains(&ra_hours) {
        return Err(format!("ra_hours must be in [0, 24); got {ra_hours}"));
    }
    if !(-90.0..=90.0).contains(&dec_degrees) {
        return Err(format!(
            "dec_degrees must be in [-90, 90]; got {dec_degrees}"
        ));
    }
    Ok(IcrsCoord {
        ra_hours,
        dec_degrees,
    })
}

/// Common error: a tool that needs a configured site was called when
/// the deployment has no `site` block. The MCP tool body projects this
/// onto a structured error payload.
pub fn site_required_error() -> String {
    "site not configured: this tool requires a `site` block in rp's config; \
     see docs/services/rp.md §\"Site Configuration\""
        .to_string()
}

// ---------------------------------------------------------------------------
// Tool body helpers — return JSON Value on success, String on failure.
// ---------------------------------------------------------------------------

pub fn compute_alt_az(
    site: &Site,
    target: IcrsCoord,
    time: DateTime<Utc>,
) -> Result<Value, String> {
    let aa = ErfarsEphemeris::new()
        .alt_az(site, target, time)
        .map_err(|e| format!("alt/az transform failed: {e}"))?;
    Ok(json!({
        "altitude_degrees": aa.altitude_degrees,
        "azimuth_degrees": aa.azimuth_degrees,
    }))
}

pub fn compute_transit(site: &Site, target: IcrsCoord, date: NaiveDate) -> Value {
    let result = ErfarsEphemeris::new().transit(site, target, date);
    json!({
        "transit_utc": result.map(|t| t.to_rfc3339()),
    })
}

pub fn compute_rise_set(
    site: &Site,
    target: IcrsCoord,
    date: NaiveDate,
    min_alt_degrees: f64,
) -> Value {
    let result = ErfarsEphemeris::new().rise_set(site, target, date, min_alt_degrees);
    match result {
        Some(rs) => json!({
            "rise_utc": rs.rise_utc.to_rfc3339(),
            "set_utc": rs.set_utc.to_rfc3339(),
        }),
        None => json!({
            "rise_utc": serde_json::Value::Null,
            "set_utc": serde_json::Value::Null,
        }),
    }
}

pub fn compute_meridian_flip(
    site: &Site,
    target: IcrsCoord,
    time: DateTime<Utc>,
    side: SideOfPier,
) -> Value {
    let result = ErfarsEphemeris::new().meridian_flip(site, target, time, side);
    json!({
        "time_to_flip_seconds": result.map(|d| d.num_seconds()),
    })
}

pub fn get_sun_position(site: &Site, time: DateTime<Utc>) -> Value {
    let info = ErfarsEphemeris::new().sun_position(site, time);
    json!({
        "ra_hours": info.coords.ra_hours,
        "dec_degrees": info.coords.dec_degrees,
        "altitude_degrees": info.alt_az.altitude_degrees,
        "azimuth_degrees": info.alt_az.azimuth_degrees,
    })
}

pub fn get_twilight(site: &Site, date: NaiveDate, kind: TwilightKind) -> Value {
    let w = ErfarsEphemeris::new().twilight(site, date, kind);
    json!({
        "kind": match kind {
            TwilightKind::Civil => "civil",
            TwilightKind::Nautical => "nautical",
            TwilightKind::Astronomical => "astronomical",
        },
        "begin_utc": w.begin_utc.map(|t| t.to_rfc3339()),
        "end_utc": w.end_utc.map(|t| t.to_rfc3339()),
    })
}

pub fn get_moon_position(site: &Site, time: DateTime<Utc>) -> Value {
    let info = ErfarsEphemeris::new().moon_position(site, time);
    json!({
        "ra_hours": info.coords.ra_hours,
        "dec_degrees": info.coords.dec_degrees,
        "altitude_degrees": info.alt_az.altitude_degrees,
        "azimuth_degrees": info.alt_az.azimuth_degrees,
        "phase_degrees": info.phase_degrees,
        "illumination_fraction": info.illumination_fraction,
    })
}

pub fn compute_moon_separation(target: IcrsCoord, time: DateTime<Utc>) -> Value {
    let sep = ErfarsEphemeris::new().moon_separation(target, time);
    json!({
        "separation_degrees": sep,
    })
}

pub fn get_local_sidereal_time(site: &Site, time: DateTime<Utc>) -> Value {
    let lst = ErfarsEphemeris::new().sidereal_time(site, time);
    json!({
        "lst_hours": lst.lst_hours,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site_seattle() -> Site {
        Site::new(47.6062, -122.3321).unwrap()
    }

    #[test]
    fn parses_rfc3339_or_uses_now() {
        let parsed = parse_time_or_now(Some("2026-05-03T22:00:00Z")).unwrap();
        assert_eq!(parsed.to_rfc3339(), "2026-05-03T22:00:00+00:00");
        let now = parse_time_or_now(None).unwrap();
        assert!(now.timestamp() > 0);
    }

    #[test]
    fn rejects_bad_time_with_helpful_diagnostic() {
        let err = parse_time_or_now(Some("not a time")).unwrap_err();
        assert!(err.contains("RFC3339"), "got: {err}");
    }

    #[test]
    fn parse_date_round_trips() {
        let d = parse_date("2026-05-03").unwrap();
        assert_eq!(d.format("%Y-%m-%d").to_string(), "2026-05-03");
    }

    #[test]
    fn icrs_validation_blocks_out_of_range() {
        assert!(validate_icrs(-1.0, 0.0).is_err());
        assert!(validate_icrs(24.0, 0.0).is_err());
        assert!(validate_icrs(0.0, 91.0).is_err());
        validate_icrs(0.7123, 41.27).unwrap();
    }

    #[test]
    fn twilight_kind_parses_known_variants() {
        assert_eq!(
            parse_twilight_kind("Astronomical").unwrap(),
            TwilightKind::Astronomical
        );
        assert!(parse_twilight_kind("daytime").is_err());
    }

    #[test]
    fn side_of_pier_parses_known_variants() {
        assert_eq!(parse_side_of_pier("East").unwrap(), SideOfPier::East);
        assert!(parse_side_of_pier("middle").is_err());
    }

    #[test]
    fn alt_az_returns_finite_numbers_for_known_target() {
        let site = site_seattle();
        let m31 = IcrsCoord {
            ra_hours: 0.7123,
            dec_degrees: 41.27,
        };
        let t = chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 11, 1, 6, 0, 0).unwrap();
        let v = compute_alt_az(&site, m31, t).unwrap();
        let alt = v["altitude_degrees"].as_f64().unwrap();
        assert!(alt.is_finite() && (-90.0..=90.0).contains(&alt));
    }

    #[test]
    fn lst_in_canonical_range() {
        let site = site_seattle();
        let t = chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 5, 3, 0, 0, 0).unwrap();
        let v = get_local_sidereal_time(&site, t);
        let lst = v["lst_hours"].as_f64().unwrap();
        assert!((0.0..24.0).contains(&lst));
    }
}
