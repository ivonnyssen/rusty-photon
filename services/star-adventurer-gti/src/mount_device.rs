//! ASCOM Alpaca Telescope device for the Star Adventurer GTi.
//!
//! This is the surface that Alpaca clients (NINA, SGPro, `rp`, ...) talk to.
//! Capability-flag overrides match the design doc's
//! [§"Capability flags"](../../../docs/services/star-adventurer-gti.md#capability-flags)
//! table; defaulted methods that the MVP does not implement are left to the
//! ascom-alpaca trait's `NOT_IMPLEMENTED` default.
//!
//! ## Submodule layout
//!
//! - [`actions`] — the three driver-specific ASCOM `Action` handlers
//!   (`SetUnparkFromApPosition`, `SetPreferredApPark`,
//!   `UnparkFromApPosition`) dispatched from `device`'s `action`.
//! - [`device`] — `impl Device for MountDevice` (connect/description,
//!   `SupportedActions` + `Action` dispatch).
//! - [`telescope`] — `impl Telescope for MountDevice` (the ASCOM
//!   surface: coordinate reads, slew/sync/park, side-of-pier,
//!   pulse-guide).
//! - [`inherent`] — methods on `MountDevice` shared between the trait
//!   impls (validation, motion-control wrappers, post-connect lifecycle,
//!   the slew planner).
//! - [`slew`] — wire-level slew helpers (`:K`/`:G`/`:I`/`:H`/`:M`/`:J`
//!   sequence) and flip-aware delta geometry.
//! - [`watchers`] — tokio tasks observing slew / park / pulse-guide
//!   completion in the background.
//! - [`tracking_guard`] — per-connection background task that stops
//!   tracking before the encoder `mech_HA` drifts into the CW
//!   exclusion zone (issue #259).
//! - [`park_persistence`] — JSON config-file read/write for `SetPark`
//!   and the boot-time writability probe.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::telescope::PierSide;
use rusty_photon_shared_transport::Session;
use tokio::sync::RwLock;

use crate::codec::SkywatcherCodec;
use crate::config::MountConfig;
use crate::manager::MountManager;

mod actions;
mod device;
mod inherent;
mod park_persistence;
mod slew;
mod telescope;
mod tracking_guard;
mod watchers;

#[cfg(all(test, feature = "mock"))]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests;

pub use park_persistence::{
    canonicalise_config_path, probe_park_file_writability, warn_if_park_path_unwritable,
};

/// Default guide rate as a fraction of sidereal. ASCOM clients see
/// this multiplied by `SIDEREAL_DEG_PER_SEC` through
/// `GuideRateRightAscension` / `GuideRateDeclination`.
const DEFAULT_GUIDE_RATE_FRACTION: f64 = 0.5;

/// In-memory mirror of latched-from-the-client state (Tracking enabled,
/// AtPark flag, last target). The values that come from the wire (current
/// RA/Dec, Slewing) are read through [`MountManager`].
#[derive(Debug)]
struct DriverState {
    tracking_requested: bool,
    at_park: bool,
    target_ra_hours: Option<f64>,
    target_dec_degrees: Option<f64>,
    slew_settle_time: Option<Duration>,
    /// In-memory park-target encoder pair. Populated on the 0→1 connect
    /// transition from `MountConfig::park_*_ticks` if `Some`, otherwise
    /// from the handshake-captured positions. `None` here means "not
    /// loaded yet" — `Park` reads via `ok_or_else` after
    /// `ensure_connected()` so an unset value surfaces as an
    /// `ASCOMError(INVALID_OPERATION)` rather than a panic.
    park_ra_ticks: Option<i32>,
    park_dec_ticks: Option<i32>,
    /// Pier side the most recent slew was *issued for*. Read by the
    /// slew-completion watcher's pickup loop so it picks
    /// `target_encoder_normal` vs `target_encoder_flipped` for the
    /// corrective re-slew. Without this, a successful flip slew would
    /// be undone by the pickup loop's first iteration (the post-flip
    /// Dec encoder is past the pole, and a pre-flip encoder target
    /// would order a slew back through the pole).
    target_pier_side: Option<PierSide>,
    /// PulseGuide rate on the RA axis as a fraction of sidereal in
    /// `(0, 1)`. `GuideRateRightAscension` is this × `SIDEREAL_DEG_PER_SEC`.
    /// Resets to [`DEFAULT_GUIDE_RATE_FRACTION`] on each disconnect.
    guide_rate_ra_fraction: f64,
    guide_rate_dec_fraction: f64,
    /// `true` between issuing a PulseGuide on this axis and the
    /// watcher clearing the flag after the pulse `duration` has
    /// elapsed (or earlier, via the cancellation rule — any
    /// axis-mutating operation clears the flag before issuing its own
    /// wire commands so the watcher's post-sleep restore bails out).
    /// See §"PulseGuide lifecycle" in the design doc.
    pulse_guiding_ra: bool,
    pulse_guiding_dec: bool,
}

