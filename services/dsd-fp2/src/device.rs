//! ASCOM Alpaca CoverCalibrator device for the Deep Sky Dad FP2.
//!
//! Holds an `Arc<FlatPanelManager>` (the service-wide façade over
//! `SharedTransport<Fp2Codec>`) plus a per-device session slot.
//!
//! Cover and calibrator state derive from the cached snapshot the manager's
//! while-open task refreshes. Writes (open/close/calibrator-on/-off) go
//! through `Session::request` so they share the same request arbitration
//! lock as the poll loop.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::cover_calibrator::{CalibratorStatus, CoverStatus};
use ascom_alpaca::api::{CoverCalibrator, Device};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use async_trait::async_trait;
use rusty_photon_service_lifecycle::ReloadSignal;
use rusty_photon_shared_transport::Session;
use tokio::sync::RwLock;
use tracing::debug;

use crate::codec::Fp2Codec;
use crate::config::{CliOverrides, Config, CoverCalibratorConfig};
use crate::config_actions::{
    self, ApplyStatus, ConfigAction, ConfigApplyResponse, ConfigGetResponse, FieldError,
};
use crate::error::DsdFp2Error;
use crate::manager::FlatPanelManager;
use crate::protocol::{Command, CLOSED_ANGLE, MAX_BRIGHTNESS, OPEN_ANGLE};

/// Delay before firing the reload so the `config.apply` HTTP response — served
/// by the very server the reload tears down — flushes before the blip.
const RELOAD_AFTER_RESPONSE_DELAY: Duration = Duration::from_millis(100);

/// State the `config.get` / `config.apply` actions need: the effective config
/// this server instance is running, where to persist edits, which fields are
/// pinned by a CLI override, and the in-process reload trigger.
#[derive(Clone)]
pub struct ConfigActionCtx {
    /// Effective config (file + CLI overrides) this server was built with.
    pub effective: Config,
    /// Where `config.apply` persists; what reload re-reads.
    pub path: PathBuf,
    /// CLI overrides, so config actions can distinguish file vs. override layers.
    pub overrides: CliOverrides,
    /// Fired (after the response flushes) to trigger the in-process reload.
    pub reload: ReloadSignal,
}

/// Deep Sky Dad FP2 as an ASCOM CoverCalibrator.
#[derive(derive_more::Debug)]
pub struct DsdFp2Device {
    config: CoverCalibratorConfig,
    #[debug(skip)]
    manager: Arc<FlatPanelManager>,
    #[debug(skip)]
    session: Arc<RwLock<Option<Session<Fp2Codec>>>>,
    /// `Some` when the driver was built with a config source (the normal path
    /// through `ServerBuilder`); `None` for focused unit-test devices that
    /// don't exercise config actions.
    #[debug(skip)]
    config_ctx: Option<ConfigActionCtx>,
}

impl DsdFp2Device {
    pub fn new(config: CoverCalibratorConfig, manager: Arc<FlatPanelManager>) -> Self {
        Self {
            config,
            manager,
            session: Arc::new(RwLock::new(None)),
            config_ctx: None,
        }
    }

    /// Attach the config-action context, enabling `config.get` / `config.apply`.
    pub fn with_config_actions(mut self, ctx: ConfigActionCtx) -> Self {
        self.config_ctx = Some(ctx);
        self
    }

    /// Hardware-clamped configurable maximum. ASCOM `MaxBrightness` and
    /// `calibrator_on`'s validation share this so they can't disagree.
    fn effective_max_brightness(&self) -> u32 {
        self.config.max_brightness.min(MAX_BRIGHTNESS as u32)
    }

    /// `config.get`: return the effective config (secrets redacted) plus the
    /// CLI-override-pinned field paths.
    async fn handle_config_get(&self, ctx: &ConfigActionCtx) -> ASCOMResult<String> {
        let mut config = serde_json::to_value(&ctx.effective).map_err(config_action_error)?;
        config_actions::redact_value(&mut config);
        let response = ConfigGetResponse {
            config,
            overrides: ctx.overrides.pinned_paths(),
        };
        serde_json::to_string(&response).map_err(config_action_error)
    }

