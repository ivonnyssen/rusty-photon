//! sky-survey-camera's [`ConfigurableDriver`] implementation plus the action
//! dispatch the camera device delegates to.
//!
//! Single ASCOM device (the camera); follow-mode telescope/rotator blocks are
//! *client* config, so their plaintext credentials are treated as secrets
//! (redacted on read, carried forward on apply). The driver has no CLI
//! overrides, so `Overrides = ()`. See [`docs/services/sky-survey-camera.md`]
//! "Config Actions".
//!
//! [`docs/services/sky-survey-camera.md`]: ../../../docs/services/sky-survey-camera.md

use std::path::PathBuf;
use std::time::Duration;

use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use rusty_photon_config::actions::{
    self, ApplyError, ApplyStatus, ConfigAction, ConfigurableDriver, FieldError,
};
use rusty_photon_service_lifecycle::ReloadSignal;
use serde_json::Error as JsonError;
use tracing::debug;

use crate::config::Config;

/// Re-exported so the camera and tests can name the redaction sentinel.
pub use rusty_photon_config::actions::REDACTED;

/// Delay before firing the reload so the `config.apply` HTTP response — served
/// by the very server the reload tears down — flushes before the blip.
const RELOAD_AFTER_RESPONSE_DELAY: Duration = Duration::from_millis(100);

/// Driver marker wiring the camera's `Config` into the generic protocol.
pub struct SkySurveyCameraDriver;

impl ConfigurableDriver for SkySurveyCameraDriver {
    type Config = Config;
    /// No CLI overrides — the binary only takes `--config` / `--log-level`.
    type Overrides = ();

    fn normalize(_config: &mut Config) {}

    fn validate(config: &Config) -> Vec<FieldError> {
        let mut errors = Vec::new();
        if config.device.unique_id.trim().is_empty() {
            errors.push(FieldError {
                path: "device.unique_id".to_string(),
                msg: "must not be empty (it is the device's stable ASCOM UniqueID)".to_string(),
            });
        }
        if let Some(t) = &config.pointing.telescope {
            if !t.offset_ra_arcsec.is_finite() {
                errors.push(FieldError {
                    path: "pointing.telescope.offset_ra_arcsec".to_string(),
                    msg: "must be finite".to_string(),
                });
            }
            if !t.offset_dec_arcsec.is_finite() {
                errors.push(FieldError {
                    path: "pointing.telescope.offset_dec_arcsec".to_string(),
                    msg: "must be finite".to_string(),
                });
            }
            if t.request_timeout.is_zero() {
                errors.push(FieldError {
                    path: "pointing.telescope.request_timeout".to_string(),
                    msg: "must be greater than 0".to_string(),
                });
            }
        }
        // The rotator only feeds rotation inside follow mode; reject the orphan
        // config rather than silently ignore it (mirrors `config::validate`).
        if config.pointing.rotator.is_some() && config.pointing.telescope.is_none() {
            errors.push(FieldError {
                path: "pointing.rotator".to_string(),
                msg: "requires pointing.telescope (rotator-driven rotation only applies in follow mode)"
                    .to_string(),
            });
        }
        if let Some(r) = &config.pointing.rotator {
            if r.request_timeout.is_zero() {
                errors.push(FieldError {
                    path: "pointing.rotator.request_timeout".to_string(),
                    msg: "must be greater than 0".to_string(),
                });
            }
        }
        errors
    }

    /// Follow-mode client credentials are plaintext passwords; redact them on
    /// read and carry them forward on apply so a round-tripped form never blanks
    /// them. Absent (`None`) blocks are simply skipped by the redaction.
    fn secret_pointers() -> &'static [&'static str] {
        &[
            "/pointing/telescope/auth/password",
            "/pointing/rotator/auth/password",
        ]
    }

    fn override_paths(_overrides: &()) -> Vec<String> {
        Vec::new()
    }

    fn apply_overrides(_config: &mut Config, _overrides: &()) {}

    fn locked_paths() -> &'static [&'static str] {
        &["device.unique_id"]
    }

    fn read_only_paths() -> &'static [&'static str] {
        &["server.port"]
    }
}

/// State the config actions need.
#[derive(Clone, Debug)]
pub struct ConfigActionCtx {
    /// Effective config this server instance is running.
    pub effective: Config,
    /// Where `config.apply` persists; what reload re-reads.
    pub path: PathBuf,
    /// Fired (after the response flushes) to trigger the in-process reload.
    pub reload: ReloadSignal,
}

fn ser_err(e: JsonError) -> ASCOMError {
    ASCOMError::new(
        ASCOMErrorCode::INVALID_OPERATION,
        format!("config: serialization error: {e}"),
    )
}

/// The supported-actions list for a device that may or may not carry a config
/// context (ctx-less focused-test devices advertise none).
pub fn supported_actions(ctx: &Option<ConfigActionCtx>) -> Vec<String> {
    if ctx.is_some() {
        ConfigAction::ALL
            .iter()
            .map(|action| action.name().to_string())
            .collect()
    } else {
        Vec::new()
    }
}

