//! Slew, park and pulse-guide completion watchers spawned by
//! [`super::MountDevice`].
//!
//! Each watcher is a tokio task that observes mount state in the
//! background, applies the per-operation completion semantics (EQMOD
//! pickup loop, post-slew tracking restore, `at_park = true`, axis
//! restore after pulse), and clears the `slew_in_progress` /
//! `pulse_guiding_<axis>` flag so the user-visible ASCOM state lines
//! up with the wire state. All three share a top-of-loop shape:
//!
//! 1. Sleep one `polling_interval` tick.
//! 2. Bail if the operation was aborted externally
//!    ([`watcher_should_abort`]) or the transport went away.
//! 3. Read a fresh snapshot via [`watcher_poll_with_retry`]
//!    (retries a handful of transient transport errors so a single
//!    USB-CDC glitch doesn't take the watcher offline mid-slew, and
//!    issues a best-effort `:L` on exhaustion).
//! 4. Honour `blocked` reads with an instant-stop and bail.
//! 5. When both axes have stopped, run the operation-specific
//!    completion step, then drop the polling-pause guard and apply
//!    the settle delay.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ascom_alpaca::api::telescope::PierSide;
use skywatcher_motor_protocol::command::MotionMode;
use skywatcher_motor_protocol::{Axis, Command};
use tokio::sync::RwLock;
use tracing::debug;

use crate::config::MountConfig;
use crate::coordinates::{
    dec_degrees_to_ticks, encoder_to_celestial, fold_to_canonical_band, local_sidereal_time_hours,
    pickup_target_ra_ticks, sidereal_step_period, target_encoder_flipped,
};
use crate::error::StarAdvError;
use crate::transport_manager::{MountSnapshot, TransportManager};

use super::slew::{pickup_reslew_axis, stop_axis_and_wait, AXIS_STOP_TIMEOUT};
use super::{pre_flip_side_for_latitude, DriverState};

/// Minimum wallclock duration the slew watcher will keep
/// `slew_in_progress` set, regardless of how fast the mount reports
/// the goto complete. See the rationale in
/// [`spawn_slew_completion_watcher`]: it guarantees that an Alpaca
/// client polling `Slewing` shortly after issuing a slew will catch
/// the `true` value at least once. Empirically ConformU's
/// AbortSlew-test wait between starting the slew and reading
/// `Slewing` runs in the 1.0–1.5 s range, so the floor needs to be
/// noticeably above that — 2 s is comfortable. A real GTi slew of
/// any meaningful distance takes well over 2 s, so this floor is
/// invisible on hardware.
const MIN_SLEW_DWELL: Duration = Duration::from_secs(2);

/// EQMOD `RAGOTORESOLUTION` / `DEGOTORESOLUTION` — see
/// `indi-3rdparty/indi-eqmod/eqmodbase.cpp:64-66`. After the goto
/// stops, the pickup loop computes the residual against the latched
/// RA/Dec target and re-issues a corrective slew if either axis
/// exceeds this threshold (5 arc-seconds).
const PICKUP_TOLERANCE_ARCSEC: f64 = 5.0;

/// EQMOD `GOTO_ITERATIVE_LIMIT` — see
/// `indi-3rdparty/indi-eqmod/eqmodbase.cpp:64`. INDI caps the
/// pickup loop at 5 iterations to keep a pathological case (motor
/// stalled, encoder oscillating, …) from running forever.
const PICKUP_MAX_ITERATIONS: u32 = 5;

/// Consecutive `poll_axes_now` failures the slew/park watcher
/// tolerates before giving up. A single transient USB-CDC glitch
/// (queue flush race, brief renumeration, …) recovers within one
/// frame and shouldn't take the watcher offline for the rest of
/// the slew — the original "one strike and exit" policy meant any
/// pre-binding hiccup left a runaway motor with no observer. Three
/// attempts × [`WATCHER_POLL_RETRY_BACKOFF`] keeps the cumulative
/// recovery window well inside the polling cadence so a genuinely
/// blocked axis is still detected within ~1 s of the firmware
/// latching the bit.
const WATCHER_POLL_RETRY_LIMIT: u32 = 3;

