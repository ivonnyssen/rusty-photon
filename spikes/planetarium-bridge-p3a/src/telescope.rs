//! The virtual Telescope device: answers plausibly, actuates nothing,
//! narrates everything.
//!
//! Slews are simulated — `Slewing` reports true for a configurable
//! convergence window while the reported position interpolates toward the
//! target, so the client sees a mount that "arrives". Sync is accepted and
//! reflected (the real bridge will ignore it; the spike reflects it to keep
//! the client's pointing model happy and to observe the client's follow-up
//! behavior). Every intent verb narrates at info level; poll getters stay
//! quiet here because the wire middleware records them all.

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use ascom_alpaca::api::telescope::{
    AlignmentMode, DriveRate, EquatorialCoordinateType, Telescope, TelescopeAxis,
};
use ascom_alpaca::api::Device;
use ascom_alpaca::{ASCOMError, ASCOMResult};
use async_trait::async_trait;
use chrono::Utc;
use rp_ephemeris::{Ephemeris, ErfarsEphemeris, IcrsCoord, Site};
use tracing::{debug, info, warn};

use crate::epoch::{EpochInference, EpochVerdict};

/// A simulated slew in flight.
#[derive(Debug, Clone, Copy)]
struct Slew {
    from: (f64, f64),
    to: (f64, f64),
    started: Instant,
    duration: Duration,
}

#[derive(Debug)]
struct MountState {
    connected: bool,
    tracking: bool,
    ra_hours: f64,
    dec_degrees: f64,
    target_ra: Option<f64>,
    target_dec: Option<f64>,
    slew: Option<Slew>,
    site: Site,
    site_elevation: f64,
}

pub struct SpikeTelescope {
    state: Mutex<MountState>,
    ephemeris: ErfarsEphemeris,
    inference: EpochInference,
    slew_duration: Duration,
    equatorial_system: EquatorialCoordinateType,
}

impl SpikeTelescope {
    pub fn new(
        site: Site,
        site_elevation: f64,
        ephemeris: ErfarsEphemeris,
        inference: EpochInference,
        slew_duration: Duration,
        equatorial_system: EquatorialCoordinateType,
    ) -> Self {
        // Start pointed at the meridian on the celestial equator — a
        // plausible idle pointing that is always above the horizon
        // somewhere sensible.
        let initial_ra = ephemeris.sidereal_time(&site, Utc::now()).lst_hours;
        Self {
            state: Mutex::new(MountState {
                connected: false,
                tracking: true,
                ra_hours: initial_ra,
                dec_degrees: 0.0,
                target_ra: None,
                target_dec: None,
                slew: None,
                site,
                site_elevation,
            }),
            ephemeris,
            inference,
            slew_duration,
            equatorial_system,
        }
    }

    /// Run `f` against the folded-forward mount state. A poisoned lock is
    /// unreachable in practice (no closure here panics), but the spike
    /// shrugs and reuses the inner state rather than propagating.
    fn with_state<R>(&self, f: impl FnOnce(&mut MountState) -> R) -> R {
        let mut guard = match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        fold_position(&mut guard);
        f(&mut guard)
    }

    fn position(&self) -> (f64, f64) {
        self.with_state(|s| (s.ra_hours, s.dec_degrees))
    }

