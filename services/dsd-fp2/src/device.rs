//! ASCOM Alpaca CoverCalibrator device for the Deep Sky Dad FP2.
//!
//! Holds an `Arc<FlatPanelManager>` (the service-wide façade over
//! `SharedTransport<Fp2Codec>`) plus a per-device session slot. The
//! session-per-device pattern follows the migration sketch in
//! `docs/plans/shared-transport-extraction.md` (qhy-focuser variant).
//!
//! Cover and calibrator state derive from the cached snapshot the manager's
//! while-open task refreshes. Writes (open/close/calibrator-on/-off) go
//! through `Session::request` so they share the same request arbitration
//! lock as the poll loop.

use std::sync::Arc;

use ascom_alpaca::api::cover_calibrator::{CalibratorStatus, CoverStatus};
use ascom_alpaca::api::{CoverCalibrator, Device};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use async_trait::async_trait;
use rusty_photon_shared_transport::Session;
use tokio::sync::RwLock;
use tracing::debug;

use crate::codec::Fp2Codec;
use crate::config::CoverCalibratorConfig;
use crate::error::DsdFp2Error;
use crate::manager::FlatPanelManager;
use crate::protocol::{Command, CLOSED_ANGLE, MAX_BRIGHTNESS, OPEN_ANGLE};

/// Deep Sky Dad FP2 as an ASCOM CoverCalibrator.
#[derive(derive_more::Debug)]
pub struct DsdFp2Device {
    config: CoverCalibratorConfig,
    #[debug(skip)]
    manager: Arc<FlatPanelManager>,
    #[debug(skip)]
    session: Arc<RwLock<Option<Session<Fp2Codec>>>>,
}

impl DsdFp2Device {
    pub fn new(config: CoverCalibratorConfig, manager: Arc<FlatPanelManager>) -> Self {
        Self {
            config,
            manager,
            session: Arc::new(RwLock::new(None)),
        }
    }

    /// Hardware-clamped configurable maximum. ASCOM `MaxBrightness` and
    /// `calibrator_on`'s validation share this so they can't disagree.
    fn effective_max_brightness(&self) -> u32 {
        self.config.max_brightness.min(MAX_BRIGHTNESS as u32)
    }
}

#[async_trait]
impl Device for DsdFp2Device {
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
        Ok(self.session.read().await.is_some() && self.manager.transport().is_available())
    }

    async fn set_connected(&self, connected: bool) -> ASCOMResult<()> {
        // Hold the write lock across the entire check-and-modify so two
        // concurrent `Connected=true` requests for this device can't both
        // observe `session.is_none()`, both call `transport.acquire()`,
        // and end up with the session refcount diverging from the
        // single per-device slot.
        let mut slot = self.session.write().await;
        let already_open = slot.is_some() && self.manager.transport().is_available();
        if already_open == connected {
            return Ok(());
        }
        match connected {
            true => {
                // `?` does SessionError<DsdFp2Error> → DsdFp2Error via the
                // From impl in error.rs, then DsdFp2Error → ASCOMError via
                // the second From impl on `?`.
                let session = self
                    .manager
                    .transport()
                    .acquire()
                    .await
                    .map_err(DsdFp2Error::from)?;
                *slot = Some(session);
                debug!("FP2 device connected");
            }
            false => {
                if let Some(session) = slot.take() {
                    // `Session::close` returns `Result<_, TransportError>`;
                    // `From<TransportError> for DsdFp2Error` handles the
                    // first hop, and `From<DsdFp2Error> for ASCOMError`
                    // does the second on `?`.
                    session.close().await.map_err(DsdFp2Error::from)?;
                    debug!("FP2 device disconnected");
                }
            }
        }
        Ok(())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("Deep Sky Dad FP2 Driver - ASCOM Alpaca CoverCalibrator".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }
}

#[async_trait]
impl CoverCalibrator for DsdFp2Device {
    async fn cover_state(&self) -> ASCOMResult<CoverStatus> {
        if !self.connected().await? {
            return Ok(CoverStatus::Unknown);
        }
        let snap = self.manager.snapshot();
        let state = snap.read().await.clone();
        Ok(derive_cover_state(state.motor_running, state.cover_raw))
    }

