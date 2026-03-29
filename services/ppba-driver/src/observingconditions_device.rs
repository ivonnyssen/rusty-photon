//! PPBA ObservingConditions device implementation
//!
//! This module implements the ASCOM Alpaca ObservingConditions trait
//! for the Pegasus Astro Pocket Powerbox Advance Gen2 environmental sensors.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::{Device, ObservingConditions};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::debug;

use crate::config::ObservingConditionsConfig;
use crate::error::PpbaError;
use crate::serial_manager::SerialManager;

/// Guard macro that returns NOT_CONNECTED if the device is not connected.
macro_rules! ensure_connected {
    ($self:ident) => {
        if !$self.connected().await.is_ok_and(|connected| connected) {
            debug!("ObservingConditions device not connected");
            return Err(ASCOMError::NOT_CONNECTED);
        }
    };
}

/// PPBA ObservingConditions device for ASCOM Alpaca
pub struct PpbaObservingConditionsDevice {
    config: ObservingConditionsConfig,
    requested_connection: Arc<RwLock<bool>>,
    serial_manager: Arc<SerialManager>,
}

impl fmt::Debug for PpbaObservingConditionsDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PpbaObservingConditionsDevice")
            .field("config", &self.config)
            .field("requested_connection", &self.requested_connection)
            .finish_non_exhaustive()
    }
}

impl PpbaObservingConditionsDevice {
    /// Create a new PPBA ObservingConditions device
    pub fn new(config: ObservingConditionsConfig, serial_manager: Arc<SerialManager>) -> Self {
        Self {
            config,
            requested_connection: Arc::new(RwLock::new(false)),
            serial_manager,
        }
    }

    /// Convert internal error to ASCOM error
    fn to_ascom_error(err: PpbaError) -> ASCOMError {
        match err {
            PpbaError::NotConnected => {
                ASCOMError::new(ASCOMErrorCode::NOT_CONNECTED, err.to_string())
            }
            PpbaError::InvalidValue(_) => {
                ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, err.to_string())
            }
            _ => ASCOMError::invalid_operation(err.to_string()),
        }
    }
}

#[async_trait]
impl Device for PpbaObservingConditionsDevice {
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
                debug!("ObservingConditions device connected");
            }
            false => {
                *self.requested_connection.write().await = false;
                self.serial_manager.disconnect().await;
                debug!("ObservingConditions device disconnected");
            }
        }
        Ok(())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("PPBA Driver - ObservingConditions interface for Pegasus Astro PPBA Gen2 environmental sensors".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }
}

#[async_trait]
impl ObservingConditions for PpbaObservingConditionsDevice {
    async fn average_period(&self) -> ASCOMResult<f64> {
        ensure_connected!(self);
        let cached = self.serial_manager.get_cached_state().await;
        let window = cached.temp_mean.window();

        // If window is 10 seconds, we're in instantaneous mode - return 0.0
        if window == Duration::from_secs(10) {
            return Ok(0.0);
        }

        // ASCOM spec requires hours, not seconds
        Ok(window.as_secs_f64() / 3600.0)
    }