/// Dispatch a vendor action against the config context. The camera's
/// `Device::action` impl delegates here.
pub async fn dispatch(
    ctx: &Option<ConfigActionCtx>,
    action: String,
    parameters: String,
) -> ASCOMResult<String> {
    let Some(parsed) = ConfigAction::from_name(&action) else {
        return Err(ASCOMError::new(
            ASCOMErrorCode::ACTION_NOT_IMPLEMENTED,
            format!("unknown action {action:?}"),
        ));
    };
    let ctx = ctx.as_ref().ok_or_else(|| {
        ASCOMError::new(
            ASCOMErrorCode::ACTION_NOT_IMPLEMENTED,
            "config actions are not configured for this device instance",
        )
    })?;

    match parsed {
        ConfigAction::Get => {
            let response = actions::config_get::<SkySurveyCameraDriver>(&ctx.effective, &())
                .map_err(ser_err)?;
            Ok(serde_json::to_string(&response).map_err(ser_err)?)
        }
        ConfigAction::Schema => {
            let response = actions::config_schema::<SkySurveyCameraDriver>();
            Ok(serde_json::to_string(&response).map_err(ser_err)?)
        }
        ConfigAction::Apply => {
            let response = match actions::config_apply::<SkySurveyCameraDriver>(
                &ctx.path,
                &(),
                &ctx.effective,
                &parameters,
            ) {
                Ok(response) => response,
                Err(ApplyError::Parse(e)) => {
                    return Err(ASCOMError::new(
                        ASCOMErrorCode::INVALID_VALUE,
                        format!("config.apply: invalid config JSON: {e}"),
                    ))
                }
                Err(ApplyError::ReadFile(e)) => {
                    return Err(ASCOMError::new(
                        ASCOMErrorCode::INVALID_OPERATION,
                        format!("config.apply: {e}"),
                    ))
                }
                Err(ApplyError::Persist(e)) => {
                    return Err(ASCOMError::new(
                        ASCOMErrorCode::INVALID_OPERATION,
                        format!("config.apply: failed to persist config: {e}"),
                    ))
                }
                Err(ApplyError::Serialize(e)) => return Err(ser_err(e)),
            };

            if matches!(response.status, ApplyStatus::Applying) {
                let reload = ctx.reload.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(RELOAD_AFTER_RESPONSE_DELAY).await;
                    debug!("firing in-process reload after config.apply");
                    reload.notify();
                });
            }

            Ok(serde_json::to_string(&response).map_err(ser_err)?)
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::config::{
        DeviceConfig, OpticsConfig, PointingConfig, RotatorFollowConfig, ServerConfig,
        SurveyConfig, TelescopeFollowConfig,
    };
    use std::path::PathBuf as P;

    fn base() -> Config {
        Config {
            device: DeviceConfig {
                name: "cam".into(),
                unique_id: "id".into(),
                description: "d".into(),
            },
            optics: OpticsConfig {
                focal_length_mm: 1000.0,
                pixel_size_x_um: 3.76,
                pixel_size_y_um: 3.76,
                sensor_width_px: 100,
                sensor_height_px: 100,
            },
            pointing: PointingConfig {
                initial_ra_deg: 0.0,
                initial_dec_deg: 0.0,
                initial_rotation_deg: 0.0,
                telescope: None,
                rotator: None,
            },
            survey: SurveyConfig {
                name: "DSS2 Red".into(),
                request_timeout: Duration::from_secs(30),
                cache_dir: P::from("/tmp"),
                endpoint: "http://x/".into(),
            },
            server: ServerConfig { port: 0 },
        }
    }

    fn telescope() -> TelescopeFollowConfig {
        TelescopeFollowConfig {
            alpaca_url: "http://x/".into(),
            device_number: 0,
            offset_ra_arcsec: 0.0,
            offset_dec_arcsec: 0.0,
            request_timeout: Duration::from_secs(2),
            auth: None,
        }
    }

    #[test]
    fn validate_accepts_static_mode() {
        assert!(SkySurveyCameraDriver::validate(&base()).is_empty());
    }

    #[test]
    fn validate_rejects_empty_unique_id() {
        let mut c = base();
        c.device.unique_id = String::new();
        assert!(SkySurveyCameraDriver::validate(&c)
            .iter()
            .any(|e| e.path == "device.unique_id"));
    }

    #[test]
    fn validate_rejects_nan_offset_and_orphan_rotator() {
        let mut c = base();
        let mut t = telescope();
        t.offset_ra_arcsec = f64::NAN;
        c.pointing.telescope = Some(t);
        assert!(SkySurveyCameraDriver::validate(&c)
            .iter()
            .any(|e| e.path == "pointing.telescope.offset_ra_arcsec"));

        let mut c2 = base();
        c2.pointing.rotator = Some(RotatorFollowConfig {
            alpaca_url: "http://x/".into(),
            device_number: 0,
            request_timeout: Duration::from_secs(2),
            auth: None,
        });
        assert!(SkySurveyCameraDriver::validate(&c2)
            .iter()
            .any(|e| e.path == "pointing.rotator"));
    }

    #[test]
    fn editability_tiers_and_secrets() {
        assert_eq!(SkySurveyCameraDriver::locked_paths(), &["device.unique_id"]);
        assert_eq!(SkySurveyCameraDriver::read_only_paths(), &["server.port"]);
        assert_eq!(
            SkySurveyCameraDriver::secret_pointers(),
            &[
                "/pointing/telescope/auth/password",
                "/pointing/rotator/auth/password"
            ]
        );
    }

    #[test]
    fn supported_actions_gated_on_ctx() {
        assert!(supported_actions(&None).is_empty());
    }
}
