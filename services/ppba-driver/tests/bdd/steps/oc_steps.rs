//! Step definitions for observing_conditions.feature

use std::time::Duration;

use crate::world::mock_serial;
use crate::world::PpbaWorld;
use ascom_alpaca::api::{Device, ObservingConditions};
use ascom_alpaca::ASCOMErrorCode;
use cucumber::{given, then, when};
use ppba_driver::Config;

// ============================================================================
// Given steps
// ============================================================================

#[given(expr = "an OC device with name {string}")]
fn oc_device_with_name(world: &mut PpbaWorld, name: String) {
    let mut config = Config::default();
    config.observingconditions.name = name;
    world.build_oc_device_with_config_and_responses(
        config,
        mock_serial::standard_connection_responses(),
    );
}

#[given(expr = "an OC device with unique ID {string}")]
fn oc_device_with_unique_id(world: &mut PpbaWorld, unique_id: String) {
    let mut config = Config::default();
    config.observingconditions.unique_id = unique_id;
    world.build_oc_device_with_config_and_responses(
        config,
        mock_serial::standard_connection_responses(),
    );
}

#[given(
    expr = "an OC device with custom status responses temp {float}, humidity {int}, dewpoint {float}"
)]
fn oc_device_with_custom_status(world: &mut PpbaWorld, temp: f64, humidity: usize, dewpoint: f64) {
    let status = format!(
        "PPBA:12.5:3.2:{}:{}:{}:1:0:128:64:0:0:0",
        temp, humidity, dewpoint
    );
    world.build_oc_device_with_responses(vec![
        "PPBA_OK".to_string(),
        status.clone(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        status.clone(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        status.clone(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
    ]);
}

#[given("an OC device with refresh update mock responses")]
fn oc_device_with_refresh_update_responses(world: &mut PpbaWorld) {
    world.build_oc_device_with_responses(vec![
        "PPBA_OK".to_string(),
        // connect: status (temp=20) + power stats
        "PPBA:12.5:3.2:20.0:50:10.0:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        // poller tick 1: status (temp=20) + power stats
        "PPBA:12.5:3.2:20.0:50:10.0:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        // refresh call: status with increased temp (temp=30)
        "PPBA:12.5:3.2:30.0:70:20.0:1:0:128:64:0:0:0".to_string(),
        // spares
        "PPBA:12.5:3.2:30.0:70:20.0:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
    ]);
}

#[given("an OC device with refresh bad status mock responses")]
fn oc_device_with_refresh_bad_status(world: &mut PpbaWorld) {
    world.build_oc_device_with_responses(vec![
        "PPBA_OK".to_string(),
        // connect: status + power stats
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        // poller tick 1: status + power stats
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
        // refresh call: bad status response
        "GARBAGE".to_string(),
        // spares
        "PPBA:12.5:3.2:25.0:60:15.5:1:0:128:64:0:0:0".to_string(),
        "PS:2.5:10.5:126.0:3600000".to_string(),
    ]);
}

#[given("an OC device with bad status response")]
fn oc_device_with_bad_status(world: &mut PpbaWorld) {
    world.build_oc_device_with_responses(vec!["PPBA_OK".to_string(), "GARBAGE".to_string()]);
}

// ============================================================================
// When steps
// ============================================================================

#[when(expr = "I set the average period to {float} hours")]
async fn set_average_period(world: &mut PpbaWorld, hours: f64) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    device.set_average_period(hours).await.unwrap();
}

#[when(expr = "I try to set the average period to {float} hours")]
async fn try_set_average_period(world: &mut PpbaWorld, hours: f64) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    match device.set_average_period(hours).await {
        Ok(()) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I try to read the temperature")]
async fn try_read_temperature(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    match device.temperature().await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I try to read the humidity")]
async fn try_read_humidity(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    match device.humidity().await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I try to read the dewpoint")]
async fn try_read_dewpoint(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    match device.dew_point().await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I age out all samples")]
async fn age_out_all_samples(world: &mut PpbaWorld) {
    // Yield multiple times to ensure the poller's first tick completes fully.
    // The poller has multiple internal await points, so we yield generously.
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    // Sleep so samples are definitively older than the 1ms window we'll set.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let manager = world.serial_manager.as_ref().expect("manager not created");
    // Shrink the window to 1ms — all samples (~100ms old) get pruned.
    manager.set_averaging_period(Duration::from_millis(1)).await;
}

#[when(expr = "I try to get sensor description for {string}")]
async fn try_get_sensor_description(world: &mut PpbaWorld, sensor: String) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    match device.sensor_description(sensor).await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when(expr = "I try to get time since last update for {string}")]
async fn try_get_time_since_last_update(world: &mut PpbaWorld, sensor: String) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    match device.time_since_last_update(sensor).await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I try to refresh the OC device")]
async fn try_refresh_oc_device(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    match device.refresh().await {
        Ok(()) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I try to read cloud cover")]
async fn try_read_cloud_cover(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    match device.cloud_cover().await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I try to read pressure")]
async fn try_read_pressure(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    match device.pressure().await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I try to read rain rate")]
async fn try_read_rain_rate(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    match device.rain_rate().await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I try to read sky brightness")]
async fn try_read_sky_brightness(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    match device.sky_brightness().await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I try to read sky quality")]
async fn try_read_sky_quality(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    match device.sky_quality().await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I try to read sky temperature")]
async fn try_read_sky_temperature(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    match device.sky_temperature().await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I try to read star FWHM")]
async fn try_read_star_fwhm(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    match device.star_fwhm().await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I try to read wind direction")]
async fn try_read_wind_direction(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    match device.wind_direction().await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I try to read wind gust")]
async fn try_read_wind_gust(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    match device.wind_gust().await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I try to read wind speed")]
async fn try_read_wind_speed(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    match device.wind_speed().await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I try to read the average period")]
async fn try_read_average_period(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    match device.average_period().await {
        Ok(_) => {
            world.last_error = None;
            world.last_error_code = None;
        }
        Err(e) => {
            world.last_error = Some(e.to_string());
            world.last_error_code = Some(e.code.raw());
        }
    }
}

#[when("I record the temperature")]
async fn record_temperature(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    let _temp = device.temperature().await.unwrap();
    // Store for later comparison - we use a simple flag approach
}

#[when("I refresh the OC device")]
async fn refresh_oc_device(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    device.refresh().await.unwrap();
}

// ============================================================================
// Then steps
// ============================================================================

#[then(expr = "the OC device static name should be {string}")]
fn oc_device_static_name_should_be(world: &mut PpbaWorld, expected: String) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    assert_eq!(device.static_name(), expected);
}

#[then(expr = "the OC device unique ID should be {string}")]
fn oc_device_unique_id_should_be(world: &mut PpbaWorld, expected: String) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    assert_eq!(device.unique_id(), expected);
}

#[then(expr = "the OC device description should contain {string}")]
async fn oc_device_description_should_contain(world: &mut PpbaWorld, expected: String) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    let description = device.description().await.unwrap();
    assert!(
        description.contains(&expected),
        "expected description to contain '{}', got: {}",
        expected,
        description
    );
}

#[then(expr = "the OC device driver info should contain {string}")]
async fn oc_device_driver_info_should_contain(world: &mut PpbaWorld, expected: String) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    let info = device.driver_info().await.unwrap();
    assert!(
        info.contains(&expected),
        "expected driver info to contain '{}', got: {}",
        expected,
        info
    );
}

#[then("the OC device driver version should not be empty")]
async fn oc_device_driver_version_not_empty(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    let version = device.driver_version().await.unwrap();
    assert!(!version.is_empty());
}

#[then(expr = "the average period should be approximately {float} hours")]
async fn average_period_should_be_approximately(world: &mut PpbaWorld, expected: f64) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    let period = device.average_period().await.unwrap();
    assert!(
        (period - expected).abs() < 0.001,
        "expected average period ~{}, got {}",
        expected,
        period
    );
}

#[then(expr = "the average period should be {float} hours")]
async fn average_period_should_be(world: &mut PpbaWorld, expected: f64) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    let period = device.average_period().await.unwrap();
    assert_eq!(period, expected);
}

#[then(expr = "the temperature should be approximately {float}")]
async fn temperature_should_be_approximately(world: &mut PpbaWorld, expected: f64) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    let temp = device.temperature().await.unwrap();
    assert!(
        (temp - expected).abs() < 0.1,
        "expected temperature ~{}, got {}",
        expected,
        temp
    );
}

#[then(expr = "the humidity should be approximately {float}")]
async fn humidity_should_be_approximately(world: &mut PpbaWorld, expected: f64) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    let humidity = device.humidity().await.unwrap();
    assert!(
        (humidity - expected).abs() < 0.1,
        "expected humidity ~{}, got {}",
        expected,
        humidity
    );
}

#[then(expr = "the dewpoint should be approximately {float}")]
async fn dewpoint_should_be_approximately(world: &mut PpbaWorld, expected: f64) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    let dewpoint = device.dew_point().await.unwrap();
    assert!(
        (dewpoint - expected).abs() < 0.1,
        "expected dewpoint ~{}, got {}",
        expected,
        dewpoint
    );
}

#[then("reading the temperature should return VALUE_NOT_SET")]
async fn temperature_should_return_value_not_set(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    let result = device.temperature().await;
    match result {
        Err(e) if e.code == ASCOMErrorCode::VALUE_NOT_SET => {}
        other => panic!("Expected VALUE_NOT_SET, got {:?}", other),
    }
}

#[then("reading the humidity should return VALUE_NOT_SET")]
async fn humidity_should_return_value_not_set(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    let result = device.humidity().await;
    match result {
        Err(e) if e.code == ASCOMErrorCode::VALUE_NOT_SET => {}
        other => panic!("Expected VALUE_NOT_SET, got {:?}", other),
    }
}

#[then("reading the dewpoint should return VALUE_NOT_SET")]
async fn dewpoint_should_return_value_not_set(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    let result = device.dew_point().await;
    match result {
        Err(e) if e.code == ASCOMErrorCode::VALUE_NOT_SET => {}
        other => panic!("Expected VALUE_NOT_SET, got {:?}", other),
    }
}

#[then(expr = "sensor description for {string} should contain {string}")]
async fn sensor_description_should_contain(
    world: &mut PpbaWorld,
    sensor: String,
    expected: String,
) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    let desc = device.sensor_description(sensor).await.unwrap();
    assert!(
        desc.contains(&expected),
        "expected sensor description to contain '{}', got: {}",
        expected,
        desc
    );
}

#[then(expr = "sensor description for {string} and {string} should match")]
async fn sensor_description_case_insensitive(
    world: &mut PpbaWorld,
    sensor1: String,
    sensor2: String,
) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    let desc1 = device.sensor_description(sensor1).await.unwrap();
    let desc2 = device.sensor_description(sensor2).await.unwrap();
    assert_eq!(desc1, desc2);
}

#[then(expr = "time since last update for {string} should be less than {float} seconds")]
async fn time_since_last_update_less_than(world: &mut PpbaWorld, sensor: String, max_time: f64) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    let time = device.time_since_last_update(sensor).await.unwrap();
    assert!(
        time < max_time,
        "expected time < {}, got {}",
        max_time,
        time
    );
}

#[then("time since last update should return NOT_IMPLEMENTED for all unimplemented sensors")]
async fn time_since_last_update_not_implemented_for_all(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    let sensors = [
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
    for sensor in sensors {
        let result = device.time_since_last_update(sensor.to_string()).await;
        match result {
            Err(e) if e.code == ASCOMErrorCode::NOT_IMPLEMENTED => {}
            other => panic!("Expected NOT_IMPLEMENTED for '{}', got {:?}", sensor, other),
        }
    }
}

#[then("sensor description should return NOT_IMPLEMENTED for all unimplemented sensors")]
async fn sensor_description_not_implemented_for_all(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    let sensors = [
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
    for sensor in sensors {
        let result = device.sensor_description(sensor.to_string()).await;
        match result {
            Err(e) if e.code == ASCOMErrorCode::NOT_IMPLEMENTED => {}
            other => panic!("Expected NOT_IMPLEMENTED for '{}', got {:?}", sensor, other),
        }
    }
}

#[then("refreshing the OC device should succeed")]
async fn refreshing_oc_device_should_succeed(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    device.refresh().await.unwrap();
}

#[then("the temperature should have increased")]
async fn temperature_should_have_increased(world: &mut PpbaWorld) {
    let device = world.oc_device.as_ref().expect("OC device not created");
    let temp = device.temperature().await.unwrap();
    // Initial data was temp=20.0, refresh data was temp=30.0
    // The mean should be > 20.0 (shifted toward 30.0)
    assert!(
        temp > 20.0,
        "expected temperature > 20.0 after refresh, got {}",
        temp
    );
}
