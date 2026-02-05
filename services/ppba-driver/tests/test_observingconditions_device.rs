//! Tests for PPBA ObservingConditions device implementation

use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::{Device, ObservingConditions};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode};
use async_trait::async_trait;
use ppba_driver::io::{SerialPair, SerialPortFactory, SerialReader, SerialWriter};
use ppba_driver::{Config, PpbaObservingConditionsDevice, Result, SerialManager};
use tokio::sync::Mutex;

/// Mock serial reader that returns predefined responses
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
            // Cycle back to beginning for polling
            *index = 0;
            if !responses.is_empty() {
                Ok(Some(responses[0].clone()))
            } else {
                Ok(None)
            }
        }
    }
}

/// Mock serial writer that records sent messages
struct MockSerialWriter {
    sent_messages: Arc<Mutex<Vec<String>>>,
}

impl MockSerialWriter {
    fn new() -> Self {
        Self {
            sent_messages: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl SerialWriter for MockSerialWriter {
    async fn write_message(&mut self, message: &str) -> Result<()> {
        let mut messages = self.sent_messages.lock().await;
        messages.push(message.to_string());
        Ok(())
    }
}

/// Mock serial port factory
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
            writer: Box::new(MockSerialWriter::new()),
        })
    }

    async fn port_exists(&self, _port: &str) -> bool {
        true
    }
}

/// Create a mock factory with standard responses for connection
fn create_connected_mock_factory() -> MockSerialPortFactory {
    // Provide enough responses for initial connection and subsequent polling/refresh
    MockSerialPortFactory::new(vec![
        "PPBA_OK".to_string(),                                     // Ping response
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // Status (temp=25.0, humidity=60, dewpoint=15.5)
        "PS:2.5:10.5:126.0:3600000".to_string(),                   // Power stats
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // Status for polling/refresh
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // Status for polling/refresh
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // Status for polling/refresh
    ])
}

/// Helper to create device with mock factory
fn create_test_device(factory: Arc<dyn SerialPortFactory>) -> PpbaObservingConditionsDevice {
    let config = Config::default();
    let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));
    PpbaObservingConditionsDevice::new(config.observingconditions, serial_manager)
}

// =============================================================================
// Device Interface Tests
// =============================================================================

#[tokio::test]
async fn test_device_static_name() {
    let factory = Arc::new(create_connected_mock_factory());
    let mut config = Config::default();
    config.observingconditions.name = "Test Weather Station".to_string();

    let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));
    let device = PpbaObservingConditionsDevice::new(config.observingconditions, serial_manager);

    assert_eq!(device.static_name(), "Test Weather Station");
}

#[tokio::test]
async fn test_device_unique_id() {
    let factory = Arc::new(create_connected_mock_factory());
    let mut config = Config::default();
    config.observingconditions.unique_id = "custom-weather-001".to_string();

    let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));
    let device = PpbaObservingConditionsDevice::new(config.observingconditions, serial_manager);

    assert_eq!(device.unique_id(), "custom-weather-001");
}

#[tokio::test]
async fn test_device_description() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let description = device.description().await.unwrap();
    assert!(description.contains("Environmental"));
}

#[tokio::test]
async fn test_device_driver_info() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let driver_info = device.driver_info().await.unwrap();
    assert!(driver_info.contains("PPBA"));
    assert!(driver_info.contains("ObservingConditions"));
}

#[tokio::test]
async fn test_device_driver_version() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let version = device.driver_version().await.unwrap();
    // Version should be populated from Cargo.toml
    assert!(!version.is_empty());
}

// =============================================================================
// Connection Tests
// =============================================================================

#[tokio::test]
async fn test_connected_initially_false() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let connected = device.connected().await.unwrap();
    assert!(!connected);
}

#[tokio::test]
async fn test_set_connected_true() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    let connected = device.connected().await.unwrap();
    assert!(connected);
}

#[tokio::test]
async fn test_set_connected_false() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();
    assert!(device.connected().await.unwrap());

    device.set_connected(false).await.unwrap();
    assert!(!device.connected().await.unwrap());
}

// =============================================================================
// Average Period Tests
// =============================================================================

#[tokio::test]
async fn test_average_period_default() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    // Default should be 5 minutes = 5/60 hours = 0.0833... hours
    let period = device.average_period().await.unwrap();
    assert!(
        (period - (5.0 / 60.0)).abs() < 0.0001,
        "Expected ~0.0833 hours, got {}",
        period
    );
}

#[tokio::test]
async fn test_set_average_period() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    // Set to 2 hours
    device.set_average_period(2.0).await.unwrap();

    let period = device.average_period().await.unwrap();
    assert_eq!(period, 2.0);
}