/// Sleep between consecutive `poll_axes_now` retry attempts in the
/// slew/park watcher. Short enough that the cumulative
/// `WATCHER_POLL_RETRY_LIMIT × WATCHER_POLL_RETRY_BACKOFF` budget
/// stays inside the next polling tick; long enough that a tokio-
/// serial read can flush whatever junk the kernel buffered during
/// a brief CDC glitch before the next attempt.
const WATCHER_POLL_RETRY_BACKOFF: Duration = Duration::from_millis(100);

/// Returns `true` when the slew-completion watcher must bail out of
/// its current iteration: either `AbortSlew` cleared
/// `slew_in_progress`, or `set_connected(false)` closed the transport.
/// Both conditions can race in mid-iteration after the top-of-loop
/// guard has already passed, so the watcher checks this helper a
/// second time immediately before issuing any post-snapshot wire
/// commands (the EQMOD pickup re-slew or the post-slew tracking
/// restart).
pub(super) async fn watcher_should_abort(
    state: &Arc<RwLock<DriverState>>,
    transport: &TransportManager,
) -> bool {
    !state.read().await.slew_in_progress || !transport.is_available()
}

/// Retrying wrapper around [`TransportManager::poll_axes_now`] used by
/// both the slew and park completion watchers. Tolerates up to
/// [`WATCHER_POLL_RETRY_LIMIT`] consecutive transport errors so a
/// single transient USB-CDC glitch (a brief renumeration, a stale
/// kernel buffer, …) doesn't take the watcher offline for the rest
/// of a goto.
///
/// On every successful poll the snapshot is emitted at `debug` so a
/// post-mortem can reconstruct the last-known-good state observed
/// before any failure. On every failed attempt the underlying error
/// is logged at `warn` with the attempt counter.
///
/// On retry exhaustion, the helper makes a best-effort `:L` on both
/// axes before returning the underlying error: even when we can no
/// longer observe state, the firmware may still be commutating step
/// pulses, and a runaway motor with no observer is the worst case
/// the original exit-on-first-error policy created. The `:L` calls
/// are fire-and-forget — if they fail too, there's nothing useful
/// the watcher can do beyond logging and bailing.
pub(super) async fn watcher_poll_with_retry(
    transport: &TransportManager,
    context: &'static str,
) -> crate::error::Result<MountSnapshot> {
    let mut last_err: Option<StarAdvError> = None;
    for attempt in 0..WATCHER_POLL_RETRY_LIMIT {
        match transport.poll_axes_now().await {
            Ok(snap) => {
                debug!(
                    context = context,
                    ra_ticks = snap.ra.position_ticks,
                    ra_running = snap.ra.running,
                    ra_blocked = snap.ra.blocked,
                    ra_goto = snap.ra.goto,
                    dec_ticks = snap.dec.position_ticks,
                    dec_running = snap.dec.running,
                    dec_blocked = snap.dec.blocked,
                    dec_goto = snap.dec.goto,
                    "watcher snapshot"
                );
                return Ok(snap);
            }
            Err(e) => {
                tracing::warn!(
                    context = context,
                    attempt = attempt + 1,
                    limit = WATCHER_POLL_RETRY_LIMIT,
                    "watcher poll_axes_now transient error: {e}"
                );
                last_err = Some(e);
                if attempt + 1 < WATCHER_POLL_RETRY_LIMIT {
                    tokio::time::sleep(WATCHER_POLL_RETRY_BACKOFF).await;
                }
            }
        }
    }
    tracing::warn!(
        context = context,
        "watcher poll_axes_now retries exhausted — best-effort :L on both axes before bailing"
    );
    let _ = transport.send(Command::InstantStop(Axis::Ra)).await;
    let _ = transport.send(Command::InstantStop(Axis::Dec)).await;
    Err(last_err
        .unwrap_or_else(|| StarAdvError::Transport("watcher poll retries exhausted".to_string())))
}

/// Clear the per-axis `pulse_guiding_<axis>` flag. `GuideDirection`
/// only resolves to `Ra` or `Dec` (see the direction-to-axis match in
/// `MountDevice::pulse_guide`), so this helper never sees
/// `Axis::Both`. Using a boolean dispatch keeps the code exhaustive
/// without an unreachable arm.
pub(super) async fn clear_pulse_flag(state: &Arc<RwLock<DriverState>>, axis: Axis) {
    let mut s = state.write().await;
    if axis == Axis::Ra {
        s.pulse_guiding_ra = false;
    } else {
        s.pulse_guiding_dec = false;
    }
}

