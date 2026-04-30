//! Step stubs for `pointing_api.feature`.
//! Phase-2: bodies are `todo!()` so scenarios fail loudly until phase 3.

use crate::world::SkySurveyCameraWorld;
use cucumber::{given, then, when};

#[given(expr = "the camera is connected with initial pointing RA {float} Dec {float}")]
fn connected_with_pointing(_world: &mut SkySurveyCameraWorld, _ra: f64, _dec: f64) {
    todo!("phase 3: start service, connect, override initial pointing in config");
}

#[given("the camera is started but not connected")]
async fn started_not_connected(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: start service without connecting");
}

#[when(expr = "I POST RA {float} Dec {float} to the position endpoint")]
async fn post_position(_world: &mut SkySurveyCameraWorld, _ra: f64, _dec: f64) {
    todo!("phase 3: POST /sky-survey/position with given coordinates");
}

#[when(expr = "I POST RA {float} Dec {float} rotation {float} to the position endpoint")]
async fn post_position_with_rotation(
    _world: &mut SkySurveyCameraWorld,
    _ra: f64,
    _dec: f64,
    _rot: f64,
) {
    todo!("phase 3: POST /sky-survey/position with rotation field");
}

#[when("I POST a malformed JSON body to the position endpoint")]
async fn post_malformed(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: send invalid JSON, capture status");
}

#[when("I GET the position endpoint")]
async fn get_position(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: GET /sky-survey/position");
}

#[then(expr = "the response status is {int}")]
fn response_status(_world: &mut SkySurveyCameraWorld, _status: u16) {
    todo!("phase 3: assert world.last_http_status == status");
}

#[then(expr = "the position response reports RA {float} Dec {float}")]
fn response_reports_position(_world: &mut SkySurveyCameraWorld, _ra: f64, _dec: f64) {
    todo!("phase 3: parse world.last_http_body and assert ra_deg/dec_deg");
}

#[then(expr = "the position response reports rotation {float}")]
fn response_reports_rotation(_world: &mut SkySurveyCameraWorld, _rot: f64) {
    todo!("phase 3: parse world.last_http_body and assert rotation_deg");
}