#[tokio::test]
async fn test_set_average_period_minimum() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    // Minimum: 0.0 hours (instantaneous)
    device.set_average_period(0.0).await.unwrap();

    let period = device.average_period().await.unwrap();
    assert_eq!(period, 0.0);
}

#[tokio::test]
async fn test_set_average_period_maximum() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    // Maximum: 24 hours
    device.set_average_period(24.0).await.unwrap();

    let period = device.average_period().await.unwrap();
    assert_eq!(period, 24.0);
}

#[tokio::test]
async fn test_set_average_period_too_small() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    // Negative values are rejected
    let result = device.set_average_period(-1.0).await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::INVALID_VALUE,
            ..
        }) => {
            // Expected error
        }
        _ => panic!("Expected INVALID_VALUE error for negative period"),
    }
}

#[tokio::test]
async fn test_set_average_period_too_large() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    // More than 24 hours is rejected
    let result = device.set_average_period(25.0).await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::INVALID_VALUE,
            ..
        }) => {
            // Expected error
        }
        _ => panic!("Expected INVALID_VALUE error for period too large"),
    }
}

// =============================================================================
// Sensor Reading Tests
// =============================================================================

#[tokio::test]
async fn test_temperature_not_connected() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.temperature().await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_CONNECTED,
            ..
        }) => {
            // Expected error
        }
        _ => panic!("Expected NOT_CONNECTED error when reading temperature while disconnected"),
    }
}

#[tokio::test]
async fn test_temperature_no_data() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Immediately after connection, means might not have data yet
    // This should return VALUE_NOT_SET
    let result = device.temperature().await;

    match result {
        Ok(_) => {
            // Data available, that's fine too
        }
        Err(ASCOMError {
            code: ASCOMErrorCode::VALUE_NOT_SET,
            ..
        }) => {
            // Expected if no samples yet
        }
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}

#[tokio::test]
async fn test_temperature_with_data() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Wait for polling to gather data
    tokio::time::sleep(Duration::from_millis(100)).await;

    let temp = device.temperature().await.unwrap();
    // From mock: temp=25.0
    assert!((temp - 25.0).abs() < 0.1);
}

#[tokio::test]
async fn test_humidity_not_connected() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.humidity().await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_CONNECTED,
            ..
        }) => {
            // Expected error
        }
        _ => panic!("Expected NOT_CONNECTED error when reading humidity while disconnected"),
    }
}

#[tokio::test]
async fn test_humidity_with_data() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Wait for polling to gather data
    tokio::time::sleep(Duration::from_millis(100)).await;

    let humidity = device.humidity().await.unwrap();
    // From mock: humidity=60
    assert!((humidity - 60.0).abs() < 0.1);
}

#[tokio::test]
async fn test_dew_point_not_connected() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.dew_point().await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_CONNECTED,
            ..
        }) => {
            // Expected error
        }
        _ => panic!("Expected NOT_CONNECTED error when reading dewpoint while disconnected"),
    }
}

#[tokio::test]
async fn test_dew_point_with_data() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Wait for polling to gather data
    tokio::time::sleep(Duration::from_millis(100)).await;

    let dewpoint = device.dew_point().await.unwrap();
    // From mock: dewpoint=15.5
    assert!((dewpoint - 15.5).abs() < 0.1);
}

// =============================================================================
// Sensor Description Tests
// =============================================================================

#[tokio::test]
async fn test_sensor_description_temperature() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let desc = device
        .sensor_description("temperature".to_string())
        .await
        .unwrap();
    assert!(desc.contains("temperature"));
}

#[tokio::test]
async fn test_sensor_description_humidity() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let desc = device
        .sensor_description("humidity".to_string())
        .await
        .unwrap();
    assert!(desc.contains("humidity"));
}

#[tokio::test]
async fn test_sensor_description_dewpoint() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let desc = device
        .sensor_description("dewpoint".to_string())
        .await
        .unwrap();
    assert!(desc.contains("Dewpoint"));
}

#[tokio::test]
async fn test_sensor_description_case_insensitive() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    // Should work with different cases
    let desc1 = device
        .sensor_description("Temperature".to_string())
        .await
        .unwrap();
    let desc2 = device
        .sensor_description("TEMPERATURE".to_string())
        .await
        .unwrap();

    assert_eq!(desc1, desc2);
}

#[tokio::test]
async fn test_sensor_description_unknown() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    // "pressure" is an unimplemented sensor - should return NOT_IMPLEMENTED
    let result = device.sensor_description("pressure".to_string()).await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_IMPLEMENTED,
            ..
        }) => {
            // Expected error for unimplemented sensor
        }
        _ => panic!("Expected NOT_IMPLEMENTED error for unimplemented sensor"),
    }
}

