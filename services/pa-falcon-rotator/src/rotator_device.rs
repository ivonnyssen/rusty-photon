//! Falcon Rotator ASCOM device implementation
//!
//! Wraps `SerialManager` behind the ASCOM `Device` + `Rotator` traits. Every
//! property read maps to one serial command — see the design doc's
//! [Why no cache](../../../docs/services/falcon-rotator.md#why-no-cache)
//! section for the rationale.

use std::fmt;
use std::sync::Arc;

use ascom_alpaca::api::{Device, Rotator};
use ascom_alpaca::{ASCOMError, ASCOMResult};
use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::debug;

use crate::config::RotatorConfig;
use crate::error::FalconRotatorError;
use crate::serial_manager::SerialManager;

/// Normalise a degree value into `[0.0, 360.0)`.
fn normalise_deg(deg: f64) -> f64 {
    ((deg % 360.0) + 360.0) % 360.0
}

/// Guard macro that returns NOT_CONNECTED if the device is not connected.
///
/// Mirrors the qhy-focuser / ppba-driver / switch_device pattern: a single
/// `ensure_connected!` line at the top of each device-bound method so
/// disconnected reads/writes never reach the SerialManager.
macro_rules! ensure_connected {
    ($self:ident) => {
        if !$self.connected().await.is_ok_and(|connected| connected) {
            debug!("Falcon Rotator device not connected");
            return Err(ASCOMError::NOT_CONNECTED);
        }
    };
}

/// Falcon Rotator device for ASCOM Alpaca.
pub struct FalconRotatorDevice {
    config: RotatorConfig,
    requested_connection: Arc<RwLock<bool>>,
    serial_manager: Arc<SerialManager>,
}

impl fmt::Debug for FalconRotatorDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FalconRotatorDevice")
            .field("config", &self.config)
            .field("requested_connection", &self.requested_connection)
            .finish_non_exhaustive()
    }
}

impl FalconRotatorDevice {
    pub fn new(config: RotatorConfig, serial_manager: Arc<SerialManager>) -> Self {
        Self {
            config,
            requested_connection: Arc::new(RwLock::new(false)),
            serial_manager,
        }
    }

    fn to_ascom_error(err: FalconRotatorError) -> ASCOMError {
        err.to_ascom_error()
    }
}

#[async_trait]
impl Device for FalconRotatorDevice {
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
        let serial_ok = self.serial_manager.is_available();
        Ok(requested && serial_ok)
    }

    async fn set_connected(&self, connected: bool) -> ASCOMResult<()> {
        if self.connected().await? == connected {
            return Ok(());
        }
        match connected {
            true => {
                self.serial_manager
                    .connect()
                    .await
                    .map_err(Self::to_ascom_error)?;
                *self.requested_connection.write().await = true;
            }
            false => {
                *self.requested_connection.write().await = false;
                self.serial_manager.disconnect().await;
            }
        }
        Ok(())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("Pegasus Falcon Rotator Driver - ASCOM Alpaca interface".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }
}

#[async_trait]
impl Rotator for FalconRotatorDevice {
    async fn can_reverse(&self) -> ASCOMResult<bool> {
        Ok(true)
    }

    async fn is_moving(&self) -> ASCOMResult<bool> {
        ensure_connected!(self);
        let status = self
            .serial_manager
            .read_status()
            .await
            .map_err(Self::to_ascom_error)?;
        Ok(status.is_moving)
    }

    async fn position(&self) -> ASCOMResult<f64> {
        ensure_connected!(self);
        let status = self
            .serial_manager
            .read_status()
            .await
            .map_err(Self::to_ascom_error)?;
        let offset = self.serial_manager.sync_offset().await;
        Ok(normalise_deg(status.position_deg + offset))
    }

    async fn mechanical_position(&self) -> ASCOMResult<f64> {
        ensure_connected!(self);
        let status = self
            .serial_manager
            .read_status()
            .await
            .map_err(Self::to_ascom_error)?;
        Ok(normalise_deg(status.position_deg))
    }

    async fn target_position(&self) -> ASCOMResult<f64> {
        ensure_connected!(self);
        if let Some(target) = self.serial_manager.target_position().await {
            return Ok(target);
        }
        // No move outstanding: fall back to current Position. Matches the
        // design doc's TargetPosition row and the dominant ASCOM convention.
        let status = self
            .serial_manager
            .read_status()
            .await
            .map_err(Self::to_ascom_error)?;
        let offset = self.serial_manager.sync_offset().await;
        Ok(normalise_deg(status.position_deg + offset))
    }

