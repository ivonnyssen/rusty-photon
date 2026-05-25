//! Tracking-time CW-exclusion-zone safety guard.
//!
//! The slew planner ([`super::slew`]) keeps the counterweights clear of
//! the CW exclusion zone at *slew* time, but once `Tracking = true` the
//! firmware advances the RA encoder autonomously and the driver issues
//! no further wire commands. A multi-hour imaging session that starts
//! at a safe negative `mech_HA` will drift up across the zone entry
//! (`mech_HA = +0.95` by default) with no intervention — the same
//! physical failure mode as a zone-crossing slew, just reached via
//! tracking drift. See issue #259.
//!
//! This module closes that gap with a per-connection background task
//! that watches the live encoder `mech_HA` while tracking and stops the
//! mount (`:K1`) before it can drift into the zone. The guard does
//! **not** pick a pier side or flip — it just stops, clears the
//! in-memory `Tracking` flag to match, and emits a `warn!`. The
//! operator (or higher-level automation) decides what to do next: flip
//! via `SetSideOfPier`, slew elsewhere, or park.
//!
//! It reads the snapshot the background poll loop already refreshes (it
//! does not poll the wire itself), so it is the "extension of the
//! existing poll loop" issue #259 describes without crossing into the
//! transport layer. It runs whenever the zone is active, independent of
//! [`crate::config::FlipPolicy::enabled`] — it is the safety floor that
//! keeps an unattended autoguided session from contacting hardware.

use std::sync::Arc;
use std::time::Duration;

use rusty_photon_shared_transport::Session;
use skywatcher_motor_protocol::{Axis, Command};
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, warn};

use crate::codec::SkywatcherCodec;
use crate::config::MountConfig;
use crate::manager::MountManager;
use crate::units::{Cpr, RaTicks};

use super::DriverState;

/// Device session slot, shared with the completion watchers. `Some`
/// between `set_connected(true)` and `set_connected(false)`.
type SessionSlot = Arc<RwLock<Option<Session<SkywatcherCodec>>>>;

/// Does the instantaneous encoder `mech_HA` fall inside the guarded
/// band — the CW exclusion zone `(zone_min, zone_max)` widened by
/// `margin` on both edges, i.e. the open interval
/// `(zone_min − margin, zone_max + margin)`?
///
/// The check is on a single folded `mech_HA` value (already in
/// `[−12, +12)`), not a swept path, so unlike
/// [`super::slew::canonical_path_crosses_binding_zone`] it needs no
/// 24-hour-wrap handling: the realistic zone+margin stays well within
/// the folded range.
///
/// An empty/inverted interval (`zone_min >= zone_max`) disables the
/// guard — the same convention the slew-path binding-zone check uses.
/// A non-finite or negative `margin` is treated as `0.0` (stop exactly
/// at zone entry) so a bad value fails safe rather than open. Config-file
/// margins are already rejected at load by
/// [`crate::config::MountConfig::validate`]; this sanitisation is
/// defense-in-depth for construction paths that bypass that check
/// (programmatic configs, tests).
pub(super) fn tracking_guard_breached(mech_ha: f64, zone: (f64, f64), margin: f64) -> bool {
    let (zone_min, zone_max) = zone;
    if zone_min >= zone_max {
        return false;
    }
    let margin = if margin.is_finite() && margin >= 0.0 {
        margin
    } else {
        0.0
    };
    mech_ha > zone_min - margin && mech_ha < zone_max + margin
}

/// One guard evaluation against the latest cached snapshot.
///
/// Returns `true` only when it stopped tracking. It is a no-op (returns
/// `false`) when the client has not engaged tracking, a slew is in
/// flight, the zone is disabled, the snapshot `mech_HA` is clear of the
/// band, parameters aren't cached yet, or the session closed mid-tick.
pub(super) async fn tracking_guard_tick(
    state: &Arc<RwLock<DriverState>>,
    manager: &MountManager,
    session_slot: &SessionSlot,
    zone: (f64, f64),
    margin: f64,
) -> bool {
    // Cheap gate first: only intervene while the client has tracking
    // engaged and no slew is in flight. `slew_to_coordinates_async`
    // clears `tracking_requested` for the slew's duration, so gating on
    // it already keeps the guard dormant during slews; the
    // `slew_in_progress` check is belt-and-suspenders for the brief
    // post-slew tracking-restart window.
    {
        let s = state.read().await;
        if !s.tracking_requested || s.slew_in_progress {
            return false;
        }
    }
    // `mech_HA` needs the per-axis CPR captured at handshake.
    let Some(params) = manager.parameters().await else {
        return false;
    };
    let snap = manager.snapshot().await;
    let mech_ha = RaTicks::new(snap.ra.position_ticks)
        .to_mech_ha(Cpr::new(params.cpr_ra))
        .value();
    if !tracking_guard_breached(mech_ha, zone, margin) {
        return false;
    }

    // Breached. Stop RA tracking on the wire FIRST, mirroring
    // `set_tracking(false)` (`:K1`, then clear the flag), so the
    // in-memory `Tracking` state never reports "off" while the motor is
    // still commutating. Read the device's own session slot the same
    // way `MountDevice::send` does.
    let guard = session_slot.read().await;
    let Some(session) = guard.as_ref() else {
        // Disconnected between the gate and here — nothing to stop.
        return false;
    };
    match manager.send(session, Command::StopMotion(Axis::Ra)).await {
        Ok(_) => {
            drop(guard);
            state.write().await.tracking_requested = false;
            warn!(
                mech_ha,
                zone_min = zone.0,
                zone_max = zone.1,
                margin,
                "tracking-guard: encoder mech_HA entered the CW exclusion-zone margin while \
                 tracking; stopped the mount (:K1). Tracking is now off — flip via SetSideOfPier, \
                 slew elsewhere, or park before re-engaging."
            );
            true
        }
        Err(e) => {
            // Leave `tracking_requested` set so `Tracking` keeps
            // reporting the wire truth; the next tick retries while
            // `mech_HA` stays in the band.
            warn!(
                error = %e,
                mech_ha,
                "tracking-guard: failed to stop tracking on zone approach; will retry next tick"
            );
            false
        }
    }
}