/// Spawn the slew-completion watcher.
///
/// Polls the snapshot every `polling_interval`. When both axes report
/// `running == false` (or the slew was aborted externally — in which
/// case `slew_in_progress` is already cleared and the watcher exits
/// immediately), runs the EQMOD-style iterative pickup loop to push
/// any RA/Dec residual under [`PICKUP_TOLERANCE_ARCSEC`], optionally
/// re-issues sidereal tracking on the RA axis (matching the design
/// doc's "if Tracking was on" branch), waits `settle`, then clears
/// `slew_in_progress`.
///
/// `tracking_was_on` is captured at slew-issue time — the live
/// `tracking_requested` flag is cleared by `slew_to_coordinates_async`
/// so `tracking()` reports the wire state during the slew, hence we
/// can't read it from `state` here.
pub(super) fn spawn_slew_completion_watcher(
    state: Arc<RwLock<DriverState>>,
    transport: Arc<TransportManager>,
    config: MountConfig,
    polling_interval: Duration,
    settle: Duration,
    tracking_was_on: bool,
) {
    let started = std::time::Instant::now();
    tokio::spawn(async move {
        // Pause the background polling task for the duration of the
        // slew. With polling paused the watcher owns the wire: pickup
        // commands fire without contending with `:j` / `:f` polls for
        // `command_lock`, and the watcher's own `poll_axes_now` reads
        // give us mount state within one wire round-trip of any
        // change — vs up to `polling_interval` of snapshot staleness
        // under the always-on polling model.
        //
        // `_poll_guard` is held by value so the polling task resumes
        // automatically on every exit path (early-return for abort,
        // disconnect, blocked-axis, panic, or normal completion).
        let _poll_guard = transport.pause_background_polling();
        let mut pickup_iterations: u32 = 0;
        // Adaptive pickup-target projection: track the instant of each
        // prior pickup re-slew so the next iteration can project the
        // residual target forward by *the actually-observed* iteration
        // duration rather than a hardcoded `polling_interval × 2`
        // multiplier. USB on the GTi sees ~400 ms per iteration; UDP
        // sees ~950 ms because the per-round-trip latency adds up
        // across the 5-frame re-slew sequence. The fixed multiplier
        // worked on USB but under-compensated on UDP by ~550 ms (~8″
        // of unaccounted LST drift per iteration). Measuring once a
        // prior iteration is available makes the projection self-tune
        // per transport.
        let mut last_pickup_at: Option<std::time::Instant> = None;
        loop {
            tokio::time::sleep(polling_interval).await;

            // External abort / disconnect path: AbortSlew clears
            // `slew_in_progress` before issuing :L; set_connected(false)
            // also clears it. Either way, bail before overwriting
            // user-visible state.
            if !state.read().await.slew_in_progress {
                return;
            }
            // Belt-and-braces: if the transport became unavailable
            // (mid-disconnect, handshake-failure rollback, ...), exit
            // even if the flag-clear hasn't happened yet. This stops
            // the watcher holding `Arc<TransportManager>` alive past
            // its useful life.
            if !transport.is_available() {
                state.write().await.slew_in_progress = false;
                return;
            }

            // Direct poll instead of reading the (now-paused) background
            // snapshot. [`watcher_poll_with_retry`] tolerates a handful
            // of transient transport errors so a single USB-CDC glitch
            // doesn't take the watcher offline mid-slew; on retry
            // exhaustion it also issues a best-effort `:L` on both
            // axes so the motor isn't left commutating with no
            // observer.
            let snap = match watcher_poll_with_retry(&transport, "slew_watcher").await {
                Ok(s) => s,
                Err(_) => {
                    state.write().await.slew_in_progress = false;
                    return;
                }
            };
            // Sky-Watcher spec §5 reports `Blocked` in the `:f`
            // status when the motor is stepping but the encoder
            // isn't advancing — typically the axis is against a
            // hard stop. Issue `:L` on both axes to halt the
            // runaway and bail out of the slew rather than letting
            // the watcher poll-loop continue while the gearbox
            // strains.
            if snap.ra.blocked || snap.dec.blocked {
                tracing::warn!(
                    ra_blocked = snap.ra.blocked,
                    dec_blocked = snap.dec.blocked,
                    "axis reports Blocked — aborting slew via :L"
                );
                let _ = transport.send(Command::InstantStop(Axis::Ra)).await;
                let _ = transport.send(Command::InstantStop(Axis::Dec)).await;
                state.write().await.slew_in_progress = false;
                return;
            }
            let still_moving = snap.ra.running || snap.dec.running;
            if still_moving {
                continue;
            }

            // Enforce a minimum slew dwell so external observers reliably
            // catch `Slewing == true`. ConformU starts a slew via HTTP,
            // then reads `Slewing` over a second HTTP call; the round-
            // trip latency can be larger than the mock's full slew
            // duration on a fast machine (the mock advances 100K
            // ticks/poll, so a small slew completes in 1-2 polls). The
            // de-facto Alpaca client poll cadence is on the order of
            // 100 ms; two full seconds of guaranteed dwell is a safe
            // floor for any reasonable client without meaningfully
            // slowing real-mount operation (real slews take seconds).
            //
            // The dwell *must* gate the pickup loop, not run after it.
            // The encoder is static while the watcher is observing
            // (tracking is off until the post-slew re-enable below),
            // so the apparent RA drifts at sidereal rate as LST
            // advances. If the pickup loop ran during the dwell wait,
            // it would re-detect that drift on every iteration and
            // burn through `PICKUP_MAX_ITERATIONS` just waiting —
            // potentially leaving a residual of one dwell-worth of
            // sidereal drift (~30") at the moment tracking re-enables.
            // Gating pickup behind the dwell means the loop sees a
            // single accumulated residual once, corrects it, then
            // hands off to tracking immediately.
            if started.elapsed() < MIN_SLEW_DWELL {
                continue;
            }

            // Both axes report stopped and the dwell has elapsed. Run
            // the EQMOD pickup loop: if either residual exceeds 5",
            // re-enter the goto sequence with a fresh delta computed
            // for the current LST. Capped at `PICKUP_MAX_ITERATIONS`
            // to match INDI's `GOTO_ITERATIVE_LIMIT`. On the GTi the
            // loop converges in 1–2 iterations because the post-stop
            // residual is bounded by the slew duration × sidereal
            // rate (~15"/s of RA drift per second of slew).
            if pickup_iterations < PICKUP_MAX_ITERATIONS {
                let (target_ra, target_dec, target_pier_side) = {
                    let s = state.read().await;
                    (s.target_ra_hours, s.target_dec_degrees, s.target_pier_side)
                };
                if let (Some(target_ra), Some(target_dec), Some(params)) =
                    (target_ra, target_dec, transport.parameters().await)
                {
                    // ERFA refuses the host UTC if `eraCal2jd`
                    // rejects the year (below `IYMIN = -4799`). A
                    // leap-second-table-out-of-range clock returns
                    // `Ok` with a warning, not an error — see the
                    // `StarAdvError::Timekeeping` rustdoc — so the
                    // realistic failure here is an absurdly-far-
                    // past clock, not a future-shifted one. Match
                    // the `poll_axes_now` failure pattern: log,
                    // clear `slew_in_progress`, exit the watcher
                    // rather than aborting the tokio task.
                    let lst = match local_sidereal_time_hours(
                        SystemTime::now(),
                        config.site_longitude_deg,
                    ) {
                        Ok(lst) => lst,
                        Err(e) => {
                            tracing::warn!("watcher LST computation failed: {e}");
                            state.write().await.slew_in_progress = false;
                            return;
                        }
                    };
                    // Flip-aware: `encoder_to_celestial` applies the
                    // post-flip RA/Dec mapping when the Dec encoder is
                    // past the pole. Without it, the residual check
                    // would interpret a successful flip as a 12-hour
                    // RA residual and the pickup loop would try to undo
                    // the flip on its first iteration.
                    let (cur_ra, cur_dec) = encoder_to_celestial(
                        snap.ra.position_ticks,
                        snap.dec.position_ticks,
                        lst,
                        params.cpr_ra,
                        params.cpr_dec,
                        config.site_latitude_deg,
                    );
                    // RA residual is on a 24-hour circle; take the
                    // shorter arc. Convert hours → arc-seconds
                    // (15°/hour × 3600″/°).
                    let ra_circ = ((target_ra - cur_ra).rem_euclid(24.0))
                        .min((cur_ra - target_ra).rem_euclid(24.0));
                    let ra_residual_arcsec = ra_circ * 15.0 * 3600.0;
                    let dec_residual_arcsec = (target_dec - cur_dec).abs() * 3600.0;
                    if ra_residual_arcsec > PICKUP_TOLERANCE_ARCSEC
                        || dec_residual_arcsec > PICKUP_TOLERANCE_ARCSEC
                    {
                        // Re-check the abort / disconnect signals
                        // immediately before issuing any wire
                        // commands. The top-of-loop guard ran one
                        // `:f` round-trip + a few coordinate ops
                        // ago; in that window AbortSlew (which
                        // clears `slew_in_progress` and issues :L)
                        // or set_connected(false) (which closes the
                        // transport) may have raced ahead. Without
                        // this second guard the pickup loop would
                        // restart motion after the user aborted.
                        if watcher_should_abort(&state, &transport).await {
                            state.write().await.slew_in_progress = false;
                            return;
                        }
                        // Pre-compensate the RA target for the LST drift
                        // that will accumulate before the next pickup
                        // iteration re-checks the residual. Without it
                        // pickup chases a moving target and the residual
                        // floor matches per-iteration sidereal drift
                        // (~6″ on USB, ~14″ on UDP). See
                        // `docs/plans/star-adventurer-gti-pickup-accuracy.md`
                        // §"Experiment B".
                        //
                        // Adaptive: use the actually-observed time delta
                        // between consecutive pickup decisions; this
                        // self-tunes for the transport's wire latency
                        // (USB ≈ 400 ms/iter, UDP ≈ 950 ms/iter).
                        // First iteration has no prior data → fall back
                        // to `polling_interval × 2` (the USB-tuned heuristic).
                        let now = std::time::Instant::now();
                        let projection = match last_pickup_at {
                            Some(t) => now.duration_since(t),
                            None => polling_interval * 2,
                        };
                        last_pickup_at = Some(now);
                        // Flip-aware target-encoder computation. With a
                        // pre-flip target side, reuse `pickup_target_ra_ticks`
                        // for the same LST pre-compensation that pre-Phase-6
                        // builds relied on. With a post-flip target side,
                        // compute the projected target via
                        // `target_encoder_flipped` so the pickup re-slew
                        // lands on the flipped encoder (past-the-pole Dec
                        // and the mirror-band RA mech_HA) rather than
                        // undoing the flip back to the pre-flip side.
                        let pre_flip_side = pre_flip_side_for_latitude(config.site_latitude_deg);
                        let target_is_flipped = target_pier_side
                            .filter(|s| *s != pre_flip_side && *s != PierSide::Unknown)
                            .is_some();
                        let (new_ra_ticks, new_dec_ticks) = if target_is_flipped {
                            let lst_proj = lst + projection.as_secs_f64() / 3600.0;
                            target_encoder_flipped(
                                target_ra,
                                target_dec,
                                lst_proj,
                                params.cpr_ra,
                                params.cpr_dec,
                            )
                        } else {
                            let new_ra =
                                pickup_target_ra_ticks(target_ra, lst, projection, params.cpr_ra);
                            let new_dec = dec_degrees_to_ticks(target_dec, params.cpr_dec);
                            (new_ra, new_dec)
                        };
                        // Fold the deltas to canonical so the pickup
                        // re-slew takes the shortest path even if the
                        // current encoder snapshot landed outside
                        // `[−cpr/2, +cpr/2)` after a through-wrap
                        // flip — see [`fold_to_canonical_band`].
                        let ra_delta = fold_to_canonical_band(
                            new_ra_ticks - snap.ra.position_ticks,
                            params.cpr_ra,
                        );
                        let dec_delta = fold_to_canonical_band(
                            new_dec_ticks - snap.dec.position_ticks,
                            params.cpr_dec,
                        );
                        pickup_iterations += 1;
                        debug!(
                            iteration = pickup_iterations,
                            ra_residual_arcsec,
                            dec_residual_arcsec,
                            projection_ms = projection.as_millis() as u64,
                            ra_delta_ticks = ra_delta,
                            "slew pickup iteration"
                        );
                        // The pickup re-slew goes through the same
                        // wire sequence as the original goto. `:L` +
                        // poll keeps the motor-not-stopped contract
                        // intact even if a previous send failed
                        // mid-sequence.
                        pickup_reslew_axis(&transport, Axis::Ra, ra_delta).await;
                        pickup_reslew_axis(&transport, Axis::Dec, dec_delta).await;
                        continue;
                    }
                }
            }

            // Slew completed cleanly. Re-enable tracking if the user had
            // it on before the slew, then apply the settle delay. Only
            // mark tracking_requested=true if the StartMotion actually
            // succeeds — otherwise Tracking() would lie about the wire
            // state. The earlier mode/period sends are best-effort but
            // failures are logged for diagnosis.
            //
            // Re-check abort / disconnect before issuing the tracking
            // wire sequence — same race-window argument as the pickup
            // loop's pre-wire guard. AbortSlew clearing `slew_in_progress`
            // between the top-of-loop check and now must skip the
            // tracking restart, or the user-visible state would say
            // "aborted" while the wire is back to tracking.
            if watcher_should_abort(&state, &transport).await {
                state.write().await.slew_in_progress = false;
                return;
            }
            if tracking_was_on {
                if let Some(params) = transport.parameters().await {
                    let period = sidereal_step_period(params.tmr_freq, params.cpr_ra);
                    if let Err(e) = transport
                        .send(Command::SetMotionMode {
                            axis: Axis::Ra,
                            mode: MotionMode::TRACKING,
                        })
                        .await
                    {
                        tracing::warn!("post-slew SetMotionMode TRACKING failed: {e}");
                    }
                    if let Err(e) = transport
                        .send(Command::SetStepPeriod {
                            axis: Axis::Ra,
                            period,
                        })
                        .await
                    {
                        tracing::warn!("post-slew SetStepPeriod failed: {e}");
                    }
                    match transport.send(Command::StartMotion(Axis::Ra)).await {
                        Ok(_) => {
                            state.write().await.tracking_requested = true;
                        }
                        Err(e) => {
                            tracing::warn!(
                                "post-slew StartMotion failed; tracking not re-enabled: {e}"
                            );
                        }
                    }
                }
            }
            // Resume background polling *now*, before the settle delay.
            // The pickup loop is done; from here on the watcher is just
            // waiting for the firmware tracking to engage (~160 ms) and
            // applying the settle margin. While we wait, the background
            // polling task should refresh the snapshot at its regular
            // cadence so an Alpaca client reading `RightAscension` right
            // after `Slewing` flips to `false` sees a snapshot that
            // reflects the encoder at its now-actively-tracking
            // position, not the watcher's last `poll_axes_now` from
            // before tracking restart. Without this, the snap is stale
            // by the duration `(tracking_engagement + settle)` and the
            // reported RA lags by that × sidereal rate (~5-10″).
            drop(_poll_guard);
            tokio::time::sleep(settle).await;
            state.write().await.slew_in_progress = false;
            return;
        }
    });
}