    /// Shared GoTo path: narrate, run epoch inference, start the simulated
    /// slew.
    fn record_goto(&self, verb: &str, ra_hours: f64, dec_degrees: f64) -> ASCOMResult<()> {
        validate_coords(ra_hours, dec_degrees)?;
        info!(
            "GOTO [{verb}] RA {} Dec {}  (raw: {ra_hours:.6} h / {dec_degrees:+.5}°)",
            fmt_hms(ra_hours),
            fmt_dms(dec_degrees),
        );
        let report = self.inference.analyze(ra_hours, dec_degrees, Utc::now());
        if let Some((name, sep)) = &report.nearest_j2000 {
            info!(
                "  nearest probe (J2000 frame): {name} at {:.2}′",
                sep * 60.0
            );
        }
        if let Some((name, sep)) = &report.nearest_apparent {
            info!(
                "  nearest probe (JNow frame):  {name} at {:.2}′",
                sep * 60.0
            );
        }
        match report.verdict {
            EpochVerdict::LooksJ2000 => info!("  ⇒ EPOCH VERDICT: client sent J2000/ICRS"),
            EpochVerdict::LooksApparentOfDate => {
                info!("  ⇒ EPOCH VERDICT: client sent JNow (apparent of date)");
            }
            EpochVerdict::NoVerdict => {
                info!("  ⇒ no epoch verdict (not near a probe object — GoTo e.g. M 31 for one)")
            }
        }
        self.with_state(|s| {
            s.target_ra = Some(ra_hours);
            s.target_dec = Some(dec_degrees);
            s.slew = Some(Slew {
                from: (s.ra_hours, s.dec_degrees),
                to: (ra_hours, dec_degrees),
                started: Instant::now(),
                duration: self.slew_duration,
            });
        });
        Ok(())
    }

    fn record_sync(&self, verb: &str, ra_hours: f64, dec_degrees: f64) -> ASCOMResult<()> {
        validate_coords(ra_hours, dec_degrees)?;
        info!(
            "SYNC [{verb}] RA {} Dec {} — pointing-model gesture; the real bridge \
             ignores this (accepted here so the client stays happy)",
            fmt_hms(ra_hours),
            fmt_dms(dec_degrees),
        );
        self.with_state(|s| {
            s.slew = None;
            s.ra_hours = ra_hours;
            s.dec_degrees = dec_degrees;
        });
        Ok(())
    }

    fn stored_target(&self) -> ASCOMResult<(f64, f64)> {
        self.with_state(|s| match (s.target_ra, s.target_dec) {
            (Some(ra), Some(dec)) => Ok((ra, dec)),
            _ => Err(ASCOMError::VALUE_NOT_SET),
        })
    }
}

/// Complete or advance an in-flight slew; called under the state lock.
fn fold_position(state: &mut MountState) {
    if let Some(slew) = state.slew {
        let frac = slew.started.elapsed().as_secs_f64() / slew.duration.as_secs_f64();
        if frac >= 1.0 {
            state.ra_hours = slew.to.0;
            state.dec_degrees = slew.to.1;
            state.slew = None;
            info!(
                "slew complete: now at RA {} Dec {}",
                fmt_hms(state.ra_hours),
                fmt_dms(state.dec_degrees)
            );
        } else {
            state.ra_hours = interp_ra(slew.from.0, slew.to.0, frac);
            state.dec_degrees = slew.from.1 + (slew.to.1 - slew.from.1) * frac;
        }
    }
}

/// RA interpolation along the shortest arc, wrap-aware at 0h/24h.
fn interp_ra(from: f64, to: f64, frac: f64) -> f64 {
    let mut delta = (to - from).rem_euclid(24.0);
    if delta > 12.0 {
        delta -= 24.0;
    }
    (from + delta * frac).rem_euclid(24.0)
}

fn validate_coords(ra_hours: f64, dec_degrees: f64) -> ASCOMResult<()> {
    if !(0.0..24.0).contains(&ra_hours) || !(-90.0..=90.0).contains(&dec_degrees) {
        warn!("client sent out-of-range coordinates: RA {ra_hours} h Dec {dec_degrees}°");
        return Err(ASCOMError::invalid_value(format!(
            "RA {ra_hours} h / Dec {dec_degrees}° out of range"
        )));
    }
    Ok(())
}

fn fmt_hms(ra_hours: f64) -> String {
    let total_seconds = (ra_hours.rem_euclid(24.0) * 3600.0).round() as u64;
    format!(
        "{:02}h{:02}m{:02}s",
        total_seconds / 3600,
        (total_seconds / 60) % 60,
        total_seconds % 60
    )
}

