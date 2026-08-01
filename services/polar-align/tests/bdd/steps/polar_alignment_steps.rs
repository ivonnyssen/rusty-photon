//! BDD step definitions for the end-to-end polar-alignment workflow.
//!
//! The scenarios spawn OmniSim (telescope + camera), rp, and
//! polar-align, plus an in-process plate-solver stub whose canned
//! solves are choreographed from a known injected axis error. The
//! completion contract is asserted through the plugin's `/status`
//! endpoint (rp deliberately ignores completion bodies) and rp's
//! session status.

use std::time::Duration;

use bdd_infra::rp_harness::{
    start_rp, write_temp_config_file, CannedWcs, CannedWcsMatrix, OmniSimHandle, PlateSolverStub,
    StubBehavior,
};
use bdd_infra::ServiceHandle;
use cucumber::{given, then, when};
use polar_align::math::{radec_from_unit, unit_from_radec, Mat3, Vec3};
use rp_ephemeris::{AltAz, ErfarsEphemeris, Site};

use crate::world::{PolarAlignWorld, SITE_LATITUDE_DEG, SITE_LONGITUDE_DEG};

/// Generate the three measurement solves (plus one adjustment solve
/// the Sequence clamps on) for a mount whose RA axis sits at the
/// given error from the pole: `east_arcmin` of true angle toward the
/// east horizon, `alt_arcmin` above it. Pure geometry in the world's
/// shared site, unrefracted — matching the service config's
/// `refraction.enabled = false`.
fn choreograph_solves(east_arcmin: f64, alt_arcmin: f64) -> Vec<CannedWcs> {
    let site = Site::new(SITE_LATITUDE_DEG, SITE_LONGITUDE_DEG).expect("test site");
    let eph = ErfarsEphemeris::new();
    let now = chrono::Utc::now();

    let axis_alt = SITE_LATITUDE_DEG + alt_arcmin / 60.0;
    let axis_az = ((east_arcmin / 60.0_f64).to_radians().sin() / axis_alt.to_radians().cos())
        .asin()
        .to_degrees();
    let axis_icrs = eph
        .icrs_from_alt_az(
            &site,
            AltAz {
                azimuth_degrees: axis_az,
                altitude_degrees: axis_alt,
            },
            now,
            None,
        )
        .expect("axis observed→ICRS");
    let axis = unit_from_radec(axis_icrs.ra_hours * 15.0, axis_icrs.dec_degrees);

    // Scope ~5° off-axis (the dec-85 sweep), three points 45° apart.
    let perp = axis
        .cross(Vec3::new(0.0, 0.0, 1.0))
        .normalized()
        .expect("axis is not the celestial pole");
    let start = Mat3::from_axis_angle(perp, 5.0_f64.to_radians()).mul_vec(axis);

    (0..4)
        .map(|i| {
            let sweep = f64::from(i.min(2)) * 45.0_f64.to_radians();
            let pointing = Mat3::from_axis_angle(axis, sweep).mul_vec(start);
            let (ra_center, dec_center) = radec_from_unit(pointing);
            CannedWcs {
                ra_center,
                dec_center,
                pixel_scale_arcsec: 1.05,
                rotation_deg: 0.0,
                solver: "stub-astap".to_string(),
                wcs_matrix: Some(CannedWcsMatrix {
                    crpix1: 512.0,
                    crpix2: 384.0,
                    cd1_1: -2.9167e-4,
                    cd1_2: 0.0,
                    cd2_1: 0.0,
                    cd2_2: 2.9167e-4,
                }),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Given steps
// ---------------------------------------------------------------------------

#[given("a running Alpaca simulator")]
async fn running_alpaca_simulator(world: &mut PolarAlignWorld) {
    if world.omnisim.is_none() {
        world.omnisim = Some(OmniSimHandle::start().await);
    }
}

#[given(
    expr = "a stub plate solver choreographed for an axis error of {float} arcminutes east and {float} arcminutes in altitude"
)]
async fn stub_solver_choreographed(world: &mut PolarAlignWorld, east: f64, alt: f64) {
    let stub = PlateSolverStub::start(StubBehavior::Sequence(choreograph_solves(east, alt))).await;
    world.plate_solver = Some(bdd_infra::rp_harness::PlateSolverConfig {
        url: stub.url.clone(),
        timeout: None,
        default_search_radius_deg: None,
    });
    world.plate_solver_stub = Some(stub);
    world.injected_error_arcmin = Some((east, alt));
}

#[given("a stub plate solver that always fails")]
async fn stub_solver_failing(world: &mut PolarAlignWorld) {
    let stub = PlateSolverStub::start(StubBehavior::Error {
        code: "solve_failed".to_string(),
        message: "no stars matched".to_string(),
    })
    .await;
    world.plate_solver = Some(bdd_infra::rp_harness::PlateSolverConfig {
        url: stub.url.clone(),
        timeout: None,
        default_search_radius_deg: None,
    });
    world.plate_solver_stub = Some(stub);
}

#[given(
    "rp is running with a camera, a mount, the stub plate solver, and the polar-align orchestrator"
)]
async fn rp_running_with_polar_align(world: &mut PolarAlignWorld) {
    configure_default_equipment(world).await;
    start_polar_align_service(world).await;
    register_polar_align_plugin(world);
    start_rp_service(world).await;
}

