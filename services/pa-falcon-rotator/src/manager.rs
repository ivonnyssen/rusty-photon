//! Thin manager wrapping `SharedTransport<FalconCodec>` plus the
//! per-driver state both ASCOM devices read from.
//!
//! The refcount, slot, open/close transitions, command-lock arbitration,
//! and the (here unused) while-open task lifetime all live in
//! [`rusty_photon_shared_transport::SharedTransport`]. What stays here:
//!
//! * The Falcon handshake (`F#` → `FV` → `DR:0` → `FA` → `VS`, with the
//!   `FA` / `VS` results discarded — the no-cache design pulls a fresh
//!   read for every property access).
//! * The three small pieces of driver-side state pinned by the design doc:
//!   [`sync_offset`](FalconManager::sync_offset) (ASCOM `Sync` offset,
//!   driver-side per the
//!   [Sync semantics](../../../docs/services/falcon-rotator.md#sync-semantics--why-driver-side-not-sd)
//!   section); [`target_position`](FalconManager::target_position) (the
//!   sky-coordinate target of the last `Move*`); and the
//!   `last_limit_detected` edge tracker used to fire a one-shot `warn!`
//!   on the rising edge of `FA.limit_detect`.
//! * The protocol surface (`read_status`, `read_voltage_raw`,
//!   `move_mechanical`, `halt`, `set_reverse`, `sync`) that the device
//!   types call through their held `&Session<FalconCodec>`.
//!
//! No while-open task: the Falcon driver issues a fresh wire command on
//! every property read and never polls in the background. `Hooks::while_open`
//! is `None`.

use std::sync::Arc;

use rusty_photon_shared_transport::{
    Connection, Hooks, Session, SharedTransport, TransportFactory,
};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::codec::{FalconCodec, FalconCodecError, FalconResponse};
use crate::error::{FalconRotatorError, Result};
use crate::protocol::{validate_echo, Command, FalconStatus};

/// Normalise a degree value into `[0.0, 360.0)`.
///
/// Handles negative deltas (e.g. from `Move(delta < 0)`) by adding a full
/// turn before the second modulo so the result is always non-negative.
fn normalise_deg(deg: f64) -> f64 {
    ((deg % 360.0) + 360.0) % 360.0
}

/// Quantise a degree value to the `MD:nn.nn` wire precision (1/100°) by
/// rounding to two decimal places.
///
/// Without this step, `format!("{:.2}", 359.999)` rounds up to `"360.00"`,
/// which violates the documented `[0, 360)` wire range. Quantising first
/// produces `360.00` as an `f64`, which the subsequent `normalise_deg`
/// call wraps back to `0.0` before formatting — keeping the wire output
/// inside the documented range.
fn quantise_to_wire(deg: f64) -> f64 {
    (deg * 100.0).round() / 100.0
}

/// Manager wrapping the shared transport plus driver-side Falcon state.
///
/// One instance per process; both ASCOM devices (`FalconRotatorDevice`
/// and `FalconStatusSwitchDevice`) hold an `Arc<FalconManager>` and each
/// hold their own `Option<Session<FalconCodec>>`.
pub struct FalconManager {
    transport: Arc<SharedTransport<FalconCodec>>,
    sync_offset: Mutex<f64>,
    target_position: Mutex<Option<f64>>,
    last_limit_detected: Arc<Mutex<Option<bool>>>,
}