impl Default for DriverState {
    fn default() -> Self {
        Self {
            tracking_requested: false,
            at_park: false,
            target_ra_hours: None,
            target_dec_degrees: None,
            slew_settle_time: None,
            park_ra_ticks: None,
            park_dec_ticks: None,
            target_pier_side: None,
            guide_rate_ra_fraction: DEFAULT_GUIDE_RATE_FRACTION,
            guide_rate_dec_fraction: DEFAULT_GUIDE_RATE_FRACTION,
            pulse_guiding_ra: false,
            pulse_guiding_dec: false,
        }
    }
}

impl DriverState {
    /// Reset per-session client state on `set_connected(false)`.
    ///
    /// Disconnect resets the per-session client state but leaves
    /// mechanical state (`at_park`) intact — the mount's encoder
    /// doesn't move just because we closed the socket. The
    /// `slew_settle_time` override is also preserved so a client that
    /// has already tuned it keeps the value across reconnects, and
    /// `target_pier_side` is left to be overwritten by the next slew.
    ///
    /// Clear:
    ///   - `target_ra_hours` / `target_dec_degrees` — latched from a
    ///     `SetTargetRA` / `SetTargetDec` call; not durable.
    ///   - `tracking_requested` — disconnect halted tracking on the
    ///     wire (`:K1`); the in-memory flag must follow.
    ///   - `slew_in_progress` is **not** cleared here — it now lives on
    ///     [`MountDevice`] as an [`AtomicBool`], cleared synchronously by
    ///     [`SlewReservation`] on rollback and by the disconnect path in
    ///     `device.rs` (the `set_connected(false)` arm) alongside this
    ///     call. Clearing it there still tells any in-flight watcher
    ///     iteration to bail out.
    ///   - `park_ra_ticks` / `park_dec_ticks` — re-loaded on next
    ///     connect from config / handshake. Clearing here means a
    ///     mid-session edit to `MountConfig::park_*_ticks` would take
    ///     effect on reconnect.
    ///   - `pulse_guiding_*` — the pulse-guide watchers are bound to
    ///     the now-closed transport; cancellation is implicit.
    ///   - `guide_rate_*_fraction` — re-initialise to the default,
    ///     matching INDI's per-session reset.
    fn reset_for_disconnect(&mut self) {
        self.target_ra_hours = None;
        self.target_dec_degrees = None;
        self.tracking_requested = false;
        self.park_ra_ticks = None;
        self.park_dec_ticks = None;
        self.pulse_guiding_ra = false;
        self.pulse_guiding_dec = false;
        self.guide_rate_ra_fraction = DEFAULT_GUIDE_RATE_FRACTION;
        self.guide_rate_dec_fraction = DEFAULT_GUIDE_RATE_FRACTION;
    }
}

#[derive(derive_more::Debug)]
pub struct MountDevice {
    config: MountConfig,
    /// Optional config-file path. `Some` when the driver was started
    /// with `--config <path>`; `None` for `Config::default()` runs. Drives
    /// `CanSetPark` and is the destination for `SetPark` writes.
    config_file_path: Option<PathBuf>,
    /// Session held while connected. `Some` between successful
    /// `set_connected(true)` and `set_connected(false)`. The slot
    /// presence is the truth — no separate "requested" bool that can
    /// desync from the shared transport's refcount. Replaces the
    /// pre-migration `requested_connection: RwLock<bool>` flag.
    #[debug(skip)]
    session: Arc<RwLock<Option<Session<SkywatcherCodec>>>>,
    state: Arc<RwLock<DriverState>>,
    /// Slew/park "in progress" flag. Lives here as an [`AtomicBool`]
    /// rather than a [`DriverState`] field so [`SlewReservation`] can
    /// roll it back **synchronously** from `Drop` — a `Drop` impl can't
    /// `.await` the `state` `RwLock`. Set by the slew / park reservation,
    /// ORed into `slewing()` and the concurrent-motion refusals, and
    /// cleared by the completion watchers, `AbortSlew`, and disconnect.
    slew_in_progress: Arc<AtomicBool>,
    #[debug(skip)]
    manager: Arc<MountManager>,
}