#[given("the polar-align service is running standalone")]
async fn polar_align_standalone(world: &mut PolarAlignWorld) {
    start_polar_align_service(world).await;
}

// ---------------------------------------------------------------------------
// When steps
// ---------------------------------------------------------------------------

#[when("a session is started via the REST API")]
async fn start_session(world: &mut PolarAlignWorld) {
    let client = reqwest::Client::new();
    let url = format!("{}/api/session/start", world.rp_url());

    let resp = client
        .post(&url)
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("failed to POST /api/session/start");

    world.last_api_status = Some(resp.status().as_u16());
    world.last_api_body = resp.json().await.ok();
}

#[when(expr = "the polar-align workflow reaches the {string} phase")]
async fn workflow_reaches_phase(world: &mut PolarAlignWorld, expected: String) {
    // Measurement = three OmniSim slews + captures + solves; allow a
    // generous bound and poll the plugin's own status endpoint.
    let client = reqwest::Client::new();
    let url = format!("{}/status", world.polar_align_url());
    let mut last_phase = String::new();
    for _ in 0..720 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if let Ok(resp) = client.get(&url).send().await {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                last_phase = body
                    .get("phase")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if last_phase == expected {
                    return;
                }
            }
        }
    }
    panic!(
        "polar-align did not reach phase '{}' within 180s (last phase: '{}')",
        expected, last_phase
    );
}

#[when("an invocation without a workflow id is posted via the REST API")]
async fn post_invocation_missing_workflow_id(world: &mut PolarAlignWorld) {
    let client = reqwest::Client::new();
    let url = format!("{}/invoke", world.polar_align_url());
    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "mcp_server_url": "http://localhost:1/mcp" }))
        .send()
        .await
        .expect("failed to POST /invoke");
    world.last_api_status = Some(resp.status().as_u16());
    world.last_api_body = resp.json().await.ok();
}

#[when("the adjustment is finished via the REST API")]
async fn finish_adjustment(world: &mut PolarAlignWorld) {
    let client = reqwest::Client::new();
    let url = format!("{}/adjust/finish", world.polar_align_url());
    let resp = client
        .post(&url)
        .send()
        .await
        .expect("failed to POST /adjust/finish");
    world.last_api_status = Some(resp.status().as_u16());
    world.last_api_body = resp.json().await.ok();
}

// ---------------------------------------------------------------------------
// Then steps
// ---------------------------------------------------------------------------

#[then(expr = "the session status should be {string}")]
async fn session_status_is(world: &mut PolarAlignWorld, expected: String) {
    // The completion POST races the session-status read; poll briefly.
    let client = reqwest::Client::new();
    let url = format!("{}/api/session/status", world.rp_url());
    let mut actual = String::new();
    for _ in 0..120 {
        let resp = client
            .get(&url)
            .send()
            .await
            .expect("failed to GET /api/session/status");
        let body: serde_json::Value = resp.json().await.expect("failed to parse session status");
        actual = body
            .get("status")
            .and_then(|v| v.as_str())
            .expect("status field missing")
            .to_string();
        if actual == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("expected session status '{expected}' but got '{actual}' after 30s");
}

#[then(
    expr = "the polar-align status should report an azimuth error within {float} arcminutes of {float}"
)]
async fn status_azimuth_error(world: &mut PolarAlignWorld, tolerance: f64, expected: f64) {
    let value = measurement_field(world, "azimuth_error_arcmin").await;
    assert!(
        (value - expected).abs() <= tolerance,
        "azimuth error {value}′ not within {tolerance}′ of {expected}′"
    );
}

