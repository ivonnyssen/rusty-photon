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

/// Helper to create device with mock factory, returning the serial manager too
fn create_test_device_with_manager(
    factory: Arc<dyn SerialPortFactory>,
) -> (PpbaObservingConditionsDevice, Arc<SerialManager>) {
    let config = Config::default();
    let serial_manager = Arc::new(SerialManager::new(config.clone(), factory));
    let device =
        PpbaObservingConditionsDevice::new(config.observingconditions, serial_manager.clone());
    (device, serial_manager)
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
    device.set_connected(true).await.unwrap();

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
    device.set_connected(true).await.unwrap();

    // Set to 2 hours
    device.set_average_period(2.0).await.unwrap();

    let period = device.average_period().await.unwrap();
    assert_eq!(period, 2.0);
}

#[tokio::test]
async fn test_set_average_period_minimum() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    // Minimum: 0.0 hours (instantaneous)
    device.set_average_period(0.0).await.unwrap();

    let period = device.average_period().await.unwrap();
    assert_eq!(period, 0.0);
}

#[tokio::test]
async fn test_set_average_period_maximum() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    // Maximum: 24 hours
    device.set_average_period(24.0).await.unwrap();

    let period = device.average_period().await.unwrap();
    assert_eq!(period, 24.0);
}

#[tokio::test]
async fn test_set_average_period_too_small() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

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
    device.set_connected(true).await.unwrap();

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
    let (device, serial_manager) = create_test_device_with_manager(factory);

    device.set_connected(true).await.unwrap();

    // Sleep so samples from connect() age, then shrink window to trigger cleanup
    tokio::time::sleep(Duration::from_millis(10)).await;
    serial_manager
        .set_averaging_period(Duration::from_millis(1))
        .await;

    let result = device.temperature().await;
    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::VALUE_NOT_SET,
            ..
        }) => {
            // get_mean() returned None because all samples aged out
        }
        other => panic!("Expected VALUE_NOT_SET error, got {:?}", other),
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
    device.set_connected(true).await.unwrap();

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
    device.set_connected(true).await.unwrap();

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
    device.set_connected(true).await.unwrap();

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
    device.set_connected(true).await.unwrap();

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
    device.set_connected(true).await.unwrap();

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

// =============================================================================
// Sensor Description Edge Case Tests
// =============================================================================

#[tokio::test]
async fn test_sensor_description_empty_string() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    let result = device.sensor_description("".to_string()).await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::INVALID_VALUE,
            ..
        }) => {
            // Expected - empty sensor name is invalid
        }
        _ => panic!("Expected INVALID_VALUE error for empty sensor name"),
    }
}

#[tokio::test]
async fn test_sensor_description_truly_unknown() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    // "foobar" is not a recognized sensor name at all
    let result = device.sensor_description("foobar".to_string()).await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::INVALID_VALUE,
            ..
        }) => {
            // Expected - truly unknown sensor returns INVALID_VALUE
        }
        _ => panic!("Expected INVALID_VALUE error for truly unknown sensor name"),
    }
}

// =============================================================================
// Time Since Last Update Edge Case Tests
// =============================================================================

#[tokio::test]
async fn test_time_since_last_update_empty_string() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    // Wait for polling to gather data
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Empty string should return most recent update across all sensors
    let time = device.time_since_last_update("".to_string()).await.unwrap();
    assert!(time < 1.0, "Expected recent update time, got {}", time);
}

#[tokio::test]
async fn test_time_since_last_update_truly_unknown() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    // "foobar" is not a recognized sensor
    let result = device.time_since_last_update("foobar".to_string()).await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::INVALID_VALUE,
            ..
        }) => {
            // Expected - truly unknown sensor returns INVALID_VALUE
        }
        _ => panic!("Expected INVALID_VALUE error for truly unknown sensor"),
    }
}

