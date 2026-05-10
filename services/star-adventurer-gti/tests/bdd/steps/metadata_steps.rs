//! Steps for device_metadata.feature.

#![allow(unused_variables)]

use crate::world::StarAdventurerWorld;
use cucumber::gherkin::Step;
use cucumber::{given, then};

#[given(expr = "a star-adventurer service configured with name {string}")]
async fn configured_with_name(world: &mut StarAdventurerWorld, name: String) {
    todo!("Phase 3: set world.config.mount.name = name, then start_service()")
}

#[given(expr = "a star-adventurer service configured with unique ID {string}")]
async fn configured_with_unique_id(world: &mut StarAdventurerWorld, id: String) {
    todo!("Phase 3: set world.config.mount.unique_id, then start_service()")
}

#[given(expr = "a star-adventurer service configured with description {string}")]
async fn configured_with_description(world: &mut StarAdventurerWorld, description: String) {
    todo!("Phase 3: set world.config.mount.description, then start_service()")
}

#[given(expr = "a star-adventurer service configured with site latitude {float} degrees")]
async fn configured_with_site_latitude(world: &mut StarAdventurerWorld, deg: f64) {
    todo!("Phase 3: set world.config.mount.site_latitude_deg, then start_service()")
}

#[given(expr = "a star-adventurer service configured with site latitude {float}")]
async fn configured_with_site_latitude_no_unit(world: &mut StarAdventurerWorld, deg: f64) {
    todo!("Phase 3: same as 'site latitude X degrees'")
}

#[given(expr = "a star-adventurer service configured with site longitude {float} degrees")]
async fn configured_with_site_longitude(world: &mut StarAdventurerWorld, deg: f64) {
    todo!("Phase 3: set world.config.mount.site_longitude_deg, then start_service()")
}

#[given(expr = "a star-adventurer service configured with site longitude {float}")]
async fn configured_with_site_longitude_no_unit(world: &mut StarAdventurerWorld, deg: f64) {
    todo!("Phase 3: same as 'site longitude X degrees'")
}

#[then(expr = "the device name should be {string}")]
async fn device_name_should_be(world: &mut StarAdventurerWorld, expected: String) {
    let actual = world.mount().static_name().to_string();
    assert_eq!(actual, expected);
}

#[then(expr = "the device unique ID should be {string}")]
async fn device_unique_id_should_be(world: &mut StarAdventurerWorld, expected: String) {
    let actual = world.mount().unique_id().to_string();
    assert_eq!(actual, expected);
}

#[then(expr = "the device description should be {string}")]
async fn device_description_should_be(world: &mut StarAdventurerWorld, expected: String) {
    let actual = world.mount().description().await.unwrap();
    assert_eq!(actual, expected);
}

#[then(expr = "the driver info should contain {string}")]
async fn driver_info_should_contain(world: &mut StarAdventurerWorld, needle: String) {
    let info = world.mount().driver_info().await.unwrap();
    assert!(
        info.contains(&needle),
        "driver_info '{info}' lacks '{needle}'"
    );
}

#[then("the driver version should not be empty")]
async fn driver_version_not_empty(world: &mut StarAdventurerWorld) {
    let version = world.mount().driver_version().await.unwrap();
    assert!(!version.is_empty());
}

#[then("the device capabilities should match these values:")]
async fn capabilities_should_match(world: &mut StarAdventurerWorld, step: &Step) {
    let _rows = step.table.as_ref().expect("expected a data table");
    todo!("Phase 3: iterate (capability, value) rows, dispatch to the matching telescope getter")
}

#[then("TrackingRates should equal [Sidereal]")]
async fn tracking_rates_sidereal_only(world: &mut StarAdventurerWorld) {
    use ascom_alpaca::api::telescope::DriveRate;
    let rates = world.mount().tracking_rates().await.unwrap();
    assert_eq!(rates, vec![DriveRate::Sidereal]);
}

#[then(expr = "SiteLatitude should be {float} degrees")]
async fn site_latitude_should_be(world: &mut StarAdventurerWorld, expected: f64) {
    let actual = world.mount().site_latitude().await.unwrap();
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

#[then(expr = "SiteLongitude should be {float} degrees")]
async fn site_longitude_should_be(world: &mut StarAdventurerWorld, expected: f64) {
    let actual = world.mount().site_longitude().await.unwrap();
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}