/// Spawn the park-completion watcher.
///
/// Same shape as [`spawn_slew_completion_watcher`] but the post-motion
/// branch sets `at_park = true` instead of re-issuing tracking. Park
/// always leaves tracking off per the ASCOM spec.
pub(super) fn spawn_park_completion_watcher(
    state: Arc<RwLock<DriverState>>,
    transport: Arc<TransportManager>,
    polling_interval: Duration,
    settle: Duration,
) {
    tokio::spawn(async move {
        // Same wire-ownership trick as the slew watcher: pause
        // background polling and drive snapshot freshness from
        // `poll_axes_now`. Park doesn't have a pickup loop so the
        // win here is smaller, but the consistency is worth it
        // and the background-polling pause also frees up the wire
        // for the `:K` + `:L` abort sequence on a blocked-axis
        // mechanical stop.
        let _poll_guard = transport.pause_background_polling();
        loop {
            tokio::time::sleep(polling_interval).await;
            // External abort / disconnect path: clears slew_in_progress.
            if !state.read().await.slew_in_progress {
                return;
            }
            // Bail if the transport became unavailable (disconnect race).
            if !transport.is_available() {
                state.write().await.slew_in_progress = false;
                return;
            }
            // See [`watcher_poll_with_retry`] in the slew watcher above:
            // tolerates transient transport errors, debug-logs every
            // successful snapshot for post-mortems, and issues a
            // best-effort `:L` on retry exhaustion so the motor halts
            // even when the wire has gone away.
            let snap = match watcher_poll_with_retry(&transport, "park_watcher").await {
                Ok(s) => s,
                Err(_) => {
                    state.write().await.slew_in_progress = false;
                    return;
                }
            };
            // Park can also hit a mechanical stop — same `:L` + bail
            // treatment as in the slew watcher. Do *not* set
            // `at_park = true` on a blocked stop: the OTA isn't at
            // the encoder-0 home pose, so the next `Unpark + slew`
            // would compute a wrong delta.
            if snap.ra.blocked || snap.dec.blocked {
                tracing::warn!(
                    ra_blocked = snap.ra.blocked,
                    dec_blocked = snap.dec.blocked,
                    "axis reports Blocked during park — aborting via :L"
                );
                let _ = transport.send(Command::InstantStop(Axis::Ra)).await;
                let _ = transport.send(Command::InstantStop(Axis::Dec)).await;
                state.write().await.slew_in_progress = false;
                return;
            }
            if snap.ra.running || snap.dec.running {
                continue;
            }
            // Resume background polling before the settle so an
            // Alpaca client reading `AtPark`-related position data
            // right after `Slewing` clears sees fresh snapshot data.
            // See the matching note in `spawn_slew_completion_watcher`.
            drop(_poll_guard);
            tokio::time::sleep(settle).await;
            let mut s = state.write().await;
            s.at_park = true;
            s.slew_in_progress = false;
            return;
        }
    });
}

