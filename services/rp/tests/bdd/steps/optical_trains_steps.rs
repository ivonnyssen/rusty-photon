//! BDD step definitions for the optical-trains configuration model
//! (`optical_trains.feature`).
//!
//! The validation scenarios reuse the plain-REST config machinery from
//! `config_rest_steps.rs` (GET / PUT `/api/config`, apply-status and
//! error-path assertions); the capture scenarios reuse the OmniSim +
//! MCP steps from `tool_steps.rs` and the document lookup steps from
//! `document_http_api_steps.rs`.

use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use serde_json::Value;

use bdd_infra::rp_harness::OpticalTrainConfig;

use crate::steps::config_rest_steps::{send_put_config, write_scenario_config};
use crate::steps::tool_steps::{add_camera, ensure_omnisim, start_rp};
use crate::world::RpWorld;

/// The reference roster + trains from rp.md § Optical Trains: two
/// cameras, two focusers, a rotator, and a filter wheel (all on
/// unreachable Alpaca URLs — equipment connects lazily and these
/// scenarios only exercise the config endpoints), a mount carrying the
/// guiding block, and the main/guide trains sharing `main-focuser`.
#[given("a temp rp config with the reference optical trains")]
fn temp_config_reference_trains(world: &mut RpWorld) {
    write_scenario_config(
        world,
        serde_json::json!({
            "cameras": [
                { "id": "main-cam", "alpaca_url": "http://127.0.0.1:1" },
                { "id": "guide-cam", "alpaca_url": "http://127.0.0.1:1" }
            ],
            "focusers": [
                { "id": "main-focuser", "alpaca_url": "http://127.0.0.1:1" },
                { "id": "guide-focuser", "alpaca_url": "http://127.0.0.1:1" }
            ],
            "rotators": [
                { "id": "falcon", "alpaca_url": "http://127.0.0.1:1" }
            ],
            "filter_wheels": [
                { "id": "main-fw", "alpaca_url": "http://127.0.0.1:1" }
            ],
            "mount": {
                "alpaca_url": "http://127.0.0.1:1",
                "guiding": { "url": "http://127.0.0.1:1" }
            },
            "optical_trains": [
                { "id": "main", "purpose": "imaging", "focal_length_mm": 1000.0,
                  "devices": ["main-focuser", "main-fw", "falcon", "main-cam"] },
                { "id": "guide", "purpose": "guiding", "focal_length_mm": 200.0,
                  "devices": ["main-focuser", "guide-focuser", "guide-cam"] }
            ]
        }),
    );
}

#[given(
    expr = "rp is running with a camera on the simulator in an imaging train with focal length {float}"
)]
async fn rp_with_camera_in_train(world: &mut RpWorld, focal_length_mm: f64) {
    ensure_omnisim(world).await;
    add_camera(world);
    world.optical_trains.push(OpticalTrainConfig {
        id: "main".to_string(),
        purpose: Some("imaging".to_string()),
        focal_length_mm: Some(focal_length_mm),
        devices: vec!["main-cam".to_string()],
    });
    start_rp(world).await;
}

#[when(expr = "I PUT \\/api\\/config with the fetched config after setting {string} to:")]
async fn put_config_with_pointer_set_docstring(world: &mut RpWorld, pointer: String, step: &Step) {
    let raw = step
        .docstring()
        .expect("this step needs a docstring with the JSON value")
        .trim();
    let value: Value =
        serde_json::from_str(raw).expect("the docstring must be valid JSON for this step");
    let mut config = world
        .fetched_config
        .clone()
        .expect("no fetched config — add a 'When I GET /api/config' step first");
    *config
        .pointer_mut(&pointer)
        .unwrap_or_else(|| panic!("pointer {pointer} not present in fetched config")) = value;
    send_put_config(world, config.to_string()).await;
}

/// Insert a key that the fetched config does not carry (the retired-key
/// scenarios): `pointer_mut` on the full pointer would fail, so resolve
/// the parent object and insert the final segment into it.
#[when(
    expr = "I PUT \\/api\\/config with the fetched config after inserting {string} set to {string}"
)]
async fn put_config_with_pointer_inserted(world: &mut RpWorld, pointer: String, raw: String) {
    let value: Value = serde_json::from_str(&raw).unwrap_or(Value::String(raw));
    let mut config = world
        .fetched_config
        .clone()
        .expect("no fetched config — add a 'When I GET /api/config' step first");
    let (parent, key) = pointer
        .rsplit_once('/')
        .unwrap_or_else(|| panic!("pointer {pointer} has no '/'"));
    let parent_value = if parent.is_empty() {
        &mut config
    } else {
        config
            .pointer_mut(parent)
            .unwrap_or_else(|| panic!("parent pointer {parent} not present in fetched config"))
    };
    parent_value
        .as_object_mut()
        .unwrap_or_else(|| panic!("parent at {parent} is not a JSON object"))
        .insert(key.to_string(), value);
    send_put_config(world, config.to_string()).await;
}

#[then(expr = "the config response body should contain {string}")]
fn config_response_body_contains(world: &mut RpWorld, needle: String) {
    let body = world
        .last_config_response_text
        .as_ref()
        .expect("no config response recorded — check the request step ran");
    assert!(
        body.contains(&needle),
        "expected the response body to contain {needle:?}; body was: {body}"
    );
}

#[then(expr = "the document body should not contain {string}")]
fn document_body_lacks_field(world: &mut RpWorld, field: String) {
    let body = document_body(world);
    assert!(
        body.get(&field).is_none(),
        "expected no '{field}' in document body, got: {:?}",
        body.get(&field)
    );
}

#[then(expr = "the document optics focal length should be {float}")]
fn document_optics_focal_length(world: &mut RpWorld, expected: f64) {
    let optics = document_optics(world);
    assert_eq!(
        optics.get("focal_length_mm").and_then(Value::as_f64),
        Some(expected),
        "unexpected optics.focal_length_mm; optics was: {optics}"
    );
}

/// Self-consistency of the documented derivation: each axis's
/// `pixel_scale_*_arcsec_per_pixel` equals
/// `206.265 × pixel_size_*_um / focal_length_mm` computed from the
/// document's own fields.
#[then("the document optics pixel scale should equal 206.265 times pixel size over focal length")]
fn document_optics_pixel_scale_consistent(world: &mut RpWorld) {
    let optics = document_optics(world).clone();
    let focal_length = optics_f64(&optics, "focal_length_mm");
    for axis in ["x", "y"] {
        let pixel_size = optics_f64(&optics, &format!("pixel_size_{axis}_um"));
        let scale = optics_f64(&optics, &format!("pixel_scale_{axis}_arcsec_per_pixel"));
        let expected = 206.265 * pixel_size / focal_length;
        assert!(
            (scale - expected).abs() < 1e-9,
            "pixel_scale_{axis}: expected {expected}, got {scale}; optics was: {optics}"
        );
    }
}

fn document_body(world: &RpWorld) -> &Value {
    world
        .last_document_response_body
        .as_ref()
        .expect("no document response body recorded")
}

fn document_optics(world: &RpWorld) -> &Value {
    document_body(world).get("optics").unwrap_or_else(|| {
        panic!(
            "document carries no optics block: {:?}",
            document_body(world)
        )
    })
}

fn optics_f64(optics: &Value, field: &str) -> f64 {
    optics
        .get(field)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("optics field {field} missing or non-numeric in {optics}"))
}