    /// `config.apply`: parse → validate → persist (layer-aware) → classify →
    /// fire the reload after the response flushes.
    async fn handle_config_apply(
        &self,
        ctx: &ConfigActionCtx,
        parameters: &str,
    ) -> ASCOMResult<String> {
        let submitted: Config = match serde_json::from_str(parameters) {
            Ok(config) => config,
            Err(e) => {
                return Err(ASCOMError::new(
                    ASCOMErrorCode::INVALID_VALUE,
                    format!("config.apply: invalid config JSON: {e}"),
                ))
            }
        };

        // Validation failure is a domain error: HTTP 200, file untouched.
        let errors = config_actions::validate(&submitted);
        if !errors.is_empty() {
            return serde_json::to_string(&ConfigApplyResponse::invalid(errors))
                .map_err(config_action_error);
        }

        // Build the value to persist: write through CLI-override-pinned fields
        // and round-tripped secrets from the file's current value. A present-
        // but-corrupt file is surfaced rather than silently treated as default
        // (which would overwrite and lose its contents).
        let file_current = match config_actions::read_file_value(&ctx.path) {
            Ok(value) => value,
            Err(msg) => {
                return Err(ASCOMError::new(
                    ASCOMErrorCode::INVALID_OPERATION,
                    format!("config.apply: {msg}"),
                ))
            }
        };

        // The redaction sentinel means "keep the stored secret unchanged". If
        // there is no stored secret to keep, the sentinel can't be honoured —
        // persisting it verbatim would bake "********" in as the real password
        // hash. Reject it as a domain error so the caller supplies a real hash.
        if config_actions::redacted_secret_without_prior(&submitted, &file_current) {
            let errors = vec![FieldError {
                path: "server.auth.password_hash".to_string(),
                msg:
                    "cannot keep an unchanged secret when none is stored; provide the password hash"
                        .to_string(),
            }];
            return serde_json::to_string(&ConfigApplyResponse::invalid(errors))
                .map_err(config_action_error);
        }

        let (to_persist, skipped) =
            config_actions::build_persist_value(&submitted, &file_current, &ctx.overrides)
                .map_err(config_action_error)?;

        // Classify what would change in the running (effective) config.
        let new_effective = config_actions::effective_value(&to_persist, &ctx.overrides);
        let running = serde_json::to_value(&ctx.effective).map_err(config_action_error)?;
        let changed = config_actions::diff_paths(&running, &new_effective);

        config_actions::save(&ctx.path, &to_persist).map_err(|e| {
            ASCOMError::new(
                ASCOMErrorCode::INVALID_OPERATION,
                format!("config.apply: failed to persist config: {e}"),
            )
        })?;
        debug!(path = %ctx.path.display(), ?changed, "config.apply persisted");

        let status = if changed.is_empty() {
            ApplyStatus::Ok
        } else {
            // Fire-after-response: the server serving this request is the one
            // the reload tears down, so yield until the response has flushed.
            let reload = ctx.reload.clone();
            tokio::spawn(async move {
                tokio::time::sleep(RELOAD_AFTER_RESPONSE_DELAY).await;
                debug!("firing in-process reload after config.apply");
                reload.notify();
            });
            ApplyStatus::Applying
        };

        let response = ConfigApplyResponse {
            status,
            applied: Vec::new(),
            reload: changed,
            restart_required: Vec::new(),
            skipped_override: skipped,
            persisted_to: Some(ctx.path.display().to_string()),
            errors: Vec::new(),
        };
        serde_json::to_string(&response).map_err(config_action_error)
    }
}

/// Map an internal config-action failure (serialization, persistence) to an
/// ASCOM error. Consistent with `error.rs`, IO/operation failures use
/// `INVALID_OPERATION`.
fn config_action_error(e: serde_json::Error) -> ASCOMError {
    ASCOMError::new(
        ASCOMErrorCode::INVALID_OPERATION,
        format!("config action serialization failed: {e}"),
    )
}

#[async_trait]
impl Device for DsdFp2Device {
    fn static_name(&self) -> &str {
        &self.config.name
    }

    fn unique_id(&self) -> &str {
        &self.config.unique_id
    }

    async fn description(&self) -> ASCOMResult<String> {
        Ok(self.config.description.clone())
    }

    async fn connected(&self) -> ASCOMResult<bool> {
        Ok(self.session.read().await.is_some() && self.manager.transport().is_available())
    }