/// Spawn the PulseGuide watcher.
///
/// Sleeps for `duration`, then restores prior state on the targeted
/// axis:
/// - **RA pulse**: stop-and-wait, then if `tracking_was_on_for_restore`
///   re-issue `:G1 TRACKING` + `:I1 sidereal_period` + `:J1` so the
///   user-observable `Tracking` state survives the pulse.
/// - **Dec pulse**: stop-and-wait (Dec is normally idle; no restore).
///
/// The watcher checks the per-axis `pulse_guiding_<axis>` flag before
/// the restore step and bails out if cleared (the cancellation rule:
/// any axis-mutating call clears the flag before its own wire commands
/// so the watcher steps aside). Errors during the restore are logged
/// at `warn` and swallowed — matches [`pickup_reslew_axis`].
pub(super) fn spawn_pulse_guide_watcher(
    state: Arc<RwLock<DriverState>>,
    transport: Arc<TransportManager>,
    axis: Axis,
    duration: Duration,
    tracking_was_on_for_restore: bool,
) {
    tokio::spawn(async move {
        tokio::time::sleep(duration).await;
        // Bail if the pulse was cancelled externally (another op
        // cleared the flag), the transport dropped, or the mount
        // entered a state that takes ownership of the axis
        // (slew/park).
        let still_active = {
            let s = state.read().await;
            let active = if axis == Axis::Ra {
                s.pulse_guiding_ra
            } else {
                s.pulse_guiding_dec
            };
            active && !s.at_park && !s.slew_in_progress
        };
        if !still_active || !transport.is_available() {
            clear_pulse_flag(&state, axis).await;
            return;
        }
        // Stop the axis. Any failure here means we can't safely restore
        // either, so log and bail.
        if let Err(e) = stop_axis_and_wait(&transport, axis, AXIS_STOP_TIMEOUT).await {
            tracing::warn!("pulse-guide restore stop {axis:?} failed: {e}");
            clear_pulse_flag(&state, axis).await;
            return;
        }
        // RA-only: re-issue sidereal tracking iff the user had it on
        // at issue time. Dec just stays stopped (Dec is normally idle).
        if axis == Axis::Ra && tracking_was_on_for_restore {
            // Re-check the cancellation flag before issuing the restore
            // commands — a concurrent set_tracking(false) between the
            // stop above and here would otherwise be silently undone.
            let still_want_restore = state.read().await.pulse_guiding_ra;
            if still_want_restore {
                if let Some(params) = transport.parameters().await {
                    let period = sidereal_step_period(params.tmr_freq, params.cpr_ra);
                    if let Err(e) = transport
                        .send(Command::SetMotionMode {
                            axis: Axis::Ra,
                            mode: MotionMode::TRACKING,
                        })
                        .await
                    {
                        tracing::warn!("pulse-guide restore :G1 failed: {e}");
                    } else if let Err(e) = transport
                        .send(Command::SetStepPeriod {
                            axis: Axis::Ra,
                            period,
                        })
                        .await
                    {
                        tracing::warn!("pulse-guide restore :I1 failed: {e}");
                    } else if let Err(e) = transport.send(Command::StartMotion(Axis::Ra)).await {
                        tracing::warn!("pulse-guide restore :J1 failed: {e}");
                    }
                }
            }
        }
        clear_pulse_flag(&state, axis).await;
    });
}