    async fn reverse(&self) -> ASCOMResult<bool> {
        ensure_connected!(self);
        let status = self
            .serial_manager
            .read_status()
            .await
            .map_err(Self::to_ascom_error)?;
        Ok(status.motor_reverse)
    }

    async fn set_reverse(&self, reverse: bool) -> ASCOMResult<()> {
        ensure_connected!(self);
        self.serial_manager
            .set_reverse(reverse)
            .await
            .map_err(Self::to_ascom_error)
    }

    async fn step_size(&self) -> ASCOMResult<f64> {
        // Vendor product page: 86.6 steps per degree → 1.0 / 86.6 ≈ 0.01155°.
        Ok(0.01155)
    }

    async fn halt(&self) -> ASCOMResult<()> {
        ensure_connected!(self);
        self.serial_manager
            .halt()
            .await
            .map_err(Self::to_ascom_error)
    }

    async fn move_(&self, position: f64) -> ASCOMResult<()> {
        ensure_connected!(self);
        if !position.is_finite() {
            return Err(FalconRotatorError::InvalidValue(format!(
                "Move delta must be finite, got {position}"
            ))
            .to_ascom_error());
        }
        let status = self
            .serial_manager
            .read_status()
            .await
            .map_err(Self::to_ascom_error)?;
        let offset = self.serial_manager.sync_offset().await;
        // ASCOM `Move(delta)` is in sky coordinates: the new sky position is
        // (mech + offset) + delta, and the mechanical wire value is therefore
        // (mech + offset + delta) - offset = mech + delta.
        let target_sky = normalise_deg(status.position_deg + offset + position);
        let target_mech = normalise_deg(target_sky - offset);
        self.serial_manager.set_target_position(target_sky).await;
        self.serial_manager
            .move_mechanical(target_mech)
            .await
            .map_err(Self::to_ascom_error)
    }

    async fn move_absolute(&self, position: f64) -> ASCOMResult<()> {
        ensure_connected!(self);
        if !position.is_finite() {
            return Err(FalconRotatorError::InvalidValue(format!(
                "MoveAbsolute target must be finite, got {position}"
            ))
            .to_ascom_error());
        }
        let offset = self.serial_manager.sync_offset().await;
        let target_sky = normalise_deg(position);
        let target_mech = normalise_deg(position - offset);
        self.serial_manager.set_target_position(target_sky).await;
        self.serial_manager
            .move_mechanical(target_mech)
            .await
            .map_err(Self::to_ascom_error)
    }

    async fn move_mechanical(&self, position: f64) -> ASCOMResult<()> {
        ensure_connected!(self);
        if !position.is_finite() {
            return Err(FalconRotatorError::InvalidValue(format!(
                "MoveMechanical target must be finite, got {position}"
            ))
            .to_ascom_error());
        }
        // Per the design-doc mapping table, MoveMechanical does NOT subtract
        // the sync offset from the wire value — the caller asked for a
        // specific mechanical angle. Internal target_position is still stored
        // in sky coordinates so subsequent TargetPosition reads stay frame-
        // consistent across the three Move variants.
        let offset = self.serial_manager.sync_offset().await;
        let target_sky = normalise_deg(position + offset);
        self.serial_manager.set_target_position(target_sky).await;
        self.serial_manager
            .move_mechanical(normalise_deg(position))
            .await
            .map_err(Self::to_ascom_error)
    }

