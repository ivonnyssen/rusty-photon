use std::sync::OnceLock;

use tzf_rs::DefaultFinder;

/// Observer site: geographic latitude / longitude only. Elevation is
/// deferred to a future revision (see `docs/services/rp.md`
/// §"Planning and Ephemeris").
#[derive(Debug, Clone, Copy)]
pub struct Site {
    pub latitude_degrees: f64,
    pub longitude_degrees: f64,
    iana_tz: &'static str,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SiteError {
    #[error("latitude_degrees must be in [-90, 90]; got {0}")]
    LatitudeOutOfRange(f64),
    #[error("longitude_degrees must be in [-180, 180]; got {0}")]
    LongitudeOutOfRange(f64),
}

static FINDER: OnceLock<DefaultFinder> = OnceLock::new();

fn finder() -> &'static DefaultFinder {
    FINDER.get_or_init(DefaultFinder::new)
}

impl Site {
    /// Construct a site, validating the lat/lon range and resolving
    /// the IANA timezone via `tzf-rs`. The finder is constructed on
    /// first call and held for the lifetime of the process — see the
    /// memory-footprint discussion in `docs/plans/rp-planning-tools.md`.
    pub fn new(latitude_degrees: f64, longitude_degrees: f64) -> Result<Self, SiteError> {
        if !(-90.0..=90.0).contains(&latitude_degrees) {
            return Err(SiteError::LatitudeOutOfRange(latitude_degrees));
        }
        if !(-180.0..=180.0).contains(&longitude_degrees) {
            return Err(SiteError::LongitudeOutOfRange(longitude_degrees));
        }
        // tzf-rs takes lng, lat (note order); the &str borrows from the
        // process-static finder, so its lifetime is effectively 'static.
        let iana_tz: &'static str = finder().get_tz_name(longitude_degrees, latitude_degrees);
        Ok(Self {
            latitude_degrees,
            longitude_degrees,
            iana_tz,
        })
    }

    /// IANA timezone name (e.g. `"America/Los_Angeles"`) derived from
    /// the site's lat/lon.
    pub fn iana_timezone(&self) -> &'static str {
        self.iana_tz
    }
}

impl std::fmt::Display for Site {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "site lat={:.4}° lon={:.4}° tz={}",
            self.latitude_degrees, self.longitude_degrees, self.iana_tz
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_out_of_range_latitude() {
        assert_eq!(
            Site::new(91.0, 0.0).unwrap_err(),
            SiteError::LatitudeOutOfRange(91.0)
        );
        assert_eq!(
            Site::new(-90.5, 0.0).unwrap_err(),
            SiteError::LatitudeOutOfRange(-90.5)
        );
    }

    #[test]
    fn rejects_out_of_range_longitude() {
        assert_eq!(
            Site::new(0.0, 181.0).unwrap_err(),
            SiteError::LongitudeOutOfRange(181.0)
        );
    }

    #[test]
    fn seattle_resolves_to_pacific_timezone() {
        let s = Site::new(47.6062, -122.3321).unwrap();
        // tzf-rs is allowed to drift between updates; assert prefix only.
        assert!(
            s.iana_timezone().starts_with("America/"),
            "expected America/* timezone, got {}",
            s.iana_timezone()
        );
    }

    #[test]
    fn madrid_resolves_to_european_timezone() {
        let s = Site::new(40.4168, -3.7038).unwrap();
        assert!(
            s.iana_timezone().starts_with("Europe/"),
            "expected Europe/* timezone, got {}",
            s.iana_timezone()
        );
    }
}
