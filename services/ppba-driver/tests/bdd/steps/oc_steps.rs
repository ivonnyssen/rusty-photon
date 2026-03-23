//! Step definitions for observing_conditions.feature

use crate::steps::infrastructure::*;
use crate::world::PpbaWorld;
use ascom_alpaca::ASCOMErrorCode;
use cucumber::{given, then, when};

// ============================================================================
// Given steps
// ============================================================================

#[given("a running PPBA server with the OC device connected")]
async fn running_server_with_oc_connected(world: &mut PpbaWorld) {
    world.config = default_test_config();
    world.start_ppba().await;

    let url = world.oc_url();
    let resp = alpaca_put(&url, "connected", &[("Connected", "true")]).await;
    assert!(
        !is_alpaca_error(&resp),
        "connecting OC failed: {}",
        alpaca_error_message(&resp)
    );
}

#[given(expr = "a running PPBA server with OC name {string}")]
async fn running_server_with_oc_name(world: &mut PpbaWorld, name: String) {
    world.config = default_test_config();
    world.config["observingconditions"]["name"] = serde_json::json!(name);
    world.start_ppba().await;
}

#[given(expr = "a running PPBA server with OC unique ID {string}")]
async fn running_server_with_oc_unique_id(world: &mut PpbaWorld, unique_id: String) {
    world.config = default_test_config();
    world.config["observingconditions"]["unique_id"] = serde_json::json!(unique_id);
    world.start_ppba().await;
}

// ============================================================================
// When steps
// ============================================================================

#[when("I wait for the OC data to be available")]
async fn wait_for_oc_data(world: &mut PpbaWorld) {
    world.wait_for_oc_data().await;
}

#[when(expr = "I set the average period to {float} hours")]
async fn set_average_period(world: &mut PpbaWorld, hours: f64) {
    let url = world.oc_url();
    let val_str = hours.to_string();
    let resp = alpaca_put(&url, "averageperiod", &[("AveragePeriod", &val_str)]).await;
    assert!(
        !is_alpaca_error(&resp),
        "set average period failed: {}",
        alpaca_error_message(&resp)
    );
}

#[when(expr = "I try to set the average period to {float} hours")]
async fn try_set_average_period(world: &mut PpbaWorld, hours: f64) {
    let url = world.oc_url();
    let val_str = hours.to_string();
    let resp = alpaca_put(&url, "averageperiod", &[("AveragePeriod", &val_str)]).await;
    world.capture_response(&resp);
}

#[when("I try to read the temperature")]
async fn try_read_temperature(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "temperature").await;
    world.capture_response(&resp);
}

#[when("I try to read the humidity")]
async fn try_read_humidity(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "humidity").await;
    world.capture_response(&resp);
}

#[when("I try to read the dewpoint")]
async fn try_read_dewpoint(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "dewpoint").await;
    world.capture_response(&resp);
}

#[when(expr = "I try to get sensor description for {string}")]
async fn try_get_sensor_description(world: &mut PpbaWorld, sensor: String) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, &format!("sensordescription?SensorName={}", sensor)).await;
    world.capture_response(&resp);
}

#[when(expr = "I try to get time since last update for {string}")]
async fn try_get_time_since_last_update(world: &mut PpbaWorld, sensor: String) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, &format!("timesincelastupdate?SensorName={}", sensor)).await;
    world.capture_response(&resp);
}

#[when("I try to refresh the OC device")]
async fn try_refresh_oc_device(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_put(&url, "refresh", &[]).await;
    world.capture_response(&resp);
}

#[when("I try to read cloud cover")]
async fn try_read_cloud_cover(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "cloudcover").await;
    world.capture_response(&resp);
}

#[when("I try to read pressure")]
async fn try_read_pressure(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "pressure").await;
    world.capture_response(&resp);
}

#[when("I try to read rain rate")]
async fn try_read_rain_rate(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "rainrate").await;
    world.capture_response(&resp);
}

#[when("I try to read sky brightness")]
async fn try_read_sky_brightness(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "skybrightness").await;
    world.capture_response(&resp);
}

#[when("I try to read sky quality")]
async fn try_read_sky_quality(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "skyquality").await;
    world.capture_response(&resp);
}

#[when("I try to read sky temperature")]
async fn try_read_sky_temperature(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "skytemperature").await;
    world.capture_response(&resp);
}

#[when("I try to read star FWHM")]
async fn try_read_star_fwhm(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "starfwhm").await;
    world.capture_response(&resp);
}