    async fn set_connected(&self, connected: bool) -> ASCOMResult<()> {
        // Hold the write lock across the entire check-and-modify so two
        // concurrent `Connected=true` requests for this device can't both
        // observe `session.is_none()`, both call `transport.acquire()`,
        // and end up with the session refcount diverging from the
        // single per-device slot.
        let mut slot = self.session.write().await;
        let already_open = slot.is_some() && self.manager.transport().is_available();
        if already_open == connected {
            return Ok(());
        }
        match connected {
            true => {
                // `?` does SessionError<DsdFp2Error> → DsdFp2Error via the
                // From impl in error.rs, then DsdFp2Error → ASCOMError via
                // the second From impl on `?`.
                let session = self
                    .manager
                    .transport()
                    .acquire()
                    .await
                    .map_err(DsdFp2Error::from)?;
                *slot = Some(session);
                debug!("FP2 device connected");
            }
            false => {
                if let Some(session) = slot.take() {
                    // `Session::close` returns `Result<_, TransportError>`;
                    // `From<TransportError> for DsdFp2Error` handles the
                    // first hop, and `From<DsdFp2Error> for ASCOMError`
                    // does the second on `?`.
                    session.close().await.map_err(DsdFp2Error::from)?;
                    debug!("FP2 device disconnected");
                }
            }
        }
        Ok(())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("Deep Sky Dad FP2 Driver - ASCOM Alpaca CoverCalibrator".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }

    async fn supported_actions(&self) -> ASCOMResult<Vec<String>> {
        // Only advertise the config actions when this instance was built with a
        // config source; a ctx-less device (focused unit tests) has none.
        if self.config_ctx.is_some() {
            Ok(ConfigAction::ALL
                .iter()
                .map(|action| action.name().to_string())
                .collect())
        } else {
            Ok(Vec::new())
        }
    }

    async fn action(&self, action: String, parameters: String) -> ASCOMResult<String> {
        let Some(parsed) = ConfigAction::from_name(&action) else {
            return Err(ASCOMError::new(
                ASCOMErrorCode::ACTION_NOT_IMPLEMENTED,
                format!("unknown action {action:?}"),
            ));
        };
        let ctx = self.config_ctx.as_ref().ok_or_else(|| {
            ASCOMError::new(
                ASCOMErrorCode::ACTION_NOT_IMPLEMENTED,
                "config actions are not configured for this device instance",
            )
        })?;
        match parsed {
            ConfigAction::Get => self.handle_config_get(ctx).await,
            ConfigAction::Apply => self.handle_config_apply(ctx, &parameters).await,
        }
    }
}

#[async_trait]
impl CoverCalibrator for DsdFp2Device {
    async fn cover_state(&self) -> ASCOMResult<CoverStatus> {
        if !self.connected().await? {
            return Ok(CoverStatus::Unknown);
        }
        let snap = self.manager.snapshot();
        let state = snap.read().await.clone();
        Ok(derive_cover_state(state.motor_running, state.cover_raw))
    }

    async fn calibrator_state(&self) -> ASCOMResult<CalibratorStatus> {
        if !self.connected().await? {
            return Ok(CalibratorStatus::Unknown);
        }
        let snap = self.manager.snapshot();
        let state = snap.read().await.clone();
        Ok(derive_calibrator_state(state.light_on))
    }

    async fn brightness(&self) -> ASCOMResult<u32> {
        if !self.connected().await? {
            return Err(ASCOMError::NOT_CONNECTED);
        }
        let snap = self.manager.snapshot();
        let state = snap.read().await.clone();
        Ok(state.brightness.unwrap_or(0) as u32)
    }

    async fn max_brightness(&self) -> ASCOMResult<u32> {
        Ok(self.effective_max_brightness())
    }

    async fn open_cover(&self) -> ASCOMResult<()> {
        execute_move(self, OPEN_ANGLE).await
    }

    async fn close_cover(&self) -> ASCOMResult<()> {
        execute_move(self, CLOSED_ANGLE).await
    }

