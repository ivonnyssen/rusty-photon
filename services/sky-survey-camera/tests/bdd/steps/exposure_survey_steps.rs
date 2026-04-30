//! Step stubs for `exposure_survey.feature`.
//! Phase-2: bodies are `todo!()` so scenarios fail loudly until phase 3.

use crate::world::SkySurveyCameraWorld;
use cucumber::{given, then, when};

#[given("the survey backend returns a healthy FITS cutout")]
async fn survey_healthy(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: install MockSurveyClient with canned FITS payload");
}

#[given("the survey backend returns HTTP 500")]
async fn survey_500(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: install MockSurveyClient that always returns 500");
}

#[given("the survey backend exceeds the request timeout")]
async fn survey_timeout(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: install MockSurveyClient that sleeps past request_timeout");
}

#[given("the survey backend returns a malformed FITS body")]
async fn survey_malformed(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: install MockSurveyClient returning non-FITS bytes");
}

#[given("the cache contains a hit for the next request")]
async fn cache_hit(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: pre-seed cache_dir with a FITS file matching the next request key");
}

#[when("I StartExposure with default parameters")]
async fn start_exposure_default(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: StartExposure with Light=true, full sensor, 1s exposure");
}

#[when("I StartExposure with Light=false")]
async fn start_exposure_dark(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: StartExposure with Light=false");
}

#[then(expr = "the resulting image has dimensions {int} by {int}")]
async fn image_dimensions(_world: &mut SkySurveyCameraWorld, _w: u32, _h: u32) {
    todo!("phase 3: GET /api/v1/camera/0/imagearray, assert NumX x NumY");
}

#[then("every pixel of the resulting image is zero")]
async fn image_all_zero(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: GET imagearray, assert all values == 0");
}

#[then("the exposure fails with ASCOM UNSPECIFIED_ERROR")]
fn exposure_fails_unspecified(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: assert world.last_ascom_error == 0x500 and ImageReady=false");
}

#[then("no outbound survey HTTP request was made")]
fn no_outbound_request(_world: &mut SkySurveyCameraWorld) {
    todo!("phase 3: assert MockSurveyClient call count is zero");
}