#[when("I try to read wind direction")]
async fn try_read_wind_direction(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "winddirection").await;
    world.capture_response(&resp);
}

#[when("I try to read wind gust")]
async fn try_read_wind_gust(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "windgust").await;
    world.capture_response(&resp);
}

#[when("I try to read wind speed")]
async fn try_read_wind_speed(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "windspeed").await;
    world.capture_response(&resp);
}

#[when("I try to read the average period")]
async fn try_read_average_period(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "averageperiod").await;
    world.capture_response(&resp);
}

// ============================================================================
// Then steps
// ============================================================================

#[then(expr = "the OC device static name should be {string}")]
async fn oc_device_static_name_should_be(world: &mut PpbaWorld, expected: String) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "name").await;
    assert!(!is_alpaca_error(&resp), "GET OC name failed");
    assert_eq!(
        alpaca_value(&resp).as_str().unwrap(),
        expected,
        "OC device name mismatch"
    );
}

#[then(expr = "the OC device unique ID should be {string}")]
async fn oc_device_unique_id_should_be(world: &mut PpbaWorld, expected: String) {
    let base = world.base_url.as_ref().expect("server not started");
    let resp = alpaca_get(base, "management/v1/configureddevices").await;
    let devices = alpaca_value(&resp)
        .as_array()
        .expect("configureddevices should return an array");
    let oc_entry = devices
        .iter()
        .find(|d| d["DeviceType"].as_str() == Some("ObservingConditions"))
        .expect("no ObservingConditions device found in configureddevices");
    assert_eq!(
        oc_entry["UniqueID"].as_str().unwrap(),
        expected,
        "OC unique ID mismatch"
    );
}

#[then(expr = "the OC device description should contain {string}")]
async fn oc_device_description_should_contain(world: &mut PpbaWorld, expected: String) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "description").await;
    assert!(!is_alpaca_error(&resp), "GET OC description failed");
    let desc = alpaca_value(&resp).as_str().unwrap();
    assert!(
        desc.contains(&expected),
        "expected OC description to contain '{}', got: {}",
        expected,
        desc
    );
}

#[then(expr = "the OC device driver info should contain {string}")]
async fn oc_device_driver_info_should_contain(world: &mut PpbaWorld, expected: String) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "driverinfo").await;
    assert!(!is_alpaca_error(&resp), "GET OC driverinfo failed");
    let info = alpaca_value(&resp).as_str().unwrap();
    assert!(
        info.contains(&expected),
        "expected OC driver info to contain '{}', got: {}",
        expected,
        info
    );
}

#[then("the OC device driver version should not be empty")]
async fn oc_device_driver_version_not_empty(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "driverversion").await;
    assert!(!is_alpaca_error(&resp), "GET OC driverversion failed");
    let version = alpaca_value(&resp).as_str().unwrap();
    assert!(!version.is_empty(), "OC driver version should not be empty");
}

#[then(expr = "the average period should be approximately {float} hours")]
async fn average_period_should_be_approximately(world: &mut PpbaWorld, expected: f64) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "averageperiod").await;
    assert!(!is_alpaca_error(&resp), "GET averageperiod failed");
    let period = alpaca_value(&resp)
        .as_f64()
        .expect("average period should be a number");
    assert!(
        (period - expected).abs() < 0.001,
        "expected average period ~{}, got {}",
        expected,
        period
    );
}

#[then(expr = "the average period should be {float} hours")]
async fn average_period_should_be(world: &mut PpbaWorld, expected: f64) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "averageperiod").await;
    assert!(!is_alpaca_error(&resp), "GET averageperiod failed");
    let period = alpaca_value(&resp)
        .as_f64()
        .expect("average period should be a number");
    assert!(
        (period - expected).abs() < f64::EPSILON,
        "expected average period {}, got {}",
        expected,
        period
    );
}

#[then(expr = "the temperature should be approximately {float}")]
async fn temperature_should_be_approximately(world: &mut PpbaWorld, expected: f64) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "temperature").await;
    assert!(!is_alpaca_error(&resp), "GET temperature failed");
    let temp = alpaca_value(&resp)
        .as_f64()
        .expect("temperature should be a number");
    assert!(
        (temp - expected).abs() < 0.1,
        "expected temperature ~{}, got {}",
        expected,
        temp
    );
}

