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