#[tokio::test]
async fn test_time_since_last_update_unimplemented_sensors() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    let unimplemented_sensors = vec![
        "cloudcover",
        "pressure",
        "rainrate",
        "skybrightness",
        "skyquality",
        "starfwhm",
        "skytemperature",
        "winddirection",
        "windgust",
        "windspeed",
    ];

    for sensor in unimplemented_sensors {
        let result = device.time_since_last_update(sensor.to_string()).await;

        match result {
            Err(ASCOMError {
                code: ASCOMErrorCode::NOT_IMPLEMENTED,
                ..
            }) => {
                // Expected
            }
            _ => panic!(
                "Expected NOT_IMPLEMENTED for time_since_last_update('{}'), got {:?}",
                sensor, result
            ),
        }
    }
}

// =============================================================================
// No Data Edge Case Tests
// =============================================================================

#[tokio::test]
async fn test_humidity_no_data() {
    let factory = Arc::new(create_connected_mock_factory());
    let (device, serial_manager) = create_test_device_with_manager(factory);
    device.set_connected(true).await.unwrap();

    // Sleep so samples from connect() age, then shrink window to trigger cleanup
    tokio::time::sleep(Duration::from_millis(10)).await;
    serial_manager
        .set_averaging_period(Duration::from_millis(1))
        .await;

    let result = device.humidity().await;
    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::VALUE_NOT_SET,
            ..
        }) => {
            // get_mean() returned None because all samples aged out
        }
        other => panic!("Expected VALUE_NOT_SET error, got {:?}", other),
    }
}

#[tokio::test]
async fn test_dew_point_no_data() {
    let factory = Arc::new(create_connected_mock_factory());
    let (device, serial_manager) = create_test_device_with_manager(factory);
    device.set_connected(true).await.unwrap();

    // Sleep so samples from connect() age, then shrink window to trigger cleanup
    tokio::time::sleep(Duration::from_millis(10)).await;
    serial_manager
        .set_averaging_period(Duration::from_millis(1))
        .await;

    let result = device.dew_point().await;
    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::VALUE_NOT_SET,
            ..
        }) => {
            // get_mean() returned None because all samples aged out
        }
        other => panic!("Expected VALUE_NOT_SET error, got {:?}", other),
    }
}

// =============================================================================
// Not Connected Error Path Tests
// =============================================================================

#[tokio::test]
async fn test_set_average_period_not_connected() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.set_average_period(1.0).await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_CONNECTED,
            ..
        }) => {
            // Expected
        }
        _ => panic!("Expected NOT_CONNECTED error when setting average period while disconnected"),
    }
}

#[tokio::test]
async fn test_average_period_not_connected() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.average_period().await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_CONNECTED,
            ..
        }) => {
            // Expected
        }
        _ => panic!("Expected NOT_CONNECTED error when reading average period while disconnected"),
    }
}

#[tokio::test]
async fn test_sensor_description_not_connected() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    let result = device.sensor_description("temperature".to_string()).await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::NOT_CONNECTED,
            ..
        }) => {
            // Expected
        }
        _ => panic!(
            "Expected NOT_CONNECTED error when reading sensor description while disconnected"
        ),
    }
}

// =============================================================================
// to_ascom_error Mapping Tests
// =============================================================================

/// Mock serial port factory that always fails on open
struct FailingSerialPortFactory;

#[async_trait]
impl SerialPortFactory for FailingSerialPortFactory {
    async fn open(&self, _port: &str, _baud_rate: u32, _timeout: Duration) -> Result<SerialPair> {
        Err(ppba_driver::PpbaError::ConnectionFailed(
            "mock port not found".to_string(),
        ))
    }

    async fn port_exists(&self, _port: &str) -> bool {
        false
    }
}

