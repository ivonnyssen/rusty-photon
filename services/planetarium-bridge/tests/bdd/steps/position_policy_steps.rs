//! Steps for `position_policy.feature`: the reported-position altitude floor
//! and its zenith idle point.

use cucumber::{then, when};

use crate::world::BridgeWorld;

/// 36 sidereal seconds — generous against the LST advancing between the
/// device's snap and this step's read, tight against any real coordinate.
const IDLE_RA_TOLERANCE_HOURS: f64 = 0.01;
const IDLE_DEC_TOLERANCE_DEG: f64 = 0.01;

#[then("the reported position should be the idle point: RA = sidereal time, Dec = site latitude")]
async fn reported_idle_point(world: &mut BridgeWorld) {
    let t = world.telescope();
    let sidereal = t.sidereal_time().await.unwrap();
    let latitude = t.site_latitude().await.unwrap();
    let ra = t.right_ascension().await.unwrap();
    let dec = t.declination().await.unwrap();

    // Wrap-aware RA distance so an LST near 0h/24h cannot fail the assert.
    let mut dra = (ra - sidereal).abs() % 24.0;
    if dra > 12.0 {
        dra = 24.0 - dra;
    }
    assert!(
        dra < IDLE_RA_TOLERANCE_HOURS,
        "reported RA {ra}h is not the sidereal time {sidereal}h"
    );
    assert!(
        (dec - latitude).abs() < IDLE_DEC_TOLERANCE_DEG,
        "reported Dec {dec} deg is not the site latitude {latitude} deg"
    );
}

#[when("the client aligns at the zenith")]
async fn aligns_at_zenith(world: &mut BridgeWorld) {
    let t = world.telescope();
    let sidereal = t.sidereal_time().await.unwrap();
    let latitude = t.site_latitude().await.unwrap();
    drop(t);
    world.try_sync(sidereal, latitude).await;
    assert_eq!(world.last_error_code, None, "the zenith Align was rejected");
    world.last_synced = Some((sidereal, latitude));
}

#[then("the reported position should be the zenith")]
async fn reported_is_zenith(world: &mut BridgeWorld) {
    let (ra_synced, dec_synced) = world.last_synced.expect("no zenith Align recorded");
    let t = world.telescope();
    let ra = t.right_ascension().await.unwrap();
    let dec = t.declination().await.unwrap();
    assert!(
        (ra - ra_synced).abs() < IDLE_RA_TOLERANCE_HOURS,
        "reported RA {ra}h, expected the synced {ra_synced}h"
    );
    assert!(
        (dec - dec_synced).abs() < IDLE_DEC_TOLERANCE_DEG,
        "reported Dec {dec} deg, expected the synced {dec_synced} deg"
    );
}