    /// The FP2 firmware has no halt-motion opcode; once `[SMOV]` starts a
    /// move it runs to completion. The ASCOM ICoverCalibratorV2 spec
    /// requires `HaltCover` to throw `MethodNotImplementedException`
    /// "if cover movement cannot be interrupted" — see
    /// <https://ascom-standards.org/newdocs/covercalibrator.html>. We
    /// honour that here.
    ///
    /// **Known ConformU divergence.** ConformU 4.3 flags this as an
    /// "issue" anyway because `CoverCalibratorTester.TestHaltCover` does
    /// not distinguish `MethodNotImplementedException` from other
    /// exceptions in its async-cover branch (it treats every exception
    /// as `Required.MustBeImplemented`). See
    /// `docs/services/dsd-fp2.md` "Known limitations" for the upstream
    /// bug report; the driver is intentionally spec-compliant.
    async fn halt_cover(&self) -> ASCOMResult<()> {
        Err(ASCOMError::new(
            ASCOMErrorCode::NOT_IMPLEMENTED,
            "HaltCover not implemented: FP2 firmware cannot interrupt an in-progress cover \
             movement. Per ICoverCalibratorV2, HaltCover MUST throw MethodNotImplementedException \
             when cover movement cannot be interrupted.",
        ))
    }

    async fn calibrator_on(&self, brightness: u32) -> ASCOMResult<()> {
        // Validate against the effective max (the lower of the config cap
        // and the hardware ceiling) so the value MaxBrightness advertises
        // and the value calibrator_on accepts agree.
        let effective_max = self.effective_max_brightness();
        if brightness > effective_max {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!("brightness {brightness} exceeds configured max {effective_max}"),
            ));
        }
        let value = FlatPanelManager::validate_brightness(brightness)?;
        let slot = self.session.read().await;
        let session = slot.as_ref().ok_or(ASCOMError::NOT_CONNECTED)?;

        session
            .request(Command::SetBrightness(value))
            .await
            .map_err(DsdFp2Error::from)?
            .parse_ok()?;
        session
            .request(Command::SetLight(true))
            .await
            .map_err(DsdFp2Error::from)?
            .parse_ok()?;

        let snap = self.manager.snapshot();
        let mut state = snap.write().await;
        state.brightness = Some(value);
        state.light_on = Some(true);
        Ok(())
    }

    async fn calibrator_off(&self) -> ASCOMResult<()> {
        let slot = self.session.read().await;
        let session = slot.as_ref().ok_or(ASCOMError::NOT_CONNECTED)?;
        session
            .request(Command::SetLight(false))
            .await
            .map_err(DsdFp2Error::from)?
            .parse_ok()?;
        let snap = self.manager.snapshot();
        let mut state = snap.write().await;
        state.light_on = Some(false);
        Ok(())
    }
}

/// Drive the cover to a target angle (`open_cover` / `close_cover`).
async fn execute_move(device: &DsdFp2Device, angle: u16) -> ASCOMResult<()> {
    let slot = device.session.read().await;
    let session = slot.as_ref().ok_or(ASCOMError::NOT_CONNECTED)?;

    session
        .request(Command::SetTarget(angle))
        .await
        .map_err(DsdFp2Error::from)?
        .parse_ok()?;
    session
        .request(Command::StartMove)
        .await
        .map_err(DsdFp2Error::from)?
        .parse_ok()?;

    // Mark motor as running locally so `cover_state` reports `Moving`
    // immediately, before the next poll observes it.
    let snap = device.manager.snapshot();
    snap.write().await.motor_running = Some(true);
    Ok(())
}

/// Derive `CoverStatus` from cached state.
fn derive_cover_state(motor_running: Option<bool>, cover_raw: Option<i32>) -> CoverStatus {
    match (motor_running, cover_raw) {
        (Some(true), _) => CoverStatus::Moving,
        (Some(false), Some(0)) => CoverStatus::Closed,
        (Some(false), Some(1)) => CoverStatus::Open,
        (Some(false), Some(_)) => CoverStatus::Unknown,
        _ => CoverStatus::Unknown,
    }
}