#[then(
    expr = "the polar-align status should report an altitude error within {float} arcminutes of {float}"
)]
async fn status_altitude_error(world: &mut PolarAlignWorld, tolerance: f64, expected: f64) {
    let value = measurement_field(world, "altitude_error_arcmin").await;
    assert!(
        (value - expected).abs() <= tolerance,
        "altitude error {value}′ not within {tolerance}′ of {expected}′"
    );
}

#[then(expr = "the stub plate solver should have received at least {int} solve requests")]
async fn stub_received_requests(world: &mut PolarAlignWorld, count: usize) {
    let stub = world
        .plate_solver_stub
        .as_ref()
        .expect("plate solver stub not started");
    let requests = stub.requests().await;
    assert!(
        requests.len() >= count,
        "expected at least {} solve requests, saw {}",
        count,
        requests.len()
    );
}

#[then(expr = "the finish request should be rejected with status {int}")]
async fn finish_rejected(world: &mut PolarAlignWorld, expected: u16) {
    assert_eq!(
        world.last_api_status,
        Some(expected),
        "unexpected /adjust/finish status (body: {:?})",
        world.last_api_body
    );
}

#[then(expr = "the invoke request should be rejected with status {int}")]
async fn invoke_rejected(world: &mut PolarAlignWorld, expected: u16) {
    assert_eq!(
        world.last_api_status,
        Some(expected),
        "unexpected /invoke status (body: {:?})",
        world.last_api_body
    );
}

#[then(expr = "the polar-align workflow phase should be {string}")]
async fn polar_align_phase_is(world: &mut PolarAlignWorld, expected: String) {
    let client = reqwest::Client::new();
    let url = format!("{}/status", world.polar_align_url());
    let resp = client
        .get(&url)
        .send()
        .await
        .expect("failed to GET /status");
    let body: serde_json::Value = resp.json().await.expect("failed to parse /status");
    let phase = body.get("phase").and_then(|p| p.as_str()).unwrap_or("");
    assert_eq!(phase, expected, "unexpected phase (body: {body})");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn measurement_field(world: &mut PolarAlignWorld, field: &str) -> f64 {
    let client = reqwest::Client::new();
    let url = format!("{}/status", world.polar_align_url());
    let body: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("failed to GET /status")
        .json()
        .await
        .expect("failed to parse /status");
    body.get("measurement")
        .and_then(|m| m.get(field))
        .and_then(|v| v.as_f64())
        .unwrap_or_else(|| panic!("measurement.{field} missing from /status: {body}"))
}

async fn configure_default_equipment(world: &mut PolarAlignWorld) {
    if world.omnisim.is_none() {
        world.omnisim = Some(OmniSimHandle::start().await);
    }
    let alpaca_url = world
        .omnisim
        .as_ref()
        .expect("OmniSim just started")
        .base_url
        .clone();

    if world.cameras.is_empty() {
        world.cameras.push(bdd_infra::rp_harness::CameraConfig {
            id: "main-cam".to_string(),
            alpaca_url: alpaca_url.clone(),
            device_number: 0,
            cooler_targets_c: Vec::new(),
        });
    }
    if world.mount.is_none() {
        world.mount = Some(bdd_infra::rp_harness::MountConfig {
            alpaca_url,
            device_number: 0,
            settle_after_slew: None,
        });
    }
}

async fn start_polar_align_service(world: &mut PolarAlignWorld) {
    if world.polar_align.is_some() {
        return;
    }
    let config = PolarAlignWorld::polar_align_service_config();
    let config_path = write_temp_config_file("polar-align-config", &config).await;
    world.polar_align = Some(ServiceHandle::start(env!("CARGO_PKG_NAME"), &config_path).await);
}

fn register_polar_align_plugin(world: &mut PolarAlignWorld) {
    let handle = world.polar_align.as_ref().expect("polar-align not started");
    let invoke_url = format!("{}/invoke", handle.base_url);

    world.plugin_configs.push(serde_json::json!({
        "name": "polar-align",
        "type": "orchestrator",
        "invoke_url": invoke_url,
        "requires_tools": []
    }));
}

async fn start_rp_service(world: &mut PolarAlignWorld) {
    if world.rp.as_ref().is_some_and(|h| h.is_running()) {
        return;
    }

    let config = world.build_rp_config();
    world.rp = Some(start_rp(&config).await);

    assert!(
        world.wait_for_rp_healthy().await,
        "rp did not become healthy within timeout"
    );
}