// =============================================================================
// Time Since Last Update Tests
// =============================================================================

#[tokio::test]
async fn test_time_since_last_update_not_connected() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device
        .time_since_last_update("temperature".to_string())
        .await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_CONNECTED,
            ..
        }) => {
            // Expected error
        }
        _ => panic!("Expected NOT_CONNECTED error"),
    }
}

#[tokio::test]
async fn test_time_since_last_update_with_data() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Wait for polling to gather data
    tokio::time::sleep(Duration::from_millis(100)).await;

    let time = device
        .time_since_last_update("temperature".to_string())
        .await
        .unwrap();

    // Should be very recent
    assert!(time < 1.0); // Less than 1 second
}

#[tokio::test]
async fn test_time_since_last_update_unknown_sensor() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // "pressure" is an unimplemented sensor - should return NOT_IMPLEMENTED
    let result = device.time_since_last_update("pressure".to_string()).await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_IMPLEMENTED,
            ..
        }) => {
            // Expected error for unimplemented sensor
        }
        _ => panic!("Expected NOT_IMPLEMENTED error for unimplemented sensor"),
    }
}

// =============================================================================
// Refresh Tests
// =============================================================================

#[tokio::test]
async fn test_refresh_not_connected() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.refresh().await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_CONNECTED,
            ..
        }) => {
            // Expected error
        }
        _ => panic!("Expected NOT_CONNECTED error when refreshing while disconnected"),
    }
}

#[tokio::test]
async fn test_refresh_connected() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    // Refresh should succeed
    device.refresh().await.unwrap();
}

// =============================================================================
// Unimplemented Sensor Tests
// =============================================================================

#[tokio::test]
async fn test_cloud_cover_not_implemented() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.cloud_cover().await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_IMPLEMENTED,
            ..
        }) => {
            // Expected - cloud cover not implemented
        }
        _ => panic!("Expected NOT_IMPLEMENTED error for cloud_cover"),
    }
}

#[tokio::test]
async fn test_pressure_not_implemented() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.pressure().await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_IMPLEMENTED,
            ..
        }) => {
            // Expected - pressure not implemented
        }
        _ => panic!("Expected NOT_IMPLEMENTED error for pressure"),
    }
}

#[tokio::test]
async fn test_rain_rate_not_implemented() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.rain_rate().await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_IMPLEMENTED,
            ..
        }) => {
            // Expected - rain_rate not implemented
        }
        _ => panic!("Expected NOT_IMPLEMENTED error for rain_rate"),
    }
}

#[tokio::test]
async fn test_sky_brightness_not_implemented() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.sky_brightness().await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_IMPLEMENTED,
            ..
        }) => {
            // Expected - sky_brightness not implemented
        }
        _ => panic!("Expected NOT_IMPLEMENTED error for sky_brightness"),
    }
}

#[tokio::test]
async fn test_sky_quality_not_implemented() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.sky_quality().await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_IMPLEMENTED,
            ..
        }) => {
            // Expected - sky_quality not implemented
        }
        _ => panic!("Expected NOT_IMPLEMENTED error for sky_quality"),
    }
}

#[tokio::test]
async fn test_sky_temperature_not_implemented() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.sky_temperature().await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_IMPLEMENTED,
            ..
        }) => {
            // Expected - sky_temperature not implemented
        }
        _ => panic!("Expected NOT_IMPLEMENTED error for sky_temperature"),
    }
}

#[tokio::test]
async fn test_star_fwhm_not_implemented() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.star_fwhm().await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_IMPLEMENTED,
            ..
        }) => {
            // Expected - star_fwhm not implemented
        }
        _ => panic!("Expected NOT_IMPLEMENTED error for star_fwhm"),
    }
}

#[tokio::test]
async fn test_wind_direction_not_implemented() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.wind_direction().await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_IMPLEMENTED,
            ..
        }) => {
            // Expected - wind_direction not implemented
        }
        _ => panic!("Expected NOT_IMPLEMENTED error for wind_direction"),
    }
}

#[tokio::test]
async fn test_wind_gust_not_implemented() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.wind_gust().await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_IMPLEMENTED,
            ..
        }) => {
            // Expected - wind_gust not implemented
        }
        _ => panic!("Expected NOT_IMPLEMENTED error for wind_gust"),
    }
}

#[tokio::test]
async fn test_wind_speed_not_implemented() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.wind_speed().await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_IMPLEMENTED,
            ..
        }) => {
            // Expected - wind_speed not implemented
        }
        _ => panic!("Expected NOT_IMPLEMENTED error for wind_speed"),
    }
}
