//! ppba-driver's [`ConfigurableDriver`] implementation plus the shared action
//! dispatch both devices delegate to.
//!
//! The driver registers **two** ASCOM devices (Switch + ObservingConditions)
//! backed by one config file. Both advertise `config.get` / `config.apply` /
//! `config.schema` and route through [`dispatch`], so an apply on either device
//! operates on the same full driver config and fires the same reload
//! (`ReloadSignal::notify` coalesces). See [`docs/services/ppba-driver.md`]
//! "Config Actions".
//!
//! [`docs/services/ppba-driver.md`]: ../../../docs/services/ppba-driver.md

use std::path::PathBuf;
use std::time::Duration;

use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use rusty_photon_config::actions::{
    self, ApplyError, ApplyStatus, ConfigAction, ConfigurableDriver, FieldError,
};
use rusty_photon_service_lifecycle::ReloadSignal;
use tracing::debug;

use crate::config::{CliOverrides, Config};
use crate::error::PpbaError;

/// Re-exported so devices and tests can name the redaction sentinel.
pub use rusty_photon_config::actions::REDACTED;

/// Delay before firing the reload so the `config.apply` HTTP response — served
/// by the very server the reload tears down — flushes before the blip.
const RELOAD_AFTER_RESPONSE_DELAY: Duration = Duration::from_millis(100);

/// Driver marker wiring the PPBA's full `Config` into the generic protocol.
pub struct PpbaDriver;

impl ConfigurableDriver for PpbaDriver {
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
        if config.serial.polling_interval.is_zero() {
            errors.push(FieldError {
                path: "serial.polling_interval".to_string(),
                msg: "must be greater than 0".to_string(),
            });
        }
        if config.serial.timeout.is_zero() {
            errors.push(FieldError {
                path: "serial.timeout".to_string(),
                msg: "must be greater than 0".to_string(),
            });
        }
        if config.observingconditions.averaging_period.is_zero() {
            errors.push(FieldError {
                path: "observingconditions.averaging_period".to_string(),
                msg: "must be greater than 0".to_string(),
            });
        }
        for (path, id) in [
            ("switch.unique_id", &config.switch.unique_id),
            (
                "observingconditions.unique_id",
                &config.observingconditions.unique_id,
            ),
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
        &["switch.unique_id", "observingconditions.unique_id"]
    }

    fn read_only_paths() -> &'static [&'static str] {
        &[
            "server.port",
            "switch.enabled",
            "observingconditions.enabled",
        ]
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

/// Dispatch a vendor action against the shared config context. Both the Switch
/// and ObservingConditions `Device::action` impls delegate here.
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
            let response = actions::config_get::<PpbaDriver>(&ctx.effective, &ctx.overrides)
                .map_err(PpbaError::from)?;
            Ok(serde_json::to_string(&response).map_err(PpbaError::from)?)
        }
        ConfigAction::Schema => {
            let response = actions::config_schema::<PpbaDriver>();
            Ok(serde_json::to_string(&response).map_err(PpbaError::from)?)
        }
        ConfigAction::Apply => {
            let response = match actions::config_apply::<PpbaDriver>(
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

            Ok(serde_json::to_string(&response).map_err(PpbaError::from)?)
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::config::{Config, ObservingConditionsConfig, SwitchConfig};

    fn valid_config() -> Config {
        Config {
            switch: SwitchConfig {
                unique_id: "switch-id".to_string(),
                ..SwitchConfig::default()
            },
            observingconditions: ObservingConditionsConfig {
                unique_id: "oc-id".to_string(),
                ..ObservingConditionsConfig::default()
            },
            ..Config::default()
        }
    }

    #[test]
    fn validate_accepts_populated_config() {
        assert!(PpbaDriver::validate(&valid_config()).is_empty());
    }

    #[test]
    fn validate_rejects_both_empty_unique_ids() {
        let paths: Vec<String> = PpbaDriver::validate(&Config::default())
            .into_iter()
            .map(|e| e.path)
            .collect();
        assert!(paths.contains(&"switch.unique_id".to_string()));
        assert!(paths.contains(&"observingconditions.unique_id".to_string()));
    }

    #[test]
    fn override_paths_cover_enable_flags() {
        let overrides = CliOverrides {
            enable_switch: Some(false),
            enable_observingconditions: Some(true),
            ..CliOverrides::default()
        };
        let paths = PpbaDriver::override_paths(&overrides);
        assert!(paths.contains(&"switch.enabled".to_string()));
        assert!(paths.contains(&"observingconditions.enabled".to_string()));
    }

    #[test]
    fn editability_tiers_cover_both_devices() {
        assert_eq!(
            PpbaDriver::locked_paths(),
            &["switch.unique_id", "observingconditions.unique_id"]
        );
        assert_eq!(
            PpbaDriver::read_only_paths(),
            &[
                "server.port",
                "switch.enabled",
                "observingconditions.enabled"
            ]
        );
    }

    #[test]
    fn supported_actions_gated_on_ctx() {
        assert!(supported_actions(&None).is_empty());
    }
}
