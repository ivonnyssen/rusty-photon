//! Unit tests for PpbaObservingConditionsDevice ASCOM error mapping and edge cases
//!
//! These tests exercise error paths in the ObservingConditions device that are
//! only reachable through internal failures (factory errors) or specific invalid
//! inputs, covering `to_ascom_error` branches and the Debug implementation.

use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::{Device, ObservingConditions};
use ascom_alpaca::ASCOMErrorCode;
use async_trait::async_trait;
use ppba_driver::error::PpbaError;
use ppba_driver::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};
use ppba_driver::{Config, PpbaObservingConditionsDevice, Result, SerialManager};
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
    async fn read_line(&mut self) -> Result<Option<String>> {
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
    async fn write_message(&mut self, _message: &str) -> Result<()> {
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
    async fn open(&self, _port: &str, _baud_rate: u32, _timeout: Duration) -> Result<SerialPair> {
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
    async fn open(&self, _port: &str, _baud_rate: u32, _timeout: Duration) -> Result<SerialPair> {
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