#[then(expr = "the humidity should be approximately {float}")]
async fn humidity_should_be_approximately(world: &mut PpbaWorld, expected: f64) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "humidity").await;
    assert!(!is_alpaca_error(&resp), "GET humidity failed");
    let humidity = alpaca_value(&resp)
        .as_f64()
        .expect("humidity should be a number");
    assert!(
        (humidity - expected).abs() < 0.1,
        "expected humidity ~{}, got {}",
        expected,
        humidity
    );
}

#[then(expr = "the dewpoint should be approximately {float}")]
async fn dewpoint_should_be_approximately(world: &mut PpbaWorld, expected: f64) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, "dewpoint").await;
    assert!(!is_alpaca_error(&resp), "GET dewpoint failed");
    let dewpoint = alpaca_value(&resp)
        .as_f64()
        .expect("dewpoint should be a number");
    assert!(
        (dewpoint - expected).abs() < 0.1,
        "expected dewpoint ~{}, got {}",
        expected,
        dewpoint
    );
}

#[then(expr = "sensor description for {string} should contain {string}")]
async fn sensor_description_should_contain(
    world: &mut PpbaWorld,
    sensor: String,
    expected: String,
) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, &format!("sensordescription?SensorName={}", sensor)).await;
    assert!(
        !is_alpaca_error(&resp),
        "sensordescription failed for '{}'",
        sensor
    );
    let desc = alpaca_value(&resp).as_str().unwrap();
    assert!(
        desc.to_lowercase().contains(&expected.to_lowercase()),
        "expected sensor description for '{}' to contain '{}', got: {}",
        sensor,
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
    let url = world.oc_url();
    let resp1 = alpaca_get(&url, &format!("sensordescription?SensorName={}", sensor1)).await;
    let resp2 = alpaca_get(&url, &format!("sensordescription?SensorName={}", sensor2)).await;
    assert!(
        !is_alpaca_error(&resp1),
        "sensordescription failed for '{}'",
        sensor1
    );
    assert!(
        !is_alpaca_error(&resp2),
        "sensordescription failed for '{}'",
        sensor2
    );
    assert_eq!(
        alpaca_value(&resp1).as_str().unwrap(),
        alpaca_value(&resp2).as_str().unwrap(),
        "sensor descriptions for '{}' and '{}' should match",
        sensor1,
        sensor2
    );
}

#[then(expr = "time since last update for {string} should be less than {float} seconds")]
async fn time_since_last_update_less_than(world: &mut PpbaWorld, sensor: String, max_time: f64) {
    let url = world.oc_url();
    let resp = alpaca_get(&url, &format!("timesincelastupdate?SensorName={}", sensor)).await;
    assert!(
        !is_alpaca_error(&resp),
        "timesincelastupdate failed for '{}'",
        sensor
    );
    let time = alpaca_value(&resp)
        .as_f64()
        .expect("time since last update should be a number");
    assert!(
        time < max_time,
        "expected time < {}, got {} for sensor '{}'",
        max_time,
        time,
        sensor
    );
}

#[then("time since last update should return NOT_IMPLEMENTED for all unimplemented sensors")]
async fn time_since_last_update_not_implemented_for_all(world: &mut PpbaWorld) {
    let url = world.oc_url();
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
        let resp = alpaca_get(&url, &format!("timesincelastupdate?SensorName={}", sensor)).await;
        assert_eq!(
            alpaca_error_number(&resp),
            ASCOMErrorCode::NOT_IMPLEMENTED.raw() as i64,
            "expected NOT_IMPLEMENTED for timesincelastupdate('{}'), got error: {}",
            sensor,
            alpaca_error_number(&resp)
        );
    }
}

#[then("sensor description should return NOT_IMPLEMENTED for all unimplemented sensors")]
async fn sensor_description_not_implemented_for_all(world: &mut PpbaWorld) {
    let url = world.oc_url();
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
        let resp = alpaca_get(&url, &format!("sensordescription?SensorName={}", sensor)).await;
        assert_eq!(
            alpaca_error_number(&resp),
            ASCOMErrorCode::NOT_IMPLEMENTED.raw() as i64,
            "expected NOT_IMPLEMENTED for sensordescription('{}'), got error: {}",
            sensor,
            alpaca_error_number(&resp)
        );
    }
}

#[then("refreshing the OC device should succeed")]
async fn refreshing_oc_device_should_succeed(world: &mut PpbaWorld) {
    let url = world.oc_url();
    let resp = alpaca_put(&url, "refresh", &[]).await;
    assert!(
        !is_alpaca_error(&resp),
        "refresh should succeed: {}",
        alpaca_error_message(&resp)
    );
}