fn fmt_dms(dec_degrees: f64) -> String {
    let sign = if dec_degrees < 0.0 { '-' } else { '+' };
    let total_arcsec = (dec_degrees.abs() * 3600.0).round() as u64;
    format!(
        "{sign}{:02}°{:02}′{:02}″",
        total_arcsec / 3600,
        (total_arcsec / 60) % 60,
        total_arcsec % 60
    )
}

impl std::fmt::Debug for SpikeTelescope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpikeTelescope").finish_non_exhaustive()
    }
}

#[async_trait]
impl Device for SpikeTelescope {
    fn static_name(&self) -> &str {
        "P3a Spike (virtual target-entry device, NOT a mount)"
    }

    fn unique_id(&self) -> &str {
        "planetarium-bridge-p3a-spike-0"
    }

    async fn connected(&self) -> ASCOMResult<bool> {
        Ok(self.with_state(|s| s.connected))
    }

    async fn set_connected(&self, connected: bool) -> ASCOMResult<()> {
        info!(
            "CLIENT {}",
            if connected {
                "CONNECTED"
            } else {
                "DISCONNECTED"
            }
        );
        self.with_state(|s| s.connected = connected);
        Ok(())
    }

    async fn description(&self) -> ASCOMResult<String> {
        Ok(
            "P3a verification spike: virtual Alpaca Telescope that records planetarium \
            GoTos. It is not a mount and never moves hardware."
                .to_owned(),
        )
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("rusty-photon planetarium-bridge P3a spike (throwaway)".to_owned())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_owned())
    }
}

#[async_trait]
impl Telescope for SpikeTelescope {
    async fn alignment_mode(&self) -> ASCOMResult<AlignmentMode> {
        Ok(AlignmentMode::GermanPolar)
    }

    async fn equatorial_system(&self) -> ASCOMResult<EquatorialCoordinateType> {
        info!(
            "client read EquatorialSystem → {:?}",
            self.equatorial_system
        );
        Ok(self.equatorial_system)
    }

    async fn at_home(&self) -> ASCOMResult<bool> {
        Ok(false)
    }

    async fn at_park(&self) -> ASCOMResult<bool> {
        Ok(false)
    }

    async fn can_park(&self) -> ASCOMResult<bool> {
        Ok(false)
    }

    async fn can_find_home(&self) -> ASCOMResult<bool> {
        Ok(false)
    }

    async fn can_pulse_guide(&self) -> ASCOMResult<bool> {
        Ok(false)
    }

    async fn can_set_tracking(&self) -> ASCOMResult<bool> {
        Ok(true)
    }

    async fn can_slew(&self) -> ASCOMResult<bool> {
        Ok(true)
    }

    async fn can_slew_async(&self) -> ASCOMResult<bool> {
        Ok(true)
    }

    async fn can_sync(&self) -> ASCOMResult<bool> {
        Ok(true)
    }

    async fn right_ascension(&self) -> ASCOMResult<f64> {
        Ok(self.position().0)
    }

    async fn declination(&self) -> ASCOMResult<f64> {
        Ok(self.position().1)
    }

    async fn right_ascension_rate(&self) -> ASCOMResult<f64> {
        Ok(0.0)
    }

    async fn declination_rate(&self) -> ASCOMResult<f64> {
        Ok(0.0)
    }

    async fn altitude(&self) -> ASCOMResult<f64> {
        let (ra_hours, dec_degrees) = self.position();
        let site = self.with_state(|s| s.site);
        self.ephemeris
            .alt_az(
                &site,
                IcrsCoord {
                    ra_hours,
                    dec_degrees,
                },
                Utc::now(),
            )
            .map(|aa| aa.altitude_degrees)
            .map_err(|e| ASCOMError::invalid_operation(format!("alt-az failed: {e}")))
    }