    async fn sync(&self, position: f64) -> ASCOMResult<()> {
        ensure_connected!(self);
        // SerialManager::sync validates finiteness, reads FA, and computes
        // the driver-side offset. ASCOM Sync never touches the device; the
        // SD wire command is deliberately absent from the protocol enum.
        self.serial_manager
            .sync(position)
            .await
            .map_err(Self::to_ascom_error)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use async_trait::async_trait;

    use super::*;
    use crate::config::Config;
    use crate::io::{SerialPair, SerialPortFactory};

    /// Test-only no-op factory: the connection-guard tests assert behaviour
    /// while disconnected, so `open` is never called.
    struct NoopFactory;

    #[async_trait]
    impl SerialPortFactory for NoopFactory {
        async fn open(
            &self,
            _port: &str,
            _baud_rate: u32,
            _timeout: Duration,
        ) -> crate::error::Result<SerialPair> {
            unimplemented!("test factory should never open a port")
        }

        async fn port_exists(&self, _port: &str) -> bool {
            true
        }
    }

    fn disconnected_device() -> FalconRotatorDevice {
        let config = Config::default();
        let manager = Arc::new(SerialManager::new(config, Arc::new(NoopFactory)));
        FalconRotatorDevice::new(RotatorConfig::default(), manager)
    }

    #[tokio::test]
    async fn can_reverse_is_always_true() {
        let device = disconnected_device();
        assert!(device.can_reverse().await.unwrap());
    }

    #[tokio::test]
    async fn step_size_matches_vendor_product_page() {
        let device = disconnected_device();
        let step = device.step_size().await.unwrap();
        assert!((step - 0.01155).abs() < 1e-9);
    }

    #[tokio::test]
    async fn static_name_comes_from_config() {
        let device = disconnected_device();
        assert_eq!(device.static_name(), "Pegasus Falcon Rotator");
    }

    #[tokio::test]
    async fn unique_id_comes_from_config() {
        let device = disconnected_device();
        assert_eq!(device.unique_id(), "pa-falcon-rotator-001");
    }

    #[tokio::test]
    async fn driver_version_matches_cargo_pkg_version() {
        let device = disconnected_device();
        assert_eq!(
            device.driver_version().await.unwrap(),
            env!("CARGO_PKG_VERSION")
        );
    }

    // ---- Disconnected guards: each method must short-circuit with NOT_CONNECTED.

    #[tokio::test]
    async fn position_when_disconnected_returns_not_connected() {
        let device = disconnected_device();
        let err = device.position().await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn mechanical_position_when_disconnected_returns_not_connected() {
        let device = disconnected_device();
        let err = device.mechanical_position().await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn target_position_when_disconnected_returns_not_connected() {
        let device = disconnected_device();
        let err = device.target_position().await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn is_moving_when_disconnected_returns_not_connected() {
        let device = disconnected_device();
        let err = device.is_moving().await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn reverse_get_when_disconnected_returns_not_connected() {
        let device = disconnected_device();
        let err = device.reverse().await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn set_reverse_when_disconnected_returns_not_connected() {
        let device = disconnected_device();
        let err = device.set_reverse(true).await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn halt_when_disconnected_returns_not_connected() {
        let device = disconnected_device();
        let err = device.halt().await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn move_when_disconnected_returns_not_connected() {
        let device = disconnected_device();
        let err = device.move_(10.0).await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn move_absolute_when_disconnected_returns_not_connected() {
        let device = disconnected_device();
        let err = device.move_absolute(45.0).await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn move_mechanical_when_disconnected_returns_not_connected() {
        let device = disconnected_device();
        let err = device.move_mechanical(45.0).await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn sync_when_disconnected_returns_not_connected() {
        let device = disconnected_device();
        let err = device.sync(45.0).await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
    }
}

/// Mock-backed integration tests for the rotator device. Exercises the
/// arithmetic and command routing through `SerialManager` against the
/// deterministic `MockSerialPortFactory`.
#[cfg(all(test, feature = "mock"))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod mock_tests {
    use super::*;
    use crate::config::Config;
    use crate::io::SerialPortFactory;
    use crate::mock::MockSerialPortFactory;

    fn connected_device_with(
        factory: Arc<MockSerialPortFactory>,
    ) -> (
        FalconRotatorDevice,
        Arc<SerialManager>,
        Arc<MockSerialPortFactory>,
    ) {
        let config = Config::default();
        let manager = Arc::new(SerialManager::new(
            config,
            Arc::clone(&factory) as Arc<dyn SerialPortFactory>,
        ));
        let device = FalconRotatorDevice::new(RotatorConfig::default(), Arc::clone(&manager));
        (device, manager, factory)
    }

    #[tokio::test]
    async fn move_absolute_subtracts_sync_offset_from_wire_value() {
        let factory = Arc::new(MockSerialPortFactory::default());
        let (device, _manager, factory) = connected_device_with(factory);
        device.set_connected(true).await.unwrap();
        factory.set_mech_position_deg(0.0).await;
        device.sync(255.20).await.unwrap(); // sync offset = (255.20 - 0) mod 360 = 255.20
        factory.clear_command_log().await;

        device.move_absolute(180.0).await.unwrap();
        // target_mech = (180 - 255.20) mod 360 = -75.20 mod 360 = 284.80
        let log = factory.command_log().await;
        assert!(log.contains(&"MD:284.80".to_string()), "got: {log:?}");
    }

    #[tokio::test]
    async fn move_mechanical_does_not_subtract_sync_offset() {
        let factory = Arc::new(MockSerialPortFactory::default());
        let (device, _manager, factory) = connected_device_with(factory);
        device.set_connected(true).await.unwrap();
        factory.set_mech_position_deg(0.0).await;
        device.sync(255.20).await.unwrap();
        factory.clear_command_log().await;

        device.move_mechanical(90.0).await.unwrap();
        let log = factory.command_log().await;
        assert!(log.contains(&"MD:90.00".to_string()), "got: {log:?}");
    }

    #[tokio::test]
    async fn move_with_relative_delta_targets_current_mech_plus_delta() {
        let factory = Arc::new(MockSerialPortFactory::default());
        let (device, _manager, factory) = connected_device_with(factory);
        device.set_connected(true).await.unwrap();
        factory.set_mech_position_deg(350.0).await;
        device.sync(320.0).await.unwrap(); // sync offset = (320 - 350) mod 360 = 330
        factory.clear_command_log().await;

        device.move_(20.0).await.unwrap();
        // target_sky = (350 + 330 + 20) mod 360 = 700 mod 360 = 340
        // target_mech = (340 - 330) mod 360 = 10
        let log = factory.command_log().await;
        assert!(log.contains(&"MD:10.00".to_string()), "got: {log:?}");
    }

    #[tokio::test]
    async fn target_position_after_move_returns_requested_sky_angle() {
        let factory = Arc::new(MockSerialPortFactory::default());
        let (device, _manager, factory) = connected_device_with(factory);
        device.set_connected(true).await.unwrap();
        factory.set_mech_position_deg(0.0).await;
        device.sync(45.0).await.unwrap();

        device.move_absolute(180.0).await.unwrap();

        let target = device.target_position().await.unwrap();
        assert!((target - 180.0).abs() < 1e-9, "got {target}");
    }

    #[tokio::test]
    async fn target_position_with_no_move_falls_back_to_position() {
        let factory = Arc::new(MockSerialPortFactory::default());
        let (device, _manager, factory) = connected_device_with(factory);
        device.set_connected(true).await.unwrap();
        factory.set_mech_position_deg(90.0).await;

        let target = device.target_position().await.unwrap();
        assert!((target - 90.0).abs() < 1e-9, "got {target}");
    }

    #[tokio::test]
    async fn position_adds_sync_offset_to_mechanical() {
        let factory = Arc::new(MockSerialPortFactory::default());
        let (device, _manager, factory) = connected_device_with(factory);
        device.set_connected(true).await.unwrap();
        factory.set_mech_position_deg(142.30).await;
        device.sync(37.50).await.unwrap();

        let pos = device.position().await.unwrap();
        assert!((pos - 37.50).abs() < 1e-9, "got {pos}");
        let mech = device.mechanical_position().await.unwrap();
        assert!((mech - 142.30).abs() < 1e-9, "got {mech}");
    }

    #[tokio::test]
    async fn move_absolute_rejects_non_finite() {
        let factory = Arc::new(MockSerialPortFactory::default());
        let (device, _manager, _factory) = connected_device_with(factory);
        device.set_connected(true).await.unwrap();

        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let err = device.move_absolute(bad).await.unwrap_err();
            assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::INVALID_VALUE);
        }
    }

    #[tokio::test]
    async fn sync_rejects_non_finite() {
        let factory = Arc::new(MockSerialPortFactory::default());
        let (device, _manager, _factory) = connected_device_with(factory);
        device.set_connected(true).await.unwrap();

        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let err = device.sync(bad).await.unwrap_err();
            assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::INVALID_VALUE);
        }
    }

    #[tokio::test]
    async fn halt_sends_fh() {
        let factory = Arc::new(MockSerialPortFactory::default());
        let (device, _manager, factory) = connected_device_with(factory);
        device.set_connected(true).await.unwrap();
        factory.clear_command_log().await;

        device.halt().await.unwrap();
        let log = factory.command_log().await;
        assert!(log.contains(&"FH".to_string()), "got: {log:?}");
    }
}
