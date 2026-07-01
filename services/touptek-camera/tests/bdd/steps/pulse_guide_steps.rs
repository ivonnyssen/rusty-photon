//! ST4 pulse-guiding steps.

use ascom_alpaca::api::camera::GuideDirection;
use cucumber::when;

use crate::world::CameraWorld;

#[when(regex = r"^I try to PulseGuide on camera device (\d+) in direction (\w+) for (\d+) ms$")]
async fn try_pulse_guide(world: &mut CameraWorld, _device: u32, direction: String, millis: u64) {
    let dir = match direction.as_str() {
        "North" => GuideDirection::North,
        "South" => GuideDirection::South,
        "East" => GuideDirection::East,
        "West" => GuideDirection::West,
        other => panic!("unknown guide direction: {other}"),
    };
    world.try_pulse_guide(dir, millis).await;
}