impl FalconManager {
    /// Build a manager around `factory`.
    ///
    /// The Falcon driver has no cached state to seed and no poll interval
    /// to capture, so unlike `PpbaManager` / `FocuserManager` this
    /// constructor does **not** take a `Config` — the only per-call
    /// configuration is the transport factory itself, which already owns
    /// the port path / baud rate / timeout it was built from.
    pub fn new(factory: Arc<dyn TransportFactory>) -> Arc<Self> {
        let last_limit_detected = Arc::new(Mutex::new(None));
        let last_for_hooks = Arc::clone(&last_limit_detected);
        let hooks = Hooks {
            handshake: Box::new(move |conn| {
                let last = Arc::clone(&last_for_hooks);
                Box::pin(handshake(conn, last))
            }),
            teardown: Box::new(|_| Box::pin(async {})),
            while_open: None,
        };
        let transport = SharedTransport::new(factory, FalconCodec, hooks);
        Arc::new(Self {
            transport,
            sync_offset: Mutex::new(0.0),
            target_position: Mutex::new(None),
            last_limit_detected,
        })
    }

    /// Access the shared transport so devices can acquire and release sessions.
    pub fn transport(&self) -> &Arc<SharedTransport<FalconCodec>> {
        &self.transport
    }

    /// Cheap, non-blocking snapshot — true between handshake completion
    /// and the start of teardown.
    pub fn is_available(&self) -> bool {
        self.transport.is_available()
    }

    /// Issue `FA` on the caller's session, parse it, and perform the
    /// `limit_detect` edge log.
    ///
    /// Logs `warn!` exactly once on the `None → true` or
    /// `Some(false) → true` transition of `FA.limit_detect`. The state
    /// initialises to `None` on connect, so a fresh connection whose
    /// very first observation reports `limit_detect = 1` still surfaces
    /// the warning.
    pub async fn read_status(&self, session: &Session<FalconCodec>) -> Result<FalconStatus> {
        let resp = session
            .request(Command::FullStatus)
            .await
            .map_err(FalconRotatorError::from)?;
        let status = match resp {
            FalconResponse::Status(s) => s,
            other => {
                return Err(FalconRotatorError::InvalidResponse(format!(
                    "FA returned non-status frame: {other:?}"
                )));
            }
        };

        let target = *self.target_position.lock().await;
        let mut last = self.last_limit_detected.lock().await;
        let rising_edge = match *last {
            None => status.limit_detect,
            Some(prev) => !prev && status.limit_detect,
        };
        *last = Some(status.limit_detect);
        drop(last);

        if rising_edge {
            warn!(
                "Falcon reported limit_detect after move toward {:?}",
                target
            );
        }

        Ok(status)
    }

    /// Issue `VS` on the caller's session and return the raw ADC count.
    pub async fn read_voltage_raw(&self, session: &Session<FalconCodec>) -> Result<u32> {
        let resp = session
            .request(Command::Voltage)
            .await
            .map_err(FalconRotatorError::from)?;
        match resp {
            FalconResponse::Voltage(v) => Ok(v),
            other => Err(FalconRotatorError::InvalidResponse(format!(
                "VS returned non-voltage frame: {other:?}"
            ))),
        }
    }

    /// Move to a mechanical angle on the caller's session.
    ///
    /// The caller has already applied any sync offset; this method
    /// validates finiteness, quantises to the `MD:nn.nn` wire precision,
    /// normalises into `[0, 360)`, emits the command, and returns the
    /// actual wire-quantised mechanical value that was sent.
    ///
    /// Returning the wire-quantised value matters because a near-boundary
    /// input (e.g. `359.999`) becomes `MD:0.00` on the wire; callers that
    /// cache a sky-coordinate `TargetPosition` derive it from this
    /// return value so the cached target matches what the device was
    /// actually told to reach.
    pub async fn move_mechanical(
        &self,
        session: &Session<FalconCodec>,
        target_mech_deg: f64,
    ) -> Result<f64> {
        if !target_mech_deg.is_finite() {
            return Err(FalconRotatorError::InvalidValue(format!(
                "move target must be finite, got {target_mech_deg}"
            )));
        }
        let wire_deg = normalise_deg(quantise_to_wire(target_mech_deg));
        let cmd = Command::MoveDeg(wire_deg);
        let resp = session
            .request(cmd.clone())
            .await
            .map_err(FalconRotatorError::from)?;
        let echo = expect_echo(resp, "MD")?;
        validate_echo(&cmd, &echo)?;
        Ok(wire_deg)
    }