    async fn azimuth(&self) -> ASCOMResult<f64> {
        let (ra_hours, dec_degrees) = self.position();
        let site = self.with_state(|s| s.site);
        self.ephemeris
            .alt_az(
                &site,
                IcrsCoord {
                    ra_hours,
                    dec_degrees,
                },
                Utc::now(),
            )
            .map(|aa| aa.azimuth_degrees)
            .map_err(|e| ASCOMError::invalid_operation(format!("alt-az failed: {e}")))
    }

    async fn sidereal_time(&self) -> ASCOMResult<f64> {
        let site = self.with_state(|s| s.site);
        Ok(self.ephemeris.sidereal_time(&site, Utc::now()).lst_hours)
    }

    async fn site_latitude(&self) -> ASCOMResult<f64> {
        Ok(self.with_state(|s| s.site.latitude_degrees))
    }

    async fn set_site_latitude(&self, site_latitude: f64) -> ASCOMResult<()> {
        info!("client SET SiteLatitude = {site_latitude}°");
        let longitude = self.with_state(|s| s.site.longitude_degrees);
        let site = Site::new(site_latitude, longitude)
            .map_err(|e| ASCOMError::invalid_value(e.to_string()))?;
        self.with_state(|s| s.site = site);
        Ok(())
    }

    async fn site_longitude(&self) -> ASCOMResult<f64> {
        Ok(self.with_state(|s| s.site.longitude_degrees))
    }

    async fn set_site_longitude(&self, site_longitude: f64) -> ASCOMResult<()> {
        info!("client SET SiteLongitude = {site_longitude}°");
        let latitude = self.with_state(|s| s.site.latitude_degrees);
        let site = Site::new(latitude, site_longitude)
            .map_err(|e| ASCOMError::invalid_value(e.to_string()))?;
        self.with_state(|s| s.site = site);
        Ok(())
    }

    async fn site_elevation(&self) -> ASCOMResult<f64> {
        Ok(self.with_state(|s| s.site_elevation))
    }

    async fn set_site_elevation(&self, site_elevation: f64) -> ASCOMResult<()> {
        info!("client SET SiteElevation = {site_elevation} m");
        self.with_state(|s| s.site_elevation = site_elevation);
        Ok(())
    }

    async fn slewing(&self) -> ASCOMResult<bool> {
        Ok(self.with_state(|s| s.slew.is_some()))
    }

    async fn slew_settle_time(&self) -> ASCOMResult<Duration> {
        Ok(Duration::ZERO)
    }

    async fn target_right_ascension(&self) -> ASCOMResult<f64> {
        self.stored_target().map(|(ra, _)| ra)
    }

    async fn set_target_right_ascension(&self, target_right_ascension: f64) -> ASCOMResult<()> {
        info!("client SET TargetRightAscension = {target_right_ascension} h");
        validate_coords(target_right_ascension, 0.0)?;
        self.with_state(|s| s.target_ra = Some(target_right_ascension));
        Ok(())
    }

    async fn target_declination(&self) -> ASCOMResult<f64> {
        self.stored_target().map(|(_, dec)| dec)
    }

    async fn set_target_declination(&self, target_declination: f64) -> ASCOMResult<()> {
        info!("client SET TargetDeclination = {target_declination}°");
        validate_coords(0.0, target_declination)?;
        self.with_state(|s| s.target_dec = Some(target_declination));
        Ok(())
    }

    async fn tracking(&self) -> ASCOMResult<bool> {
        Ok(self.with_state(|s| s.tracking))
    }

    async fn set_tracking(&self, tracking: bool) -> ASCOMResult<()> {
        info!("client SET Tracking = {tracking}");
        self.with_state(|s| s.tracking = tracking);
        Ok(())
    }

    async fn tracking_rate(&self) -> ASCOMResult<DriveRate> {
        Ok(DriveRate::Sidereal)
    }