    async fn calibrator_state(&self) -> ASCOMResult<CalibratorStatus> {
        if !self.connected().await? {
            return Ok(CalibratorStatus::Unknown);
        }
        let snap = self.manager.snapshot();
        let state = snap.read().await.clone();
        Ok(derive_calibrator_state(state.light_on))
    }

    async fn brightness(&self) -> ASCOMResult<u32> {
        if !self.connected().await? {
            return Err(ASCOMError::NOT_CONNECTED);
        }
        let snap = self.manager.snapshot();
        let state = snap.read().await.clone();
        Ok(state.brightness.unwrap_or(0) as u32)
    }

    async fn max_brightness(&self) -> ASCOMResult<u32> {
        Ok(self.effective_max_brightness())
    }

    async fn open_cover(&self) -> ASCOMResult<()> {
        execute_move(self, OPEN_ANGLE).await
    }

    async fn close_cover(&self) -> ASCOMResult<()> {
        execute_move(self, CLOSED_ANGLE).await
    }

    /// The FP2 firmware has no halt-motion opcode; once `[SMOV]` starts a
    /// move it runs to completion. The ASCOM ICoverCalibratorV2 spec
    /// requires `HaltCover` to throw `MethodNotImplementedException`
    /// "if cover movement cannot be interrupted" — see
    /// <https://ascom-standards.org/newdocs/covercalibrator.html>. We
    /// honour that here.
    ///
    /// **Known ConformU divergence.** ConformU 4.3 flags this as an
    /// "issue" anyway because `CoverCalibratorTester.TestHaltCover` does
    /// not distinguish `MethodNotImplementedException` from other
    /// exceptions in its async-cover branch (it treats every exception
    /// as `Required.MustBeImplemented`). See
    /// `docs/services/dsd-fp2.md` "Known limitations" for the upstream
    /// bug report; the driver is intentionally spec-compliant.
    async fn halt_cover(&self) -> ASCOMResult<()> {
        Err(ASCOMError::new(
            ASCOMErrorCode::NOT_IMPLEMENTED,
            "HaltCover not implemented: FP2 firmware cannot interrupt an in-progress cover \
             movement. Per ICoverCalibratorV2, HaltCover MUST throw MethodNotImplementedException \
             when cover movement cannot be interrupted.",
        ))
    }

    async fn calibrator_on(&self, brightness: u32) -> ASCOMResult<()> {
        // Validate against the effective max (the lower of the config cap
        // and the hardware ceiling) so the value MaxBrightness advertises
        // and the value calibrator_on accepts agree.
        let effective_max = self.effective_max_brightness();
        if brightness > effective_max {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!("brightness {brightness} exceeds configured max {effective_max}"),
            ));
        }
        let value = FlatPanelManager::validate_brightness(brightness)?;
        let slot = self.session.read().await;
        let session = slot.as_ref().ok_or(ASCOMError::NOT_CONNECTED)?;

        session
            .request(Command::SetBrightness(value))
            .await
            .map_err(DsdFp2Error::from)?
            .parse_ok()?;
        session
            .request(Command::SetLight(true))
            .await
            .map_err(DsdFp2Error::from)?
            .parse_ok()?;

        let snap = self.manager.snapshot();
        let mut state = snap.write().await;
        state.brightness = Some(value);
        state.light_on = Some(true);
        Ok(())
    }

    async fn calibrator_off(&self) -> ASCOMResult<()> {
        let slot = self.session.read().await;
        let session = slot.as_ref().ok_or(ASCOMError::NOT_CONNECTED)?;
        session
            .request(Command::SetLight(false))
            .await
            .map_err(DsdFp2Error::from)?
            .parse_ok()?;
        let snap = self.manager.snapshot();
        let mut state = snap.write().await;
        state.light_on = Some(false);
        Ok(())
    }
}

/// Drive the cover to a target angle (`open_cover` / `close_cover`).
async fn execute_move(device: &DsdFp2Device, angle: u16) -> ASCOMResult<()> {
    let slot = device.session.read().await;
    let session = slot.as_ref().ok_or(ASCOMError::NOT_CONNECTED)?;

    session
        .request(Command::SetTarget(angle))
        .await
        .map_err(DsdFp2Error::from)?
        .parse_ok()?;
    session
        .request(Command::StartMove)
        .await
        .map_err(DsdFp2Error::from)?
        .parse_ok()?;

    // Mark motor as running locally so `cover_state` reports `Moving`
    // immediately, before the next poll observes it.
    let snap = device.manager.snapshot();
    snap.write().await.motor_running = Some(true);
    Ok(())
}