    /// Issue `FH`, validate the `FH:1` echo, and clear the stored target.
    pub async fn halt(&self, session: &Session<FalconCodec>) -> Result<()> {
        let cmd = Command::Halt;
        let resp = session
            .request(cmd.clone())
            .await
            .map_err(FalconRotatorError::from)?;
        let echo = expect_echo(resp, "FH")?;
        validate_echo(&cmd, &echo)?;
        *self.target_position.lock().await = None;
        Ok(())
    }

    /// Read `FA` then write `FN:b` iff the device's `motor_reverse` differs.
    ///
    /// EEPROM-wear protection (design doc Reverse semantics): the Falcon
    /// persists `FN:b` to EEPROM on every write, so we read first and
    /// skip the write when the device already reports the requested value.
    pub async fn set_reverse(&self, session: &Session<FalconCodec>, want: bool) -> Result<()> {
        let current = self.read_status(session).await?.motor_reverse;
        if current == want {
            debug!(
                "set_reverse({}): device already matches, skipping FN write",
                want
            );
            return Ok(());
        }
        let cmd = Command::SetReverse(want);
        let resp = session
            .request(cmd.clone())
            .await
            .map_err(FalconRotatorError::from)?;
        let echo = expect_echo(resp, "FN")?;
        validate_echo(&cmd, &echo)?;
        Ok(())
    }

    /// Driver-side sync: store `(sky_deg - mech) mod 360` in `sync_offset`.
    ///
    /// Per the design doc Sync semantics, ASCOM `Sync` must leave
    /// `MechanicalPosition` unchanged, so the offset lives in driver
    /// memory and the Falcon's `SD` command is never issued.
    pub async fn sync(&self, session: &Session<FalconCodec>, sky_deg: f64) -> Result<()> {
        if !sky_deg.is_finite() {
            return Err(FalconRotatorError::InvalidValue(format!(
                "sync target must be finite, got {sky_deg}"
            )));
        }
        let mech = self.read_status(session).await?.position_deg;
        let offset = normalise_deg(sky_deg - mech);
        *self.sync_offset.lock().await = offset;
        debug!(
            "sync: sky={:.4} mech={:.4} → sync_offset={:.4}",
            sky_deg, mech, offset
        );
        Ok(())
    }

    /// Read the current driver-side sync offset.
    pub async fn sync_offset(&self) -> f64 {
        *self.sync_offset.lock().await
    }

    /// Store the last-requested sky-coordinate target.
    pub async fn set_target_position(&self, sky_deg: f64) {
        *self.target_position.lock().await = Some(sky_deg);
    }

    /// Clear the stored target.
    pub async fn clear_target_position(&self) {
        *self.target_position.lock().await = None;
    }

    /// Read the last-requested sky-coordinate target.
    pub async fn target_position(&self) -> Option<f64> {
        *self.target_position.lock().await
    }

    /// Reset all driver-side per-session state. Called by the devices on
    /// the 1→0 disconnect transition so a subsequent reconnect starts
    /// from a clean slate (`sync_offset = 0`, no target, no remembered
    /// limit edge).
    pub async fn clear_session_state(&self) {
        *self.sync_offset.lock().await = 0.0;
        *self.target_position.lock().await = None;
        *self.last_limit_detected.lock().await = None;
    }
}

/// Extract the trimmed echo string from a [`FalconResponse::Echo`], or
/// return an `InvalidResponse` describing the unexpected shape.
fn expect_echo(resp: FalconResponse, label: &str) -> Result<String> {
    match resp {
        FalconResponse::Echo(s) => Ok(s),
        other => Err(FalconRotatorError::InvalidResponse(format!(
            "{label} returned non-echo frame: {other:?}"
        ))),
    }
}

