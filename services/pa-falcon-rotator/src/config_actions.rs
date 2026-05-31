//! pa-falcon-rotator's [`ConfigurableDriver`] implementation plus the shared
//! action dispatch both devices delegate to.
//!
//! The driver registers **two** ASCOM devices (rotator + status switch) backed
//! by one config file. Both advertise `config.get` / `config.apply` /
//! `config.schema` and route through [`dispatch`], so an apply on either device
//! operates on the same full driver config and fires the same reload signal.
//! `ReloadSignal::notify` coalesces, so even a (spurious) apply on both devices
//! collapses to a single reload. See [`docs/services/falcon-rotator.md`]
//! "Config Actions".
//!
//! [`docs/services/falcon-rotator.md`]: ../../../docs/services/falcon-rotator.md

use std::path::PathBuf;
use std::time::Duration;

use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use rusty_photon_config::actions::{
    self, ApplyError, ApplyStatus, ConfigAction, ConfigurableDriver, FieldError,
};
use rusty_photon_service_lifecycle::ReloadSignal;
use tracing::debug;

use crate::config::{CliOverrides, Config};
use crate::error::FalconRotatorError;

/// Re-exported so devices and tests can name the redaction sentinel.
pub use rusty_photon_config::actions::REDACTED;

/// Delay before firing the reload so the `config.apply` HTTP response — served
/// by the very server the reload tears down — flushes before the blip.
const RELOAD_AFTER_RESPONSE_DELAY: Duration = Duration::from_millis(100);

/// Driver marker wiring the Falcon's full `Config` into the generic protocol.
pub struct FalconRotatorDriver;

impl ConfigurableDriver for FalconRotatorDriver {
    type Config = Config;
    type Overrides = CliOverrides;

    fn normalize(config: &mut Config) {
        let trimmed = config.serial.port.trim();
        if trimmed.len() != config.serial.port.len() {
            config.serial.port = trimmed.to_string();
        }
    }

    fn validate(config: &Config) -> Vec<FieldError> {
        let mut errors = Vec::new();
        if config.serial.port.trim().is_empty() {
            errors.push(FieldError {
                path: "serial.port".to_string(),
                msg: "must not be empty".to_string(),
            });
        }
        if config.serial.baud_rate == 0 {
            errors.push(FieldError {
                path: "serial.baud_rate".to_string(),
                msg: "must be greater than 0".to_string(),
            });
        }
        if config.serial.timeout.is_zero() {
            errors.push(FieldError {
                path: "serial.timeout".to_string(),
                msg: "must be greater than 0".to_string(),
            });
        }
        for (path, id) in [
            ("rotator.unique_id", &config.rotator.unique_id),
            ("switch.unique_id", &config.switch.unique_id),
        ] {
            if id.trim().is_empty() {
                errors.push(FieldError {
                    path: path.to_string(),
                    msg: "must not be empty (it is the device's stable ASCOM UniqueID)".to_string(),
                });
            }
        }
        errors
    }

    fn secret_pointers() -> &'static [&'static str] {
        &["/server/auth/password_hash"]
    }

    fn override_paths(overrides: &CliOverrides) -> Vec<String> {
        overrides.pinned_paths()
    }

    fn apply_overrides(config: &mut Config, overrides: &CliOverrides) {
        overrides.apply(config);
    }

    fn locked_paths() -> &'static [&'static str] {
        &["rotator.unique_id", "switch.unique_id"]
    }

    fn read_only_paths() -> &'static [&'static str] {
        &["server.port", "rotator.enabled", "switch.enabled"]
    }
}

/// State the config actions need, shared (cloned) by both devices.
#[derive(Clone)]
pub struct ConfigActionCtx {
    /// Effective config (file + CLI overrides) this server was built with.
    pub effective: Config,
    /// Where `config.apply` persists; what reload re-reads.
    pub path: PathBuf,
    /// CLI overrides, so config actions distinguish file vs. override layers.
    pub overrides: CliOverrides,
    /// Fired (after the response flushes) to trigger the in-process reload.
    pub reload: ReloadSignal,
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

/// Dispatch a vendor action against the shared config context. Both the rotator
/// and switch `Device::action` impls delegate here.
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
            let response =
                actions::config_get::<FalconRotatorDriver>(&ctx.effective, &ctx.overrides)
                    .map_err(FalconRotatorError::from)?;
            Ok(serde_json::to_string(&response).map_err(FalconRotatorError::from)?)
        }
        ConfigAction::Schema => {
            let response = actions::config_schema::<FalconRotatorDriver>();
            Ok(serde_json::to_string(&response).map_err(FalconRotatorError::from)?)
        }
        ConfigAction::Apply => {
            let response = match actions::config_apply::<FalconRotatorDriver>(
                &ctx.path,
                &ctx.overrides,
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
                Err(ApplyError::Serialize(e)) => {
                    return Err(ASCOMError::new(
                        ASCOMErrorCode::INVALID_OPERATION,
                        format!("config.apply: {e}"),
                    ))
                }
            };

            if matches!(response.status, ApplyStatus::Applying) {
                let reload = ctx.reload.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(RELOAD_AFTER_RESPONSE_DELAY).await;
                    debug!("firing in-process reload after config.apply");
                    reload.notify();
                });
            }

            Ok(serde_json::to_string(&response).map_err(FalconRotatorError::from)?)
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::config::{Config, RotatorConfig, SerialConfig, SwitchConfig};
    use std::time::Duration;

    fn valid_config() -> Config {
        Config {
            rotator: RotatorConfig {
                unique_id: "rotator-id".to_string(),
                ..RotatorConfig::default()
            },
            switch: SwitchConfig {
                unique_id: "switch-id".to_string(),
                ..SwitchConfig::default()
            },
            ..Config::default()
        }
    }

    #[test]
    fn validate_accepts_populated_config() {
        assert!(FalconRotatorDriver::validate(&valid_config()).is_empty());
    }

    #[test]
    fn validate_rejects_both_empty_unique_ids() {
        let errors = FalconRotatorDriver::validate(&Config::default());
        let paths: Vec<String> = errors.into_iter().map(|e| e.path).collect();
        assert!(paths.contains(&"rotator.unique_id".to_string()));
        assert!(paths.contains(&"switch.unique_id".to_string()));
    }

    #[test]
    fn validate_flags_bad_serial_fields() {
        let config = Config {
            serial: SerialConfig {
                port: "  ".to_string(),
                baud_rate: 0,
                timeout: Duration::ZERO,
            },
            ..valid_config()
        };
        let paths: Vec<String> = FalconRotatorDriver::validate(&config)
            .into_iter()
            .map(|e| e.path)
            .collect();
        for expected in ["serial.port", "serial.baud_rate", "serial.timeout"] {
            assert!(paths.contains(&expected.to_string()), "missing {expected}");
        }
    }

    #[test]
    fn editability_tiers_cover_both_devices() {
        assert_eq!(
            FalconRotatorDriver::locked_paths(),
            &["rotator.unique_id", "switch.unique_id"]
        );
        assert_eq!(
            FalconRotatorDriver::read_only_paths(),
            &["server.port", "rotator.enabled", "switch.enabled"]
        );
    }

    #[test]
    fn supported_actions_gated_on_ctx() {
        assert!(supported_actions(&None).is_empty());
    }
}