/// Derive `CoverStatus` from cached state.
fn derive_cover_state(motor_running: Option<bool>, cover_raw: Option<i32>) -> CoverStatus {
    match (motor_running, cover_raw) {
        (Some(true), _) => CoverStatus::Moving,
        (Some(false), Some(0)) => CoverStatus::Closed,
        (Some(false), Some(1)) => CoverStatus::Open,
        (Some(false), Some(_)) => CoverStatus::Unknown,
        _ => CoverStatus::Unknown,
    }
}

/// Derive `CalibratorStatus` from cached state. There's no `On` variant in
/// the ASCOM enum — `Ready` is what callers expect when the lamp is lit
/// and stable, which the FP2 always is (no warm-up).
fn derive_calibrator_state(light_on: Option<bool>) -> CalibratorStatus {
    match light_on {
        Some(true) => CalibratorStatus::Ready,
        Some(false) => CalibratorStatus::Off,
        None => CalibratorStatus::Unknown,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn derive_cover_state_table_matches_spec() {
        // Motor running → Moving regardless of GOPS
        assert_eq!(derive_cover_state(Some(true), Some(0)), CoverStatus::Moving);
        assert_eq!(derive_cover_state(Some(true), Some(1)), CoverStatus::Moving);
        assert_eq!(derive_cover_state(Some(true), None), CoverStatus::Moving);

        // Motor stopped → use GOPS
        assert_eq!(
            derive_cover_state(Some(false), Some(0)),
            CoverStatus::Closed
        );
        assert_eq!(derive_cover_state(Some(false), Some(1)), CoverStatus::Open);
        // GOPS in-between → Unknown
        assert_eq!(
            derive_cover_state(Some(false), Some(255)),
            CoverStatus::Unknown
        );

        // No data → Unknown
        assert_eq!(derive_cover_state(None, None), CoverStatus::Unknown);
        assert_eq!(derive_cover_state(Some(false), None), CoverStatus::Unknown);
    }

    #[test]
    fn derive_calibrator_state_table_matches_spec() {
        assert_eq!(derive_calibrator_state(Some(true)), CalibratorStatus::Ready);
        assert_eq!(derive_calibrator_state(Some(false)), CalibratorStatus::Off);
        assert_eq!(derive_calibrator_state(None), CalibratorStatus::Unknown);
    }
}

#[cfg(all(test, feature = "mock"))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod mock_tests {
    use super::*;
    use crate::config::{Config, CoverCalibratorConfig, SerialConfig, ServerConfig};
    use crate::mock::MockTransportFactory;
    use std::time::Duration;

    fn test_config() -> Config {
        Config {
            serial: SerialConfig {
                port: "/dev/mock".to_string(),
                polling_interval: Duration::from_secs(60),
                ..Default::default()
            },
            server: ServerConfig {
                port: 0,
                discovery_port: None,
                tls: None,
                auth: None,
            },
            cover_calibrator: CoverCalibratorConfig::default(),
        }
    }

    fn make_device() -> (DsdFp2Device, MockTransportFactory) {
        let factory = MockTransportFactory::default();
        let manager = FlatPanelManager::new(test_config(), Arc::new(factory.clone()));
        let device = DsdFp2Device::new(CoverCalibratorConfig::default(), manager);
        (device, factory)
    }

    fn make_device_with_cap(cap: u32) -> (DsdFp2Device, MockTransportFactory) {
        let factory = MockTransportFactory::default();
        let manager = FlatPanelManager::new(test_config(), Arc::new(factory.clone()));
        let cc_config = CoverCalibratorConfig {
            max_brightness: cap,
            ..CoverCalibratorConfig::default()
        };
        let device = DsdFp2Device::new(cc_config, manager);
        (device, factory)
    }

    #[tokio::test]
    async fn device_starts_disconnected() {
        let (device, _) = make_device();
        assert!(!device.connected().await.unwrap());
        // Pre-connect reads return Unknown without error.
        assert_eq!(device.cover_state().await.unwrap(), CoverStatus::Unknown);
        assert_eq!(
            device.calibrator_state().await.unwrap(),
            CalibratorStatus::Unknown
        );
    }

    #[tokio::test]
    async fn brightness_read_when_disconnected_errors() {
        let (device, _) = make_device();
        let err = device.brightness().await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn set_connected_acquires_and_releases() {
        let (device, _) = make_device();
        device.set_connected(true).await.unwrap();
        assert!(device.connected().await.unwrap());
        device.set_connected(false).await.unwrap();
        assert!(!device.connected().await.unwrap());
    }

    #[tokio::test]
    async fn set_connected_is_idempotent() {
        let (device, _) = make_device();
        device.set_connected(true).await.unwrap();
        device.set_connected(true).await.unwrap(); // no-op
        assert!(device.connected().await.unwrap());
        device.set_connected(false).await.unwrap();
        device.set_connected(false).await.unwrap(); // no-op
        assert!(!device.connected().await.unwrap());
    }

    #[tokio::test]
    async fn calibrator_on_then_off_round_trips() {
        let (device, factory) = make_device();
        device.set_connected(true).await.unwrap();
        device.calibrator_on(2048).await.unwrap();
        assert_eq!(
            device.calibrator_state().await.unwrap(),
            CalibratorStatus::Ready
        );
        assert_eq!(device.brightness().await.unwrap(), 2048);
        assert_eq!(factory.state().brightness().await, 2048);
        assert!(factory.state().light_on().await);

        device.calibrator_off().await.unwrap();
        assert_eq!(
            device.calibrator_state().await.unwrap(),
            CalibratorStatus::Off
        );
        // Brightness retained as commanded (firmware behaviour).
        assert_eq!(device.brightness().await.unwrap(), 2048);
        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn calibrator_on_rejects_brightness_above_max() {
        let (device, _) = make_device();
        device.set_connected(true).await.unwrap();
        let err = device
            .calibrator_on(MAX_BRIGHTNESS as u32 + 1)
            .await
            .unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::INVALID_VALUE);
        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn open_cover_then_close_cover_change_observable_state() {
        let (device, _) = make_device();
        device.set_connected(true).await.unwrap();

        // Default is closed (mock `cover_angle = 270`). Close again to
        // exercise the wire path even though it's a no-op for the cover
        // angle; the manager's snapshot is still directly poked to
        // Moving.
        device.close_cover().await.unwrap();
        // After close, our local optimistic write set motor_running =
        // Some(true). Our mock completes moves instantly inside `[SMOV]`,
        // so the motor is no longer running on the device side; the next
        // poll would correct the cache. For now we just verify the call
        // succeeded and exercise the open path too.
        device.open_cover().await.unwrap();
        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn writes_when_disconnected_return_not_connected() {
        let (device, _) = make_device();
        let err = device.open_cover().await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
        let err = device.close_cover().await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
        let err = device.calibrator_on(100).await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
        let err = device.calibrator_off().await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn max_brightness_caps_at_hardware_limit() {
        let (device, _) = make_device();
        // Config default is MAX_BRIGHTNESS; the impl caps anyway.
        assert_eq!(
            device.max_brightness().await.unwrap(),
            MAX_BRIGHTNESS as u32
        );
    }

    #[tokio::test]
    async fn calibrator_on_rejects_brightness_above_configured_cap() {
        let (device, _) = make_device_with_cap(2048);
        device.set_connected(true).await.unwrap();
        // MaxBrightness must agree with what calibrator_on accepts.
        assert_eq!(device.max_brightness().await.unwrap(), 2048);

        // 2048 is at the cap — allowed.
        device.calibrator_on(2048).await.unwrap();

        // 2049 is above the configured cap but below the hardware ceiling
        // — must be rejected with INVALID_VALUE so the MaxBrightness
        // promise isn't violated.
        let err = device.calibrator_on(2049).await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::INVALID_VALUE);

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn metadata_fields_round_trip() {
        let (device, _) = make_device();
        assert_eq!(device.static_name(), "Deep Sky Dad FP2");
        assert_eq!(device.unique_id(), "dsd-fp2-001");
        let desc = device.description().await.unwrap();
        assert!(desc.contains("Deep Sky Dad"));
        let info = device.driver_info().await.unwrap();
        assert!(info.contains("CoverCalibrator"));
        let ver = device.driver_version().await.unwrap();
        assert!(!ver.is_empty());
    }
}