impl std::fmt::Debug for FalconManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FalconManager")
            .field("is_available", &self.transport.is_available())
            .finish_non_exhaustive()
    }
}

/// Connect-time handshake: `F#` → `FV` → `DR:0` → `FA` → `VS`.
///
/// The `FA` and `VS` reads are smoke tests — we just want to confirm
/// the wire format is honoured and that the device responds, so the
/// parsed results are discarded. Initialises the `last_limit_detected`
/// edge tracker to `None` so the first `read_status` after handshake
/// sees a fresh observation.
async fn handshake(
    conn: &Connection<FalconCodec>,
    last_limit_detected: Arc<Mutex<Option<bool>>>,
) -> std::result::Result<(), FalconCodecError> {
    // F# — ping
    let ping = conn.request(Command::Ping).await?;
    if !matches!(ping, FalconResponse::Ack) {
        return Err(FalconCodecError::InvalidResponse(
            "ping: expected FR_OK ack".to_string(),
        ));
    }

    // FV — firmware version (surfaced at info!)
    let fv = conn.request(Command::FirmwareVersion).await?;
    let version = match fv {
        FalconResponse::FirmwareVersion(v) => v,
        other => {
            return Err(FalconCodecError::InvalidResponse(format!(
                "firmware: expected FV reply, got {other:?}"
            )));
        }
    };
    info!("Falcon firmware v{}", version);

    // DR:0 — force de-rotation off
    let dr = conn.request(Command::DerotationOff).await?;
    match dr {
        FalconResponse::Echo(ref s) if s == "DR:0" => {}
        other => {
            return Err(FalconCodecError::InvalidResponse(format!(
                "derotation: expected DR:0 echo, got {other:?}"
            )));
        }
    }

    // FA — smoke test full status (parsed result discarded; no-cache design)
    let fa = conn.request(Command::FullStatus).await?;
    if !matches!(fa, FalconResponse::Status(_)) {
        return Err(FalconCodecError::InvalidResponse(format!(
            "full status: expected FR_OK:.. reply, got {fa:?}"
        )));
    }

    // VS — smoke test voltage
    let vs = conn.request(Command::Voltage).await?;
    if !matches!(vs, FalconResponse::Voltage(_)) {
        return Err(FalconCodecError::InvalidResponse(format!(
            "voltage: expected VS:.. reply, got {vs:?}"
        )));
    }

    // Initialise the edge tracker so the first post-connect read_status sees
    // a fresh observation (rising-edge log fires on None → true).
    *last_limit_detected.lock().await = None;
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    // ---- normalise_deg ---------------------------------------------------

    #[test]
    fn normalise_deg_zero_is_zero() {
        assert!((normalise_deg(0.0)).abs() < 1e-9);
    }

    #[test]
    fn normalise_deg_under_360_passthrough() {
        assert!((normalise_deg(180.0) - 180.0).abs() < 1e-9);
    }

    #[test]
    fn normalise_deg_wraps_positive_overflow() {
        assert!((normalise_deg(370.0) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn normalise_deg_wraps_negative_into_positive() {
        assert!((normalise_deg(-10.0) - 350.0).abs() < 1e-9);
    }

    #[test]
    fn normalise_deg_handles_two_turn_overflow() {
        assert!((normalise_deg(720.0)).abs() < 1e-9);
    }

    // ---- quantise_to_wire ------------------------------------------------

    #[test]
    fn quantise_to_wire_rounds_up_to_360_from_just_below() {
        assert!((quantise_to_wire(359.999) - 360.0).abs() < 1e-9);
    }

    #[test]
    fn quantise_to_wire_preserves_two_decimal_values() {
        assert!((quantise_to_wire(123.45) - 123.45).abs() < 1e-9);
    }

    #[test]
    fn quantise_to_wire_then_normalise_keeps_wire_in_range() {
        let v = normalise_deg(quantise_to_wire(359.999));
        assert!((v).abs() < 1e-9, "expected 0.0, got {v}");
        let formatted = format!("{v:.2}");
        assert_eq!(formatted, "0.00");
    }
}

/// Mock-backed integration tests for the manager.
///
/// Exercises the handshake + protocol API against the deterministic
/// [`MockFalconTransportFactory`](crate::mock::MockFalconTransportFactory).
/// Race / refcount / rollback invariants are tested once for everyone in
/// `rusty-photon-shared-transport`'s own test suite — not duplicated here.
#[cfg(all(test, feature = "mock"))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod mock_tests {
    use super::*;
    use crate::mock::MockFalconTransportFactory;
    use std::sync::Mutex as StdMutex;
    use tracing::Subscriber;
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::Layer;

    fn make_manager() -> (Arc<FalconManager>, Arc<MockFalconTransportFactory>) {
        let factory = Arc::new(MockFalconTransportFactory::default());
        let manager = FalconManager::new(Arc::clone(&factory) as Arc<dyn TransportFactory>);
        (manager, factory)
    }

    async fn acquire_session(manager: &FalconManager) -> Session<FalconCodec> {
        manager
            .transport()
            .acquire()
            .await
            .expect("acquire should succeed against the mock factory")
    }

    // ---- handshake + lifecycle ------------------------------------------

    #[tokio::test]
    async fn acquire_makes_available_and_runs_handshake_sequence() {
        let (manager, factory) = make_manager();
        assert!(!manager.is_available());
        let session = acquire_session(&manager).await;
        assert!(manager.is_available());

        // Pin the exact handshake sequence per the design-doc Connection
        // Lifecycle: F# → FV → DR:0 → FA → VS. A reorder or dropped
        // smoke-test command fails loudly here.
        let log = factory.command_log().await;
        assert_eq!(
            log,
            vec![
                "F#".to_string(),
                "FV".to_string(),
                "DR:0".to_string(),
                "FA".to_string(),
                "VS".to_string(),
            ],
            "handshake order must match design doc"
        );

        session.close().await.unwrap();
        assert!(!manager.is_available());
    }

    #[tokio::test]
    async fn second_acquire_reuses_transport_without_extra_commands() {
        let (manager, factory) = make_manager();
        let first = acquire_session(&manager).await;
        let after_first = factory.command_log().await;

        let second = acquire_session(&manager).await;
        assert_eq!(
            factory.command_log().await,
            after_first,
            "second acquire must not run another handshake"
        );

        // First close just drops the refcount; transport stays open.
        second.close().await.unwrap();
        assert!(manager.is_available());

        first.close().await.unwrap();
        assert!(!manager.is_available());
    }

    #[tokio::test]
    async fn close_resets_driver_state_on_subsequent_clear() {
        let (manager, factory) = make_manager();
        let session = acquire_session(&manager).await;

        manager.set_target_position(123.45).await;
        factory.set_limit_detect(true).await;
        let _ = manager.read_status(&session).await.unwrap();
        factory.set_mech_position_deg(45.0).await;
        manager.sync(&session, 90.0).await.unwrap();
        assert!((manager.sync_offset().await - 45.0).abs() < 1e-9);

        session.close().await.unwrap();
        manager.clear_session_state().await;

        assert_eq!(manager.target_position().await, None);
        assert!((manager.sync_offset().await).abs() < 1e-9);
        assert_eq!(*manager.last_limit_detected.lock().await, None);
    }

    // ---- read_status / read_voltage_raw ---------------------------------

    #[tokio::test]
    async fn read_status_returns_parsed_status() {
        let (manager, factory) = make_manager();
        factory.set_mech_position_deg(50.0).await;
        let session = acquire_session(&manager).await;

        let status = manager.read_status(&session).await.unwrap();
        assert!((status.position_deg - 50.0).abs() < 1e-9);
        assert!(!status.is_moving);
    }

    #[tokio::test]
    async fn read_voltage_raw_returns_default() {
        let (manager, factory) = make_manager();
        factory.set_voltage_raw(812).await;
        let session = acquire_session(&manager).await;
        let v = manager.read_voltage_raw(&session).await.unwrap();
        assert_eq!(v, 812);
    }

    // ---- move / halt ----------------------------------------------------

    #[tokio::test]
    async fn move_mechanical_sends_md_with_normalised_angle() {
        let (manager, factory) = make_manager();
        let session = acquire_session(&manager).await;
        factory.clear_command_log().await;

        manager.move_mechanical(&session, -30.0).await.unwrap(); // → 330°
        let log = factory.command_log().await;
        assert_eq!(log, vec!["MD:330.00".to_string()]);
    }

    #[tokio::test]
    async fn move_mechanical_rejects_non_finite() {
        let (manager, factory) = make_manager();
        let session = acquire_session(&manager).await;
        factory.clear_command_log().await;

        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let err = manager.move_mechanical(&session, bad).await.unwrap_err();
            assert!(
                matches!(err, FalconRotatorError::InvalidValue(_)),
                "expected InvalidValue for {bad}, got {err:?}"
            );
        }
        assert!(
            factory.command_log().await.is_empty(),
            "no command should reach the wire for non-finite targets"
        );
    }

    #[tokio::test]
    async fn move_mechanical_wraps_just_under_360_to_zero() {
        let (manager, factory) = make_manager();
        let session = acquire_session(&manager).await;
        factory.clear_command_log().await;

        manager.move_mechanical(&session, 359.999).await.unwrap();
        assert_eq!(factory.command_log().await, vec!["MD:0.00".to_string()]);
    }

    #[tokio::test]
    async fn move_mechanical_returns_wire_quantised_value() {
        let (manager, factory) = make_manager();
        let session = acquire_session(&manager).await;
        factory.clear_command_log().await;

        let wire = manager.move_mechanical(&session, 359.999).await.unwrap();
        assert!(
            (wire - 0.0).abs() < 1e-9,
            "expected wire-quantised 0.0, got {wire}"
        );

        let wire = manager.move_mechanical(&session, 123.456).await.unwrap();
        assert!(
            (wire - 123.46).abs() < 1e-9,
            "expected wire-quantised 123.46, got {wire}"
        );
    }

    #[tokio::test]
    async fn move_mechanical_exact_360_wraps_to_zero() {
        let (manager, factory) = make_manager();
        let session = acquire_session(&manager).await;
        factory.clear_command_log().await;

        manager.move_mechanical(&session, 360.0).await.unwrap();
        assert_eq!(factory.command_log().await, vec!["MD:0.00".to_string()]);
    }

    #[tokio::test]
    async fn halt_sends_fh_and_clears_target() {
        let (manager, factory) = make_manager();
        let session = acquire_session(&manager).await;
        manager.set_target_position(180.0).await;
        factory.clear_command_log().await;

        manager.halt(&session).await.unwrap();
        assert_eq!(factory.command_log().await, vec!["FH".to_string()]);
        assert_eq!(manager.target_position().await, None);
    }

    // ---- set_reverse (EEPROM-wear protection) --------------------------

    #[tokio::test]
    async fn set_reverse_skips_write_when_equal() {
        let (manager, factory) = make_manager();
        factory.set_motor_reverse(true).await;
        let session = acquire_session(&manager).await;
        factory.clear_command_log().await;

        manager.set_reverse(&session, true).await.unwrap();
        // Only the FA read; no FN write.
        assert_eq!(factory.command_log().await, vec!["FA".to_string()]);
    }

    #[tokio::test]
    async fn set_reverse_writes_when_different() {
        let (manager, factory) = make_manager();
        // Mock starts with reverse=false.
        let session = acquire_session(&manager).await;
        factory.clear_command_log().await;

        manager.set_reverse(&session, true).await.unwrap();
        // Reads FA first, then writes FN:1.
        assert_eq!(
            factory.command_log().await,
            vec!["FA".to_string(), "FN:1".to_string()]
        );
    }

    // ---- sync (driver-side offset) -------------------------------------

    #[tokio::test]
    async fn sync_offset_arithmetic() {
        let (manager, factory) = make_manager();
        factory.set_mech_position_deg(120.0).await;
        let session = acquire_session(&manager).await;

        // mech = 120°, sync to 30° → offset = (30 - 120) mod 360 = 270.
        manager.sync(&session, 30.0).await.unwrap();
        let offset = manager.sync_offset().await;
        assert!(
            (offset - 270.0).abs() < 1e-9,
            "expected offset 270.0, got {offset}"
        );
    }

    #[tokio::test]
    async fn sync_rejects_non_finite() {
        let (manager, _factory) = make_manager();
        let session = acquire_session(&manager).await;
        let starting_offset = manager.sync_offset().await;

        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let err = manager.sync(&session, bad).await.unwrap_err();
            assert!(
                matches!(err, FalconRotatorError::InvalidValue(_)),
                "expected InvalidValue for {bad}, got {err:?}"
            );
        }
        assert!((manager.sync_offset().await - starting_offset).abs() < 1e-9);
    }

    // ---- limit_detect edge log -----------------------------------------

    /// Tracing layer that counts events at WARN level. We use a counter
    /// rather than capturing the full event text so the assertion stays
    /// resilient to changes in the warning format.
    #[derive(Clone, Default)]
    struct WarnCounter(Arc<StdMutex<u32>>);

    impl<S: Subscriber> Layer<S> for WarnCounter {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            if event.metadata().level() == &tracing::Level::WARN {
                *self.0.lock().unwrap() += 1;
            }
        }
    }

    impl WarnCounter {
        fn count(&self) -> u32 {
            *self.0.lock().unwrap()
        }
    }

    #[tokio::test]
    async fn limit_detect_edge_log_fires_once_on_rising_edge() {
        let counter = WarnCounter::default();
        let subscriber = tracing_subscriber::registry().with(counter.clone());
        let _guard = tracing::subscriber::set_default(subscriber);

        let (manager, factory) = make_manager();
        let session = acquire_session(&manager).await;

        // First read: limit_detect=false. State: None → Some(false). No warn.
        let _ = manager.read_status(&session).await.unwrap();
        assert_eq!(counter.count(), 0);

        // Flip the device's flag and read again. State: Some(false) →
        // Some(true). One warn fires.
        factory.set_limit_detect(true).await;
        let _ = manager.read_status(&session).await.unwrap();
        assert_eq!(
            counter.count(),
            1,
            "expected exactly one warn on rising edge"
        );

        // Same flag again: Some(true) → Some(true). No new warn.
        let _ = manager.read_status(&session).await.unwrap();
        assert_eq!(counter.count(), 1, "no new warn while flag stays high");
    }

    #[tokio::test]
    async fn limit_detect_edge_log_fires_on_first_observation_when_high() {
        let counter = WarnCounter::default();
        let subscriber = tracing_subscriber::registry().with(counter.clone());
        let _guard = tracing::subscriber::set_default(subscriber);

        let (manager, factory) = make_manager();
        factory.set_limit_detect(true).await;
        let session = acquire_session(&manager).await;

        // The handshake's FA doesn't pass through read_status, so the edge
        // tracker is still None when read_status fires for real.
        // None → Some(true) should warn.
        let _ = manager.read_status(&session).await.unwrap();
        assert_eq!(counter.count(), 1);
    }

    // ---- Debug ----------------------------------------------------------

    #[tokio::test]
    async fn debug_representation_contains_struct_name() {
        let (manager, _) = make_manager();
        let debug_str = format!("{manager:?}");
        assert!(debug_str.contains("FalconManager"));
    }
}