#[tokio::test]
async fn test_set_connected_connection_failed_maps_to_invalid_operation() {
    // FailingSerialPortFactory returns ConnectionFailed which hits the wildcard branch
    let factory: Arc<dyn SerialPortFactory> = Arc::new(FailingSerialPortFactory);
    let device = create_test_device(factory);

    let result = device.set_connected(true).await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::INVALID_OPERATION,
            ..
        }) => {
            // ConnectionFailed -> wildcard -> invalid_operation
        }
        other => panic!(
            "Expected INVALID_OPERATION error for connection failure, got {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn test_set_connected_bad_ping_maps_to_invalid_operation() {
    // Bad ping response causes InvalidResponse which hits the wildcard branch
    let factory = Arc::new(MockSerialPortFactory::new(vec![
        "GARBAGE".to_string(), // Bad ping response
    ]));
    let device = create_test_device(factory);

    let result = device.set_connected(true).await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::INVALID_OPERATION,
            ..
        }) => {
            // InvalidResponse -> wildcard -> invalid_operation
        }
        other => panic!(
            "Expected INVALID_OPERATION error for bad ping, got {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn test_set_connected_bad_status_maps_to_invalid_operation() {
    // Good ping but bad status response
    let factory = Arc::new(MockSerialPortFactory::new(vec![
        "PPBA_OK".to_string(), // Valid ping
        "GARBAGE".to_string(), // Bad status response
    ]));
    let device = create_test_device(factory);

    let result = device.set_connected(true).await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::INVALID_OPERATION,
            ..
        }) => {
            // ParseError/InvalidResponse -> wildcard -> invalid_operation
        }
        other => panic!(
            "Expected INVALID_OPERATION error for bad status, got {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn test_refresh_bad_status_maps_to_invalid_operation() {
    // Successful connect, then feed bad status on refresh
    let factory = Arc::new(MockSerialPortFactory::new(vec![
        "PPBA_OK".to_string(),                                     // Ping
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // Status (connect)
        "PS:2.5:10.5:126.0:3600000".to_string(),                   // Power stats (connect)
        "GARBAGE".to_string(),                                     // Bad status for refresh()
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // Polling
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(), // Polling
    ]));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();

    let result = device.refresh().await;

    match result {
        Err(ASCOMError {
            code: ASCOMErrorCode::INVALID_OPERATION,
            ..
        }) => {
            // ParseError/InvalidResponse -> wildcard -> invalid_operation
        }
        other => panic!(
            "Expected INVALID_OPERATION error for bad refresh status, got {:?}",
            other
        ),
    }
}

// =============================================================================
// Connection Idempotency Tests
// =============================================================================

#[tokio::test]
async fn test_set_connected_true_when_already_connected() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();
    assert!(device.connected().await.unwrap());

    // Calling set_connected(true) again should be a no-op
    device.set_connected(true).await.unwrap();
    assert!(device.connected().await.unwrap());
}

#[tokio::test]
async fn test_set_connected_false_when_already_disconnected() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);

    // Device starts disconnected
    assert!(!device.connected().await.unwrap());

    // Calling set_connected(false) should be a no-op
    device.set_connected(false).await.unwrap();
    assert!(!device.connected().await.unwrap());
}

// =============================================================================
// Time Since Last Update - Per-Sensor Tests
// =============================================================================

#[tokio::test]
async fn test_time_since_last_update_humidity() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    let time = device
        .time_since_last_update("humidity".to_string())
        .await
        .unwrap();
    assert!(time < 1.0, "Expected recent humidity update, got {}", time);
}

#[tokio::test]
async fn test_time_since_last_update_dewpoint() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    let time = device
        .time_since_last_update("dewpoint".to_string())
        .await
        .unwrap();
    assert!(time < 1.0, "Expected recent dewpoint update, got {}", time);
}

#[tokio::test]
async fn test_time_since_last_update_case_insensitive() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Should work with different cases
    let time1 = device
        .time_since_last_update("Temperature".to_string())
        .await
        .unwrap();
    let time2 = device
        .time_since_last_update("TEMPERATURE".to_string())
        .await
        .unwrap();

    assert!(time1 < 1.0);
    assert!(time2 < 1.0);
}

// =============================================================================
// Sensor Description - All Unimplemented Sensors
// =============================================================================

#[tokio::test]
async fn test_sensor_description_unimplemented_sensors() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    let unimplemented_sensors = vec![
        "cloudcover",
        "pressure",
        "rainrate",
        "skybrightness",
        "skyquality",
        "starfwhm",
        "skytemperature",
        "winddirection",
        "windgust",
        "windspeed",
    ];

    for sensor in unimplemented_sensors {
        let result = device.sensor_description(sensor.to_string()).await;

        match result {
            Err(ASCOMError {
                code: ASCOMErrorCode::NOT_IMPLEMENTED,
                ..
            }) => {
                // Expected
            }
            _ => panic!(
                "Expected NOT_IMPLEMENTED for sensor_description('{}'), got {:?}",
                sensor, result
            ),
        }
    }
}