    async fn set_tracking_rate(&self, tracking_rate: DriveRate) -> ASCOMResult<()> {
        info!("client SET TrackingRate = {tracking_rate:?}");
        if tracking_rate == DriveRate::Sidereal {
            Ok(())
        } else {
            Err(ASCOMError::invalid_value("only Sidereal is supported"))
        }
    }

    async fn tracking_rates(&self) -> ASCOMResult<Vec<DriveRate>> {
        Ok(vec![DriveRate::Sidereal])
    }

    async fn utc_date(&self) -> ASCOMResult<SystemTime> {
        Ok(SystemTime::now())
    }

    async fn axis_rates(
        &self,
        axis: TelescopeAxis,
    ) -> ASCOMResult<Vec<std::ops::RangeInclusive<f64>>> {
        debug!("client read AxisRates for {axis:?} → none (no manual motion)");
        Ok(Vec::new())
    }

    async fn abort_slew(&self) -> ASCOMResult<()> {
        info!("ABORT SLEW received");
        self.with_state(|s| s.slew = None);
        Ok(())
    }

    async fn slew_to_coordinates(&self, right_ascension: f64, declination: f64) -> ASCOMResult<()> {
        self.record_goto("SlewToCoordinates (blocking)", right_ascension, declination)?;
        // The blocking variant's contract: return once the slew completes.
        tokio::time::sleep(self.slew_duration).await;
        debug!("blocking slew returned after the convergence window");
        Ok(())
    }

    async fn slew_to_coordinates_async(
        &self,
        right_ascension: f64,
        declination: f64,
    ) -> ASCOMResult<()> {
        self.record_goto("SlewToCoordinatesAsync", right_ascension, declination)
    }

    async fn slew_to_target(&self) -> ASCOMResult<()> {
        let (ra, dec) = self.stored_target()?;
        self.record_goto("SlewToTarget (blocking)", ra, dec)?;
        tokio::time::sleep(self.slew_duration).await;
        Ok(())
    }

    async fn slew_to_target_async(&self) -> ASCOMResult<()> {
        let (ra, dec) = self.stored_target()?;
        self.record_goto("SlewToTargetAsync", ra, dec)
    }

    async fn sync_to_coordinates(&self, right_ascension: f64, declination: f64) -> ASCOMResult<()> {
        self.record_sync("SyncToCoordinates", right_ascension, declination)
    }

    async fn sync_to_target(&self) -> ASCOMResult<()> {
        let (ra, dec) = self.stored_target()?;
        self.record_sync("SyncToTarget", ra, dec)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn interp_ra_takes_the_short_way_across_the_wrap() {
        // 23.8h → 0.2h is a 0.4h eastward hop through 24h/0h, not a 23.6h
        // westward sweep.
        let halfway = interp_ra(23.8, 0.2, 0.5);
        assert!(
            (halfway - 0.0).abs() < 1e-9 || (halfway - 24.0).abs() < 1e-9,
            "expected the midpoint at the wrap, got {halfway}"
        );
        assert!((interp_ra(23.8, 0.2, 1.0) - 0.2).abs() < 1e-9);
    }

    #[test]
    fn interp_ra_is_plain_interpolation_away_from_the_wrap() {
        assert!((interp_ra(2.0, 4.0, 0.25) - 2.5).abs() < 1e-9);
    }

    #[test]
    fn out_of_range_coordinates_are_rejected() {
        assert!(validate_coords(24.0, 0.0).is_err());
        assert!(validate_coords(-0.1, 0.0).is_err());
        assert!(validate_coords(0.0, 90.1).is_err());
        assert!(validate_coords(12.0, -45.0).is_ok());
    }

    #[test]
    fn formatting_matches_astronomical_notation() {
        assert_eq!(fmt_hms(0.712_305), "00h42m44s");
        assert_eq!(fmt_dms(41.269_167), "+41°16′09″");
        assert_eq!(fmt_dms(-11.623), "-11°37′23″");
    }
}
