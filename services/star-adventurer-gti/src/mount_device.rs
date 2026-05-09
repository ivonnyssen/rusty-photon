//! ASCOM Alpaca Telescope device for the Star Adventurer GTi.
//!
//! This is the surface that Alpaca clients (NINA, SGPro, `rp`, ...) talk to.
//! Capability-flag overrides match the design doc's
//! [§"Capability flags"](../../../docs/services/star-adventurer-gti.md#capability-flags)
//! table; defaulted methods that the MVP does not implement are left to the
//! ascom-alpaca trait's `NOT_IMPLEMENTED` default.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ascom_alpaca::api::telescope::{
    AlignmentMode, DriveRate, EquatorialCoordinateType, PierSide, Telescope, TelescopeAxis,
};
use ascom_alpaca::api::Device;
use ascom_alpaca::{ASCOMError, ASCOMResult};
use async_trait::async_trait;
use std::ops::RangeInclusive;
use tokio::sync::RwLock;

use crate::config::MountConfig;
use crate::transport_manager::TransportManager;

/// In-memory mirror of latched-from-the-client state (Tracking enabled,
/// AtPark flag, last target). The values that come from the wire (current
/// RA/Dec, Slewing) are read through [`TransportManager`].
#[derive(Debug, Default)]
#[allow(dead_code)] // Phase 3 reads target_ra_hours / target_dec_degrees in SlewToTarget*
struct DriverState {
    tracking_requested: bool,
    at_park: bool,
    target_ra_hours: Option<f64>,
    target_dec_degrees: Option<f64>,
    slew_settle_time: Option<Duration>,
}

pub struct MountDevice {
    config: MountConfig,
    requested_connection: Arc<RwLock<bool>>,
    state: Arc<RwLock<DriverState>>,
    transport: Arc<TransportManager>,
}

impl fmt::Debug for MountDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MountDevice")
            .field("config", &self.config)
            .field("requested_connection", &self.requested_connection)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl MountDevice {
    pub fn new(config: MountConfig, transport: Arc<TransportManager>) -> Self {
        Self {
            config,
            requested_connection: Arc::new(RwLock::new(false)),
            state: Arc::new(RwLock::new(DriverState::default())),
            transport,
        }
    }
}

#[async_trait]
impl Device for MountDevice {
    fn static_name(&self) -> &str {
        &self.config.name
    }

    fn unique_id(&self) -> &str {
        &self.config.unique_id
    }

    async fn description(&self) -> ASCOMResult<String> {
        Ok(self.config.description.clone())
    }

    async fn connected(&self) -> ASCOMResult<bool> {
        let requested = *self.requested_connection.read().await;
        Ok(requested && self.transport.is_available())
    }

    async fn set_connected(&self, _connected: bool) -> ASCOMResult<()> {
        // Phase 3: connect/disconnect the underlying TransportManager,
        // gated through requested_connection so the mirror is consistent.
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("Star Adventurer GTi Driver - ASCOM Alpaca Telescope for Sky-Watcher GEM".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }
}

#[async_trait]
impl Telescope for MountDevice {
    // ---- Capability flags (constants from the design doc) ----

    async fn alignment_mode(&self) -> ASCOMResult<AlignmentMode> {
        Ok(AlignmentMode::GermanPolar)
    }

    async fn equatorial_system(&self) -> ASCOMResult<EquatorialCoordinateType> {
        Ok(EquatorialCoordinateType::Topocentric)
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
    async fn can_set_tracking(&self) -> ASCOMResult<bool> {
        Ok(true)
    }
    async fn can_park(&self) -> ASCOMResult<bool> {
        Ok(true)
    }
    async fn can_unpark(&self) -> ASCOMResult<bool> {
        Ok(true)
    }
    async fn does_refraction(&self) -> ASCOMResult<bool> {
        Ok(false)
    }

    async fn tracking_rates(&self) -> ASCOMResult<Vec<DriveRate>> {
        Ok(vec![DriveRate::Sidereal])
    }

    // ---- Required-by-trait reads (Phase 3 fills in the body) ----

    async fn at_home(&self) -> ASCOMResult<bool> {
        Ok(false)
    }

    async fn at_park(&self) -> ASCOMResult<bool> {
        Ok(self.state.read().await.at_park)
    }

    async fn right_ascension(&self) -> ASCOMResult<f64> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn right_ascension_rate(&self) -> ASCOMResult<f64> {
        Ok(0.0)
    }

    async fn declination_rate(&self) -> ASCOMResult<f64> {
        Ok(0.0)
    }

    async fn sidereal_time(&self) -> ASCOMResult<f64> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn tracking(&self) -> ASCOMResult<bool> {
        Ok(self.state.read().await.tracking_requested)
    }

    async fn tracking_rate(&self) -> ASCOMResult<DriveRate> {
        Ok(DriveRate::Sidereal)
    }

    async fn utc_date(&self) -> ASCOMResult<SystemTime> {
        Ok(SystemTime::now())
    }

    async fn axis_rates(&self, _axis: TelescopeAxis) -> ASCOMResult<Vec<RangeInclusive<f64>>> {
        // MoveAxis is deferred from MVP; no rates supported.
        Ok(vec![])
    }

    // ---- Site coordinates (configured, read-only) ----

    async fn site_latitude(&self) -> ASCOMResult<f64> {
        Ok(self.config.site_latitude_deg)
    }

    async fn site_longitude(&self) -> ASCOMResult<f64> {
        Ok(self.config.site_longitude_deg)
    }

    async fn site_elevation(&self) -> ASCOMResult<f64> {
        Ok(self.config.site_elevation_m)
    }

    // ---- Side-of-pier read (Phase 3) ----

    async fn side_of_pier(&self) -> ASCOMResult<PierSide> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    // ---- Slew settle time (read/write, lives in the in-memory mirror) ----

    async fn slew_settle_time(&self) -> ASCOMResult<Duration> {
        Ok(self
            .state
            .read()
            .await
            .slew_settle_time
            .unwrap_or(self.config.settle_after_slew))
    }

    async fn set_slew_settle_time(&self, slew_settle_time: Duration) -> ASCOMResult<()> {
        self.state.write().await.slew_settle_time = Some(slew_settle_time);
        Ok(())
    }
}

#[cfg(all(test, feature = "mock"))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::transport::mock::MockTransportFactory;