// =============================================================================
// Refresh Data Update Tests
// =============================================================================

#[tokio::test]
async fn test_refresh_updates_sensor_data() {
    // Use different values in the refresh response to confirm data updates
    let factory = Arc::new(MockSerialPortFactory::new(vec![
        "PPBA_OK".to_string(),                                     // Ping
        "PPBA:12.5:3.2:20.0:50:10.0:1:0:128:64:0:0:0".to_string(), // Status (initial: temp=20.0, humidity=50, dewpoint=10.0)
        "PS:2.5:10.5:126.0:3600000".to_string(),                   // Power stats
        "PPBA:12.5:3.2:30.0:70:20.0:1:0:128:64:0:0:0".to_string(), // Status for refresh (temp=30.0, humidity=70, dewpoint=20.0)
        "PPBA:12.5:3.2:30.0:70:20.0:1:0:128:64:0:0:0".to_string(), // Polling
        "PPBA:12.5:3.2:30.0:70:20.0:1:0:128:64:0:0:0".to_string(), // Polling
    ]));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let temp_before = device.temperature().await.unwrap();

    // Force refresh with updated data
    device.refresh().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let temp_after = device.temperature().await.unwrap();

    // The mean should shift toward the new values
    assert!(
        temp_after > temp_before,
        "Expected temperature to increase after refresh with higher values, before={}, after={}",
        temp_before,
        temp_after
    );
}

// =============================================================================
// Average Period Fractional and Transition Tests
// =============================================================================

#[tokio::test]
async fn test_set_average_period_fractional() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    // Set to 0.5 hours (30 minutes)
    device.set_average_period(0.5).await.unwrap();

    let period = device.average_period().await.unwrap();
    assert!(
        (period - 0.5).abs() < 0.0001,
        "Expected 0.5 hours, got {}",
        period
    );
}

#[tokio::test]
async fn test_set_average_period_transition_from_instantaneous() {
    let factory = Arc::new(create_connected_mock_factory());
    let device = create_test_device(factory);
    device.set_connected(true).await.unwrap();

    // Set to instantaneous
    device.set_average_period(0.0).await.unwrap();
    assert_eq!(device.average_period().await.unwrap(), 0.0);

    // Then change to 1 hour
    device.set_average_period(1.0).await.unwrap();
    assert_eq!(device.average_period().await.unwrap(), 1.0);

    // And back to instantaneous
    device.set_average_period(0.0).await.unwrap();
    assert_eq!(device.average_period().await.unwrap(), 0.0);
}

// =============================================================================
// Sensor Readings with Different Data
// =============================================================================

#[tokio::test]
async fn test_sensor_readings_reflect_status_values() {
    // Use specific known values
    let factory = Arc::new(MockSerialPortFactory::new(vec![
        "PPBA_OK".to_string(),
        "PPBA:12.5:3.2:18.3:45:8.7:1:0:128:64:0:0:0".to_string(), // temp=18.3, humidity=45, dewpoint=8.7
        "PS:2.5:10.5:126.0:3600000".to_string(),
        "PPBA:12.5:3.2:18.3:45:8.7:1:0:128:64:0:0:0".to_string(), // Polling
        "PPBA:12.5:3.2:18.3:45:8.7:1:0:128:64:0:0:0".to_string(), // Polling
        "PPBA:12.5:3.2:18.3:45:8.7:1:0:128:64:0:0:0".to_string(), // Polling
    ]));
    let device = create_test_device(factory);

    device.set_connected(true).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let temp = device.temperature().await.unwrap();
    assert!(
        (temp - 18.3).abs() < 0.1,
        "Expected temp ~18.3, got {}",
        temp
    );

    let humidity = device.humidity().await.unwrap();
    assert!(
        (humidity - 45.0).abs() < 0.1,
        "Expected humidity ~45, got {}",
        humidity
    );

    let dewpoint = device.dew_point().await.unwrap();
    assert!(
        (dewpoint - 8.7).abs() < 0.1,
        "Expected dewpoint ~8.7, got {}",
        dewpoint
    );
}