/// Spawn the per-connection tracking-time safety guard.
///
/// Called on the `set_connected(true)` 0→1 transition once the session
/// slot is populated. The task ticks at `polling_interval` (reusing the
/// background poll loop's cadence), reads the cached snapshot, and acts
/// only when [`tracking_guard_tick`] finds tracking engaged and
/// `mech_HA` inside the guarded band. It self-terminates when
/// `set_connected(false)` clears the session slot — the same disconnect
/// signal the completion watchers observe.
pub(super) fn spawn_tracking_guard(
    state: Arc<RwLock<DriverState>>,
    manager: Arc<MountManager>,
    session_slot: SessionSlot,
    config: MountConfig,
    polling_interval: Duration,
) {
    let zone = (config.binding_zone_min_hours, config.binding_zone_max_hours);
    let margin = config.tracking_guard_margin_hours;
    tokio::spawn(async move {
        let mut ticker = interval(polling_interval);
        // Skip the immediate first tick (matches the background poll
        // loop): the handshake just seeded the snapshot.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if session_slot.read().await.is_none() {
                debug!("tracking-guard: session closed, exiting");
                return;
            }
            tracking_guard_tick(&state, &manager, &session_slot, zone, margin).await;
        }
    });
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::tracking_guard_breached;
    use proptest::prelude::*;

    // Default GTi zone for the example-based cases.
    const ZONE: (f64, f64) = (0.95, 11.05);

    #[test]
    fn far_below_the_zone_is_not_breached() {
        // A session starting at mech_HA = −3 h is well clear.
        assert!(!tracking_guard_breached(-3.0, ZONE, 0.05));
    }

    #[test]
    fn inside_the_hard_zone_is_breached_even_with_zero_margin() {
        assert!(tracking_guard_breached(6.0, ZONE, 0.0));
    }

    #[test]
    fn margin_stops_tracking_before_zone_entry() {
        // mech_HA = 0.92 is below the 0.95 zone entry but inside the
        // 0.05 h margin band (0.90, 11.10) — the guard fires early.
        assert!(tracking_guard_breached(0.92, ZONE, 0.05));
        // Without the margin (0.0) the same point is still clear.
        assert!(!tracking_guard_breached(0.92, ZONE, 0.0));
    }

    #[test]
    fn band_edges_are_exclusive() {
        // The band is the open interval (zone_min − margin, zone_max + margin).
        assert!(!tracking_guard_breached(0.95 - 0.05, ZONE, 0.05));
        assert!(!tracking_guard_breached(11.05 + 0.05, ZONE, 0.05));
    }

    #[test]
    fn just_inside_each_edge_is_breached() {
        assert!(tracking_guard_breached(0.95 - 0.05 + 1e-6, ZONE, 0.05));
        assert!(tracking_guard_breached(11.05 + 0.05 - 1e-6, ZONE, 0.05));
    }

    #[test]
    fn disabled_zone_never_breaches() {
        // The world's test config disables the zone with min > max.
        assert!(!tracking_guard_breached(6.0, (24.0, 0.0), 0.05));
        // An equal pair is also empty.
        assert!(!tracking_guard_breached(6.0, (6.0, 6.0), 0.05));
    }

    #[test]
    fn negative_margin_degrades_to_zero() {
        // Treated as 0.0: the band collapses to the raw zone, so a
        // point below zone entry is clear but one inside still fires.
        assert!(!tracking_guard_breached(0.92, ZONE, -1.0));
        assert!(tracking_guard_breached(6.0, ZONE, -1.0));
    }

    #[test]
    fn non_finite_margin_degrades_to_zero() {
        assert!(!tracking_guard_breached(0.92, ZONE, f64::NAN));
        assert!(tracking_guard_breached(6.0, ZONE, f64::INFINITY));
    }

    #[test]
    fn non_finite_mech_ha_is_not_breached() {
        assert!(!tracking_guard_breached(f64::NAN, ZONE, 0.05));
    }

    proptest! {
        /// Any point strictly inside the hard zone is breached for every
        /// non-negative margin — widening the band can't exclude it.
        #[test]
        fn inside_hard_zone_always_breached(
            x in 0.951f64..11.049,
            margin in 0.0f64..2.0,
        ) {
            prop_assert!(tracking_guard_breached(x, ZONE, margin));
        }

        /// Increasing the margin never un-breaches a point: the guarded
        /// band only grows.
        #[test]
        fn margin_widening_is_monotonic(
            x in -12.0f64..12.0,
            m1 in 0.0f64..1.0,
            extra in 0.0f64..1.0,
        ) {
            if tracking_guard_breached(x, ZONE, m1) {
                prop_assert!(tracking_guard_breached(x, ZONE, m1 + extra));
            }
        }
    }
}
