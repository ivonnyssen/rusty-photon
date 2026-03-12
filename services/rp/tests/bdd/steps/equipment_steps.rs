//! BDD step definitions for equipment connectivity feature

use cucumber::{given, then, when};

use crate::steps::infrastructure::{OmniSimHandle, RpHandle};
use crate::world::{CameraConfig, FilterWheelConfig, RpWorld};

// --- Given steps ---

#[given("a running Alpaca simulator")]
async fn running_alpaca_simulator(world: &mut RpWorld) {
    if world.omnisim.is_none() {
        world.omnisim = Some(OmniSimHandle::start().await);
    }
}

#[given("rp is configured with a camera on the simulator")]
fn configured_with_camera(world: &mut RpWorld) {
    let url = world.omnisim_url();
    world.cameras.push(CameraConfig {
        id: "main-cam".to_string(),
        alpaca_url: url,
        device_number: 0,
    });
}

#[given("rp is configured with a filter wheel on the simulator")]
fn configured_with_filter_wheel(world: &mut RpWorld) {
    let url = world.omnisim_url();
    world.filter_wheels.push(FilterWheelConfig {
        id: "main-fw".to_string(),
        alpaca_url: url,
        device_number: 0,
        filters: vec![
            "Luminance".to_string(),
            "Red".to_string(),
            "Green".to_string(),
            "Blue".to_string(),
        ],
    });
}

#[given(expr = "rp is configured with a camera at {string} device {int}")]
fn configured_with_camera_at(world: &mut RpWorld, url: String, device_number: i32) {
    world.cameras.push(CameraConfig {
        id: "main-cam".to_string(),
        alpaca_url: url,
        device_number: device_number as u32,
    });
}

// --- When steps ---

#[when("rp starts")]
async fn rp_starts(world: &mut RpWorld) {
    // Use port 0 so the rp process binds to an OS-assigned port.
    // The actual port is discovered by parsing the process stdout.
    world.rp = Some(RpHandle {
        child: None,
        base_url: String::new(),
        port: 0,
        config_path: String::new(),
        stdout_drain: None,
    });

    let config = world.build_config();
    let config_path = std::env::temp_dir()
        .join(format!(
            "rp-test-config-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
        .to_string_lossy()
        .to_string();
    tokio::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap())
        .await
        .expect("failed to write config");

    // Start rp — it discovers its own port from stdout
    world.rp = Some(RpHandle::start(&config_path).await);

    assert!(
        world.wait_for_rp_healthy().await,
        "rp did not become healthy within timeout"
    );
}

// --- Then steps ---

#[then("the equipment status should show the camera as connected")]
async fn camera_should_be_connected(world: &mut RpWorld) {
    let client = reqwest::Client::new();
    let url = format!("{}/api/equipment", world.rp_url());
    let resp = client
        .get(&url)
        .send()
        .await
        .expect("failed to GET /api/equipment");

    let body: serde_json::Value = resp
        .json()
        .await
        .expect("failed to parse equipment response");

    let cameras = body
        .get("cameras")
        .and_then(|v| v.as_array())
        .expect("no cameras array in equipment response");

    let cam = cameras
        .iter()
        .find(|c| c.get("id").and_then(|v| v.as_str()) == Some("main-cam"))
        .expect("main-cam not found in equipment response");

    assert_eq!(
        cam.get("connected").and_then(|v| v.as_bool()),
        Some(true),
        "expected main-cam to be connected, got: {:?}",
        cam
    );
}

#[then("the equipment status should show the filter wheel as connected")]
async fn filter_wheel_should_be_connected(world: &mut RpWorld) {
    let client = reqwest::Client::new();
    let url = format!("{}/api/equipment", world.rp_url());
    let resp = client
        .get(&url)
        .send()
        .await
        .expect("failed to GET /api/equipment");

    let body: serde_json::Value = resp
        .json()
        .await
        .expect("failed to parse equipment response");

    let filter_wheels = body
        .get("filter_wheels")
        .and_then(|v| v.as_array())
        .expect("no filter_wheels array in equipment response");

    let fw = filter_wheels
        .iter()
        .find(|f| f.get("id").and_then(|v| v.as_str()) == Some("main-fw"))
        .expect("main-fw not found in equipment response");

    assert_eq!(
        fw.get("connected").and_then(|v| v.as_bool()),
        Some(true),
        "expected main-fw to be connected, got: {:?}",
        fw
    );
}

#[then("the equipment status should show the camera as disconnected")]
async fn camera_should_be_disconnected(world: &mut RpWorld) {
    let client = reqwest::Client::new();
    let url = format!("{}/api/equipment", world.rp_url());
    let resp = client
        .get(&url)
        .send()
        .await
        .expect("failed to GET /api/equipment");

    let body: serde_json::Value = resp
        .json()
        .await
        .expect("failed to parse equipment response");

    let cameras = body
        .get("cameras")
        .and_then(|v| v.as_array())
        .expect("no cameras array in equipment response");

    let cam = cameras
        .iter()
        .find(|c| c.get("id").and_then(|v| v.as_str()) == Some("main-cam"))
        .expect("main-cam not found in equipment response");

    assert_eq!(
        cam.get("connected").and_then(|v| v.as_bool()),
        Some(false),
        "expected main-cam to be disconnected, got: {:?}",
        cam
    );
}