/// Derive `CalibratorStatus` from cached state. There's no `On` variant in
/// the ASCOM enum — `Ready` is what callers expect when the lamp is lit
/// and stable, which the FP2 always is (no warm-up).
fn derive_calibrator_state(light_on: Option<bool>) -> CalibratorStatus {
    match light_on {
        Some(true) => CalibratorStatus::Ready,
        Some(false) => CalibratorStatus::Off,
        None => CalibratorStatus::Unknown,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn derive_cover_state_table_matches_spec() {
        // Motor running → Moving regardless of GOPS
        assert_eq!(derive_cover_state(Some(true), Some(0)), CoverStatus::Moving);
        assert_eq!(derive_cover_state(Some(true), Some(1)), CoverStatus::Moving);
        assert_eq!(derive_cover_state(Some(true), None), CoverStatus::Moving);

        // Motor stopped → use GOPS
        assert_eq!(
            derive_cover_state(Some(false), Some(0)),
            CoverStatus::Closed
        );
        assert_eq!(derive_cover_state(Some(false), Some(1)), CoverStatus::Open);
        // GOPS in-between → Unknown
        assert_eq!(
            derive_cover_state(Some(false), Some(255)),
            CoverStatus::Unknown
        );

        // No data → Unknown
        assert_eq!(derive_cover_state(None, None), CoverStatus::Unknown);
        assert_eq!(derive_cover_state(Some(false), None), CoverStatus::Unknown);
    }

    #[test]
    fn derive_calibrator_state_table_matches_spec() {
        assert_eq!(derive_calibrator_state(Some(true)), CalibratorStatus::Ready);
        assert_eq!(derive_calibrator_state(Some(false)), CalibratorStatus::Off);
        assert_eq!(derive_calibrator_state(None), CalibratorStatus::Unknown);
    }
}

#[cfg(all(test, feature = "mock"))]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod mock_tests {
    use super::*;
    use crate::config::{Config, CoverCalibratorConfig, SerialConfig, ServerConfig};
    use crate::mock::MockTransportFactory;
    use std::time::Duration;

    fn test_config() -> Config {
        Config {
            serial: SerialConfig {
                port: "/dev/mock".to_string(),
                polling_interval: Duration::from_secs(60),
                ..Default::default()
            },
            server: ServerConfig {
                port: 0,
                discovery_port: None,
                tls: None,
                auth: None,
            },
            cover_calibrator: CoverCalibratorConfig::default(),
        }
    }

    fn make_device() -> (DsdFp2Device, MockTransportFactory) {
        let factory = MockTransportFactory::default();
        let manager = FlatPanelManager::new(test_config(), Arc::new(factory.clone()));
        let device = DsdFp2Device::new(CoverCalibratorConfig::default(), manager);
        (device, factory)
    }

    fn make_device_with_cap(cap: u32) -> (DsdFp2Device, MockTransportFactory) {
        let factory = MockTransportFactory::default();
        let manager = FlatPanelManager::new(test_config(), Arc::new(factory.clone()));
        let cc_config = CoverCalibratorConfig {
            max_brightness: cap,
            ..CoverCalibratorConfig::default()
        };
        let device = DsdFp2Device::new(cc_config, manager);
        (device, factory)
    }

    #[tokio::test]
    async fn device_starts_disconnected() {
        let (device, _) = make_device();
        assert!(!device.connected().await.unwrap());
        // Pre-connect reads return Unknown without error.
        assert_eq!(device.cover_state().await.unwrap(), CoverStatus::Unknown);
        assert_eq!(
            device.calibrator_state().await.unwrap(),
            CalibratorStatus::Unknown
        );
    }

    #[tokio::test]
    async fn brightness_read_when_disconnected_errors() {
        let (device, _) = make_device();
        let err = device.brightness().await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn set_connected_acquires_and_releases() {
        let (device, _) = make_device();
        device.set_connected(true).await.unwrap();
        assert!(device.connected().await.unwrap());
        device.set_connected(false).await.unwrap();
        assert!(!device.connected().await.unwrap());
    }

    #[tokio::test]
    async fn set_connected_is_idempotent() {
        let (device, _) = make_device();
        device.set_connected(true).await.unwrap();
        device.set_connected(true).await.unwrap(); // no-op
        assert!(device.connected().await.unwrap());
        device.set_connected(false).await.unwrap();
        device.set_connected(false).await.unwrap(); // no-op
        assert!(!device.connected().await.unwrap());
    }

    #[tokio::test]
    async fn calibrator_on_then_off_round_trips() {
        let (device, factory) = make_device();
        device.set_connected(true).await.unwrap();
        device.calibrator_on(2048).await.unwrap();
        assert_eq!(
            device.calibrator_state().await.unwrap(),
            CalibratorStatus::Ready
        );
        assert_eq!(device.brightness().await.unwrap(), 2048);
        assert_eq!(factory.state().brightness().await, 2048);
        assert!(factory.state().light_on().await);

        device.calibrator_off().await.unwrap();
        assert_eq!(
            device.calibrator_state().await.unwrap(),
            CalibratorStatus::Off
        );
        // Brightness retained as commanded (firmware behaviour).
        assert_eq!(device.brightness().await.unwrap(), 2048);
        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn calibrator_on_rejects_brightness_above_max() {
        let (device, _) = make_device();
        device.set_connected(true).await.unwrap();
        let err = device
            .calibrator_on(MAX_BRIGHTNESS as u32 + 1)
            .await
            .unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::INVALID_VALUE);
        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn open_cover_then_close_cover_change_observable_state() {
        let (device, _) = make_device();
        device.set_connected(true).await.unwrap();

        // Default is closed (mock `cover_angle = 270`). Close again to
        // exercise the wire path even though it's a no-op for the cover
        // angle; the manager's snapshot is still directly poked to
        // Moving.
        device.close_cover().await.unwrap();
        // After close, our local optimistic write set motor_running =
        // Some(true). Our mock completes moves instantly inside `[SMOV]`,
        // so the motor is no longer running on the device side; the next
        // poll would correct the cache. For now we just verify the call
        // succeeded and exercise the open path too.
        device.open_cover().await.unwrap();
        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn writes_when_disconnected_return_not_connected() {
        let (device, _) = make_device();
        let err = device.open_cover().await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
        let err = device.close_cover().await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
        let err = device.calibrator_on(100).await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
        let err = device.calibrator_off().await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::NOT_CONNECTED);
    }

    #[tokio::test]
    async fn max_brightness_caps_at_hardware_limit() {
        let (device, _) = make_device();
        // Config default is MAX_BRIGHTNESS; the impl caps anyway.
        assert_eq!(
            device.max_brightness().await.unwrap(),
            MAX_BRIGHTNESS as u32
        );
    }

    #[tokio::test]
    async fn calibrator_on_rejects_brightness_above_configured_cap() {
        let (device, _) = make_device_with_cap(2048);
        device.set_connected(true).await.unwrap();
        // MaxBrightness must agree with what calibrator_on accepts.
        assert_eq!(device.max_brightness().await.unwrap(), 2048);

        // 2048 is at the cap — allowed.
        device.calibrator_on(2048).await.unwrap();

        // 2049 is above the configured cap but below the hardware ceiling
        // — must be rejected with INVALID_VALUE so the MaxBrightness
        // promise isn't violated.
        let err = device.calibrator_on(2049).await.unwrap_err();
        assert_eq!(err.code, ascom_alpaca::ASCOMErrorCode::INVALID_VALUE);

        device.set_connected(false).await.unwrap();
    }

    #[tokio::test]
    async fn metadata_fields_round_trip() {
        let (device, _) = make_device();
        assert_eq!(device.static_name(), "Deep Sky Dad FP2");
        assert_eq!(device.unique_id(), "dsd-fp2-001");
        let desc = device.description().await.unwrap();
        assert!(desc.contains("Deep Sky Dad"));
        let info = device.driver_info().await.unwrap();
        assert!(info.contains("CoverCalibrator"));
        let ver = device.driver_version().await.unwrap();
        assert!(!ver.is_empty());
    }

    // --- Config actions ---------------------------------------------------

    /// Build a device wired with a config-action context backed by a temp file
    /// pre-seeded with `effective`. Returns the reload handle (clone) so tests
    /// can assert the fire-after-response reload, and the `TempDir`/path.
    fn device_with_config_actions(
        effective: Config,
    ) -> (
        DsdFp2Device,
        ReloadSignal,
        tempfile::TempDir,
        std::path::PathBuf,
    ) {
        let factory = MockTransportFactory::default();
        let manager = FlatPanelManager::new(effective.clone(), Arc::new(factory));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dsd-fp2.json");
        std::fs::write(&path, serde_json::to_string(&effective).unwrap()).unwrap();
        let reload = ReloadSignal::new();
        let device = DsdFp2Device::new(effective.cover_calibrator.clone(), manager)
            .with_config_actions(ConfigActionCtx {
                effective,
                path: path.clone(),
                overrides: CliOverrides::default(),
                reload: reload.clone(),
            });
        (device, reload, dir, path)
    }

    #[tokio::test]
    async fn supported_actions_lists_config_actions_when_configured() {
        let (device, _reload, _dir, _path) = device_with_config_actions(test_config());
        let actions = device.supported_actions().await.unwrap();
        assert!(actions.contains(&"config.get".to_string()));
        assert!(actions.contains(&"config.apply".to_string()));
    }

    #[tokio::test]
    async fn supported_actions_empty_without_config_ctx() {
        let (device, _) = make_device();
        assert!(device.supported_actions().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn config_get_returns_effective_config_and_overrides() {
        let (device, _reload, _dir, _path) = device_with_config_actions(test_config());
        // Config actions must work while disconnected.
        assert!(!device.connected().await.unwrap());
        let body = device
            .action("config.get".to_string(), String::new())
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            value
                .pointer("/config/serial/port")
                .and_then(|v| v.as_str()),
            Some("/dev/mock")
        );
        assert!(value
            .get("overrides")
            .and_then(|v| v.as_array())
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn config_get_redacts_password_hash() {
        let mut effective = test_config();
        effective.server.auth = Some(rp_auth::config::AuthConfig {
            username: "obs".to_string(),
            password_hash: "$argon2id$v=19$real".to_string(),
        });
        let (device, _reload, _dir, _path) = device_with_config_actions(effective);
        let body = device
            .action("config.get".to_string(), String::new())
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            value
                .pointer("/config/server/auth/password_hash")
                .and_then(|v| v.as_str()),
            Some(crate::config_actions::REDACTED)
        );
    }

    #[tokio::test]
    async fn config_apply_persists_and_fires_reload() {
        let (device, reload, _dir, path) = device_with_config_actions(test_config());
        let mut changed = test_config();
        changed.cover_calibrator.max_brightness = 2048;
        let params = serde_json::to_string(&changed).unwrap();

        let body = device
            .action("config.apply".to_string(), params)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            value.get("status").and_then(|v| v.as_str()),
            Some("applying")
        );
        let reload_paths = value.get("reload").and_then(|v| v.as_array()).unwrap();
        assert!(reload_paths
            .iter()
            .any(|p| p.as_str() == Some("cover_calibrator.max_brightness")));

        // Persisted to disk with the new value.
        let persisted: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            persisted
                .pointer("/cover_calibrator/max_brightness")
                .and_then(|v| v.as_u64()),
            Some(2048)
        );

        // The reload is fired after the response — it must arrive.
        tokio::time::timeout(Duration::from_secs(2), reload.recv())
            .await
            .expect("config.apply should fire the reload");
    }

    #[tokio::test]
    async fn config_apply_without_change_returns_ok() {
        let (device, _reload, _dir, _path) = device_with_config_actions(test_config());
        let params = serde_json::to_string(&test_config()).unwrap();
        let body = device
            .action("config.apply".to_string(), params)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value.get("status").and_then(|v| v.as_str()), Some("ok"));
    }

    #[tokio::test]
    async fn config_apply_invalid_leaves_file_unchanged() {
        let (device, _reload, _dir, path) = device_with_config_actions(test_config());
        let before = std::fs::read_to_string(&path).unwrap();
        let mut bad = test_config();
        bad.serial.baud_rate = 0; // fails validation
        let params = serde_json::to_string(&bad).unwrap();

        let body = device
            .action("config.apply".to_string(), params)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            value.get("status").and_then(|v| v.as_str()),
            Some("invalid")
        );
        assert!(!value
            .get("errors")
            .and_then(|v| v.as_array())
            .unwrap()
            .is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[tokio::test]
    async fn config_apply_rejects_non_json() {
        let (device, _reload, _dir, _path) = device_with_config_actions(test_config());
        let err = device
            .action("config.apply".to_string(), "not json".to_string())
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
    }

    #[tokio::test]
    async fn unknown_action_returns_action_not_implemented() {
        let (device, _reload, _dir, _path) = device_with_config_actions(test_config());
        let err = device
            .action("config.frobnicate".to_string(), String::new())
            .await
            .unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::ACTION_NOT_IMPLEMENTED);
    }
}