    async fn set_average_period(&self, period: f64) -> ASCOMResult<()> {
        ensure_connected!(self);
        // ASCOM spec requires hours. Must accept 0.0 for instantaneous readings.
        // Per spec: "All drivers must accept 0.0 to specify that an instantaneous value is available"

        // Reject negative values
        if period < 0.0 {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!("Average period cannot be negative, got {}", period),
            ));
        }

        // Set a reasonable upper limit (24 hours)
        if period > 24.0 {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!("Average period cannot exceed 24 hours, got {}", period),
            ));
        }

        // Convert hours to Duration
        // Special case: 0.0 means instantaneous (use small but reasonable averaging window)
        let duration = if period == 0.0 {
            // Use 10 seconds for instantaneous - enough to avoid aging out samples
            // immediately while still being effectively instantaneous for astronomy
            Duration::from_secs(10)
        } else {
            Duration::from_secs_f64(period * 3600.0) // Convert hours to seconds
        };

        // Update mean calculators in SerialManager
        self.serial_manager.set_averaging_period(duration).await;

        debug!("Average period set to {} hours", period);
        Ok(())
    }

    async fn temperature(&self) -> ASCOMResult<f64> {
        ensure_connected!(self);

        let state = self.serial_manager.get_cached_state().await;
        state.temp_mean.get_mean().ok_or_else(|| {
            ASCOMError::new(
                ASCOMErrorCode::VALUE_NOT_SET,
                "No temperature data available yet",
            )
        })
    }

    async fn humidity(&self) -> ASCOMResult<f64> {
        ensure_connected!(self);

        let state = self.serial_manager.get_cached_state().await;
        state.humidity_mean.get_mean().ok_or_else(|| {
            ASCOMError::new(
                ASCOMErrorCode::VALUE_NOT_SET,
                "No humidity data available yet",
            )
        })
    }

    async fn dew_point(&self) -> ASCOMResult<f64> {
        ensure_connected!(self);

        let state = self.serial_manager.get_cached_state().await;
        state.dewpoint_mean.get_mean().ok_or_else(|| {
            ASCOMError::new(
                ASCOMErrorCode::VALUE_NOT_SET,
                "No dewpoint data available yet",
            )
        })
    }

    async fn time_since_last_update(&self, sensor_name: String) -> ASCOMResult<f64> {
        ensure_connected!(self);

        let state = self.serial_manager.get_cached_state().await;

        let duration = match sensor_name.to_lowercase().as_str() {
            // Empty string means "latest update time" across all sensors
            "" => {
                // Return the most recent update time among all implemented sensors
                let times = [
                    state.temp_mean.time_since_last_update(),
                    state.humidity_mean.time_since_last_update(),
                    state.dewpoint_mean.time_since_last_update(),
                ];
                // Get the minimum time (most recent update)
                times
                    .iter()
                    .filter_map(|&t| t)
                    .min()
                    .or(Some(Duration::ZERO))
            }
            "temperature" => state.temp_mean.time_since_last_update(),
            "humidity" => state.humidity_mean.time_since_last_update(),
            "dewpoint" => state.dewpoint_mean.time_since_last_update(),
            // Unimplemented sensors - return NOT_IMPLEMENTED error
            "cloudcover" | "pressure" | "rainrate" | "skybrightness" | "skyquality"
            | "starfwhm" | "skytemperature" | "winddirection" | "windgust" | "windspeed" => {
                return Err(ASCOMError::NOT_IMPLEMENTED);
            }
            // Truly unknown sensor name
            _ => {
                return Err(ASCOMError::new(
                    ASCOMErrorCode::INVALID_VALUE,
                    format!("Unknown sensor name: {}", sensor_name),
                ))
            }
        };

        Ok(duration.map(|d| d.as_secs_f64()).unwrap_or(f64::MAX))
    }

    async fn sensor_description(&self, sensor_name: String) -> ASCOMResult<String> {
        ensure_connected!(self);
        match sensor_name.to_lowercase().as_str() {
            "temperature" => Ok("PPBA internal temperature sensor".to_string()),
            "humidity" => Ok("PPBA internal humidity sensor".to_string()),
            "dewpoint" => Ok("Dewpoint calculated from temperature and humidity".to_string()),
            // Empty string is an invalid sensor name
            "" => Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                "Sensor name cannot be empty".to_string(),
            )),
            // Unimplemented sensors - return NOT_IMPLEMENTED error
            "cloudcover" | "pressure" | "rainrate" | "skybrightness" | "skyquality"
            | "starfwhm" | "skytemperature" | "winddirection" | "windgust" | "windspeed" => {
                Err(ASCOMError::NOT_IMPLEMENTED)
            }
            // Truly unknown sensor name
            _ => Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!("Unknown sensor name: {}", sensor_name),
            )),
        }
    }

    async fn refresh(&self) -> ASCOMResult<()> {
        ensure_connected!(self);

        // Trigger immediate refresh via SerialManager
        self.serial_manager
            .refresh_status()
            .await
            .map_err(Self::to_ascom_error)?;
        debug!("ObservingConditions sensors refreshed");
        Ok(())
    }

    // All other sensors are not implemented and use default trait implementations
    // which return NOT_IMPLEMENTED errors
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    //! Unit tests for PpbaObservingConditionsDevice ASCOM error mapping and edge cases
    //!
    //! These tests exercise error paths in the ObservingConditions device that are
    //! only reachable through internal failures (factory errors) or specific invalid
    //! inputs, covering `to_ascom_error` branches and the Debug implementation.

    use super::*;
    use crate::config::Config;
    use crate::error::PpbaError;
    use crate::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};
    use crate::serial_manager::SerialManager;
    use ascom_alpaca::api::{Device, ObservingConditions};
    use ascom_alpaca::ASCOMErrorCode;
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Mutex;

    // ============================================================================
    // Mock Serial Infrastructure
    // ============================================================================

    struct MockSerialReader {
        responses: Arc<Mutex<Vec<String>>>,
        index: Arc<Mutex<usize>>,
    }

    impl MockSerialReader {
        fn new(responses: Vec<String>) -> Self {
            Self {
                responses: Arc::new(Mutex::new(responses)),
                index: Arc::new(Mutex::new(0)),
            }
        }
    }

    #[async_trait]
    impl SerialReader for MockSerialReader {
        async fn read_line(&mut self) -> crate::error::Result<Option<String>> {
            let responses = self.responses.lock().await;
            let mut index = self.index.lock().await;
            if *index < responses.len() {
                let response = responses[*index].clone();
                *index += 1;
                Ok(Some(response))
            } else {
                *index = 0;
                if !responses.is_empty() {
                    Ok(Some(responses[0].clone()))
                } else {
                    Ok(None)
                }
            }
        }
    }

    struct MockSerialWriter;

    #[async_trait]
    impl SerialWriter for MockSerialWriter {
        async fn write_message(&mut self, _message: &str) -> crate::error::Result<()> {
            Ok(())
        }
    }

    struct MockSerialPortFactory {
        responses: Vec<String>,
    }

    impl MockSerialPortFactory {
        fn new(responses: Vec<String>) -> Self {
            Self { responses }
        }
    }

    #[async_trait]
    impl SerialPortFactory for MockSerialPortFactory {
        async fn open(
            &self,
            _port: &str,
            _baud_rate: u32,
            _timeout: Duration,
        ) -> crate::error::Result<SerialPair> {
            Ok(SerialPair {
                reader: Box::new(MockSerialReader::new(self.responses.clone())),
                writer: Box::new(MockSerialWriter),
            })
        }

        async fn port_exists(&self, _port: &str) -> bool {
            true
        }
    }

    struct FailingMockSerialPortFactory;

    #[async_trait]
    impl SerialPortFactory for FailingMockSerialPortFactory {
        async fn open(
            &self,
            _port: &str,
            _baud_rate: u32,
            _timeout: Duration,
        ) -> crate::error::Result<SerialPair> {
            Err(PpbaError::ConnectionFailed(
                "Mock factory error".to_string(),
            ))
        }

        async fn port_exists(&self, _port: &str) -> bool {
            false
        }
    }

    fn standard_connection_responses() -> Vec<String> {
        vec![
            "PPBA_OK".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
            "PS:2.5:10.5:126.0:3600000".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
            "PS:2.5:10.5:126.0:3600000".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
            "PS:2.5:10.5:126.0:3600000".to_string(),
        ]
    }

    fn create_oc_device(factory: Arc<dyn SerialPortFactory>) -> PpbaObservingConditionsDevice {
        let config = Config::default();
        let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));
        PpbaObservingConditionsDevice::new(config.observingconditions, serial_manager)
    }

    // ============================================================================
    // Connection Error Mapping Tests
    // ============================================================================

    #[tokio::test]
    async fn test_oc_connect_factory_error_maps_to_invalid_operation() {
        let device = create_oc_device(Arc::new(FailingMockSerialPortFactory));

        let err = device.set_connected(true).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
        assert!(err.message.contains("Connection failed"));
    }

    #[tokio::test]
    async fn test_oc_connect_bad_ping_maps_to_invalid_operation() {
        let factory = Arc::new(MockSerialPortFactory::new(vec!["BAD_RESPONSE".to_string()]));
        let device = create_oc_device(factory);

        let err = device.set_connected(true).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_OPERATION);
    }

    // ============================================================================
    // Not Connected Guard Tests
    // ============================================================================

    #[tokio::test]
    async fn test_oc_operations_fail_when_not_connected() {
        let factory = Arc::new(MockSerialPortFactory::new(vec![]));
        let device = create_oc_device(factory);

        assert_eq!(
            device.temperature().await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device.humidity().await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device.dew_point().await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device.average_period().await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device.set_average_period(1.0).await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device.refresh().await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device
                .time_since_last_update("temperature".to_string())
                .await
                .unwrap_err()
                .code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device
                .sensor_description("temperature".to_string())
                .await
                .unwrap_err()
                .code,
            ASCOMErrorCode::NOT_CONNECTED
        );
    }

    // ============================================================================
    // Average Period Validation Tests
    // ============================================================================

    #[tokio::test]
    async fn test_oc_set_average_period_negative_maps_to_invalid_value() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_oc_device(factory);
        device.set_connected(true).await.unwrap();

        let err = device.set_average_period(-1.0).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
        assert!(err.message.contains("negative"));

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn test_oc_set_average_period_too_large_maps_to_invalid_value() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_oc_device(factory);
        device.set_connected(true).await.unwrap();

        let err = device.set_average_period(25.0).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
        assert!(err.message.contains("24 hours"));

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn test_oc_set_average_period_zero_for_instantaneous() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_oc_device(factory);
        device.set_connected(true).await.unwrap();

        device.set_average_period(0.0).await.unwrap();
        let period = device.average_period().await.unwrap();
        // 0.0 means instantaneous mode
        assert!((period - 0.0).abs() < f64::EPSILON);

        device.set_connected(false).await.unwrap();
    }

    // ============================================================================
    // Sensor Description Tests
    // ============================================================================

    #[tokio::test]
    async fn test_oc_sensor_descriptions() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_oc_device(factory);
        device.set_connected(true).await.unwrap();

        let temp_desc = device
            .sensor_description("temperature".to_string())
            .await
            .unwrap();
        assert!(temp_desc.contains("temperature"));

        let humidity_desc = device
            .sensor_description("humidity".to_string())
            .await
            .unwrap();
        assert!(humidity_desc.contains("humidity"));

        let dewpoint_desc = device
            .sensor_description("dewpoint".to_string())
            .await
            .unwrap();
        assert!(dewpoint_desc.contains("ewpoint"));

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn test_oc_sensor_description_empty_name_invalid() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_oc_device(factory);
        device.set_connected(true).await.unwrap();

        let err = device.sensor_description("".to_string()).await.unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn test_oc_sensor_description_unimplemented_sensor() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_oc_device(factory);
        device.set_connected(true).await.unwrap();

        let err = device
            .sensor_description("pressure".to_string())
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::NOT_IMPLEMENTED);

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn test_oc_sensor_description_unknown_sensor() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_oc_device(factory);
        device.set_connected(true).await.unwrap();

        let err = device
            .sensor_description("foobar".to_string())
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);

        device.set_connected(false).await.unwrap();
    }

    // ============================================================================
    // Time Since Last Update Tests
    // ============================================================================

    #[tokio::test]
    async fn test_oc_time_since_last_update_implemented_sensors() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_oc_device(factory);
        device.set_connected(true).await.unwrap();

        // After connect, refresh has populated sensor means
        let temp_time = device
            .time_since_last_update("temperature".to_string())
            .await
            .unwrap();
        assert!(temp_time < 1.0); // should be very recent

        let humidity_time = device
            .time_since_last_update("humidity".to_string())
            .await
            .unwrap();
        assert!(humidity_time < 1.0);

        // Empty string means most recent across all sensors
        let all_time = device.time_since_last_update("".to_string()).await.unwrap();
        assert!(all_time < 1.0);

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn test_oc_time_since_last_update_unimplemented_sensor() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_oc_device(factory);
        device.set_connected(true).await.unwrap();

        let err = device
            .time_since_last_update("pressure".to_string())
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::NOT_IMPLEMENTED);

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn test_oc_time_since_last_update_unknown_sensor() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_oc_device(factory);
        device.set_connected(true).await.unwrap();

        let err = device
            .time_since_last_update("foobar".to_string())
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);

        device.set_connected(false).await.unwrap();
    }

    // ============================================================================
    // Sensor Read Tests (covers temperature/humidity/dewpoint value paths)
    // ============================================================================

    #[tokio::test]
    async fn test_oc_read_sensor_values_when_connected() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_oc_device(factory);
        device.set_connected(true).await.unwrap();

        // Status: temp=25.0, humidity=60, dewpoint=15.5
        let temp = device.temperature().await.unwrap();
        assert!((temp - 25.0).abs() < 0.01);

        let humidity = device.humidity().await.unwrap();
        assert!((humidity - 60.0).abs() < 0.01);

        let dewpoint = device.dew_point().await.unwrap();
        assert!((dewpoint - 15.5).abs() < 0.01);

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn test_oc_average_period_default() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_oc_device(factory);
        device.set_connected(true).await.unwrap();

        // Default averaging period is based on polling interval config
        let period = device.average_period().await.unwrap();
        assert!(period > 0.0);

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn test_oc_set_average_period_normal_value() {
        let factory = Arc::new(MockSerialPortFactory::new(standard_connection_responses()));
        let device = create_oc_device(factory);
        device.set_connected(true).await.unwrap();

        // Set to 1 hour
        device.set_average_period(1.0).await.unwrap();
        let period = device.average_period().await.unwrap();
        assert!((period - 1.0).abs() < 0.001);

        device.set_connected(false).await.unwrap();
    }

    // ============================================================================
    // Miscellaneous Tests
    // ============================================================================

    #[tokio::test]
    async fn test_oc_device_debug_format() {
        let factory = Arc::new(MockSerialPortFactory::new(vec![]));
        let device = create_oc_device(factory);

        let debug_str = format!("{:?}", device);
        assert!(debug_str.contains("PpbaObservingConditionsDevice"));
        assert!(debug_str.contains("config"));
        assert!(debug_str.contains("requested_connection"));
        assert!(debug_str.contains(".."));
    }

    #[tokio::test]
    async fn test_oc_device_info() {
        let factory = Arc::new(MockSerialPortFactory::new(vec![]));
        let device = create_oc_device(factory);

        let info = device.driver_info().await.unwrap();
        assert!(info.contains("PPBA"));

        let version = device.driver_version().await.unwrap();
        assert!(!version.is_empty());

        let description = device.description().await.unwrap();
        assert!(!description.is_empty());
    }

    #[tokio::test]
    async fn test_oc_refresh_when_connected() {
        let factory = Arc::new(MockSerialPortFactory::new(vec![
            "PPBA_OK".to_string(),
            "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
            "PS:2.5:10.5:126.0:3600000".to_string(),
            // Response for refresh()
            "PPBA:13.0:3.5:26.0:65:16.0:1:0:128:64:0:0:0".to_string(),
            // Polling responses
            "PPBA:13.0:3.5:26.0:65:16.0:1:0:128:64:0:0:0".to_string(),
            "PS:2.5:10.5:126.0:3600000".to_string(),
        ]));
        let device = create_oc_device(factory);
        device.set_connected(true).await.unwrap();

        // Refresh should succeed and update readings
        device.refresh().await.unwrap();

        device.set_connected(false).await.unwrap();
    }
}