impl MountDevice {
    pub fn new(config: MountConfig, manager: Arc<MountManager>) -> Self {
        Self::with_config_file_path(config, manager, None)
    }

    /// Construct with an optional config-file path. `Some(path)` enables
    /// `CanSetPark` / `SetPark` persistence; `None` leaves
    /// `CanSetPark = false` and `SetPark = NOT_IMPLEMENTED`.
    pub fn with_config_file_path(
        config: MountConfig,
        manager: Arc<MountManager>,
        config_file_path: Option<PathBuf>,
    ) -> Self {
        Self {
            config,
            config_file_path,
            session: Arc::new(RwLock::new(None)),
            state: Arc::new(RwLock::new(DriverState::default())),
            slew_in_progress: Arc::new(AtomicBool::new(false)),
            manager,
        }
    }

    /// Send one command through the device's session and return the
    /// typed response. Returns [`crate::error::StarAdvError::NotConnected`]
    /// when the session slot is empty.
    pub(super) async fn send(
        &self,
        cmd: skywatcher_motor_protocol::Command,
    ) -> crate::error::Result<skywatcher_motor_protocol::Response> {
        let guard = self.session.read().await;
        let session = guard
            .as_ref()
            .ok_or(crate::error::StarAdvError::NotConnected)?;
        self.manager.send(session, cmd).await
    }
}

/// RAII reservation of the `slew_in_progress` slot on [`MountDevice`].
///
/// Acquired before a slew or park issues any motion. While held, the
/// reservation **rolls back on drop** — clearing `slew_in_progress` — so
/// every `?` early-return on the motion-issue path (a failed wire
/// command, or a failed hand-off to the completion watcher) restores the
/// flag without an explicit clear at the call site. On the success path
/// the caller calls [`SlewReservation::dismiss`] once the completion
/// watcher has been spawned; from that point the watcher owns clearing
/// the flag.
///
/// The flag is an [`AtomicBool`] rather than a field behind the device's
/// `RwLock<DriverState>` precisely so this rollback can be a synchronous
/// store from `Drop` (a `Drop` impl cannot `.await` a `tokio::sync::RwLock`
/// write). Mirrors the synchronous rollback-on-drop guard the
/// `rusty-photon-shared-transport` `acquire()` path uses for its refcount.
#[must_use = "a dropped reservation rolls back slew_in_progress; bind it for the operation's duration"]
pub(super) struct SlewReservation {
    flag: Arc<AtomicBool>,
    armed: bool,
}

impl SlewReservation {
    /// Reserve the slot, returning the guard, or [`None`] when a slew /
    /// park is already in progress. The check-and-set is a single
    /// `compare_exchange`, so two concurrent callers can't both win the
    /// reservation (the TOCTOU-free guarantee the previous lock-guarded
    /// check-and-set gave).
    pub(super) fn try_acquire(flag: &Arc<AtomicBool>) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self {
                flag: Arc::clone(flag),
                armed: true,
            })
    }

    /// Hand the flag's lifecycle off to the completion watcher: disarm
    /// the rollback so dropping this guard leaves `slew_in_progress` set.
    /// Call only after the watcher has been successfully spawned.
    pub(super) fn dismiss(mut self) {
        self.armed = false;
    }
}

impl Drop for SlewReservation {
    fn drop(&mut self) {
        if self.armed {
            self.flag.store(false, Ordering::SeqCst);
        }
    }
}

/// Convert latitude sign into the natural pre-flip pier side: `West`
/// for the Northern Hemisphere (Polaris-side counterweight), `East`
/// for the Southern. Used everywhere the slew planner / watcher
/// needs to compare the user-requested pier side against the
/// pre-flip pose.
fn pre_flip_side_for_latitude(site_latitude_deg: f64) -> PierSide {
    if site_latitude_deg >= 0.0 {
        PierSide::West
    } else {
        PierSide::East
    }
}