    fn device() -> MountDevice {
        let cfg = Config::default();
        let manager = Arc::new(TransportManager::new(
            cfg.clone(),
            Arc::new(MockTransportFactory),
        ));
        MountDevice::new(cfg.mount, manager)
    }

    #[tokio::test]
    async fn fresh_device_reports_disconnected() {
        // requested_connection defaults to false; transport has never been
        // opened, so connected() must be false until set_connected lands.
        let d = device();
        assert!(!d.connected().await.unwrap());
    }

    #[tokio::test]
    async fn capability_flags_match_the_design_doc() {
        let d = device();
        // Capability flags are constants; pin them so a future change to
        // the design doc must update both this test and the capability
        // table at services/star-adventurer-gti.md §"Capability flags".
        assert_eq!(
            d.alignment_mode().await.unwrap(),
            AlignmentMode::GermanPolar
        );
        assert_eq!(
            d.equatorial_system().await.unwrap(),
            EquatorialCoordinateType::Topocentric
        );
        assert!(d.can_slew().await.unwrap());
        assert!(d.can_slew_async().await.unwrap());
        assert!(d.can_sync().await.unwrap());
        assert!(d.can_set_tracking().await.unwrap());
        assert!(d.can_park().await.unwrap());
        assert!(d.can_unpark().await.unwrap());
        assert!(!d.does_refraction().await.unwrap());
        assert_eq!(d.tracking_rates().await.unwrap(), vec![DriveRate::Sidereal]);
    }

    #[tokio::test]
    async fn defaulted_state_reads_match_initial_driver_state() {
        let d = device();
        assert!(!d.at_home().await.unwrap());
        assert!(!d.at_park().await.unwrap());
        assert!(!d.tracking().await.unwrap());
        assert_eq!(d.tracking_rate().await.unwrap(), DriveRate::Sidereal);
        // Ra/Dec rates are zeroed because custom-rate tracking is deferred
        // from MVP.
        assert_eq!(d.right_ascension_rate().await.unwrap(), 0.0);
        assert_eq!(d.declination_rate().await.unwrap(), 0.0);
    }

    #[tokio::test]
    async fn axis_rates_is_empty_for_every_axis() {
        // MoveAxis is deferred from MVP, so axis_rates returns an empty
        // Vec for every axis — including TelescopeAxis::Primary.
        let d = device();
        for axis in [
            TelescopeAxis::Primary,
            TelescopeAxis::Secondary,
            TelescopeAxis::Tertiary,
        ] {
            assert!(d.axis_rates(axis).await.unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn site_coordinates_pass_through_from_config() {
        let cfg = Config::default();
        let manager = Arc::new(TransportManager::new(
            cfg.clone(),
            Arc::new(MockTransportFactory),
        ));
        let mut mount_cfg = cfg.mount.clone();
        mount_cfg.site_latitude_deg = 47.6062;
        mount_cfg.site_longitude_deg = -122.3321;
        mount_cfg.site_elevation_m = 56.0;
        let d = MountDevice::new(mount_cfg, manager);
        assert_eq!(d.site_latitude().await.unwrap(), 47.6062);
        assert_eq!(d.site_longitude().await.unwrap(), -122.3321);
        assert_eq!(d.site_elevation().await.unwrap(), 56.0);
    }

    #[tokio::test]
    async fn slew_settle_time_setter_overrides_config() {
        let d = device();
        // Config default is 2s.
        assert_eq!(d.slew_settle_time().await.unwrap(), Duration::from_secs(2));
        d.set_slew_settle_time(Duration::from_millis(500))
            .await
            .unwrap();
        assert_eq!(
            d.slew_settle_time().await.unwrap(),
            Duration::from_millis(500)
        );
    }

    #[tokio::test]
    async fn set_connected_is_not_implemented_in_phase_2() {
        let d = device();
        let err = d.set_connected(true).await.unwrap_err();
        assert_eq!(err.code, ASCOMError::NOT_IMPLEMENTED.code);
    }

    #[tokio::test]
    async fn right_ascension_and_sidereal_time_are_not_implemented_in_phase_2() {
        let d = device();
        assert_eq!(
            d.right_ascension().await.unwrap_err().code,
            ASCOMError::NOT_IMPLEMENTED.code
        );
        assert_eq!(
            d.sidereal_time().await.unwrap_err().code,
            ASCOMError::NOT_IMPLEMENTED.code
        );
        assert_eq!(
            d.side_of_pier().await.unwrap_err().code,
            ASCOMError::NOT_IMPLEMENTED.code
        );
    }

    #[tokio::test]
    async fn driver_info_and_version_are_populated() {
        let d = device();
        assert!(d.driver_info().await.unwrap().contains("Star Adventurer"));
        assert!(!d.driver_version().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn description_passes_through_from_config() {
        let d = device();
        assert!(!d.description().await.unwrap().is_empty());
    }
}
