//! star-adventurer-gti's [`ConfigurableDriver`] implementation plus the action
//! dispatch the mount device delegates to (alongside its existing `ApParkAction`
//! vendor actions).
//!
//! The mount's parse-don't-validate config types (`FlipRangeHours`, `DecLimits`,
//! the `Usb|Udp` transport enum, the custom-serde `ApPark`, …) self-validate at
//! **deserialize** time, so a bad submission fails with `ApplyError::Parse`
//! before `validate` runs. `Overrides = ()`: the CLI transport/server-port
//! overrides all target fields the UI renders **read-only**, so there is nothing
//! to override-pin. See [`docs/services/star-adventurer-gti.md`] "Config Actions".
//!
//! [`docs/services/star-adventurer-gti.md`]: ../../../docs/services/star-adventurer-gti.md

use std::path::PathBuf;
use std::time::Duration;

use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use rusty_photon_config::actions::{
    self, ApplyError, ApplyStatus, ConfigAction, ConfigurableDriver, FieldError,
};
use rusty_photon_service_lifecycle::ReloadSignal;
use tracing::debug;

use crate::config::Config;
use crate::error::StarAdvError;

/// Re-exported so the device and tests can name the redaction sentinel.
pub use rusty_photon_config::actions::REDACTED;

/// Delay before firing the reload so the `config.apply` HTTP response — served
/// by the very server the reload tears down — flushes before the blip.
const RELOAD_AFTER_RESPONSE_DELAY: Duration = Duration::from_millis(100);

/// Driver marker wiring the mount's `Config` into the generic protocol.
pub struct StarAdvDriver;

impl ConfigurableDriver for StarAdvDriver {
    type Config = Config;
    /// CLI overrides (`--transport`/`--port`/`--baud`/`--server-port`) all target
    /// read-only fields, so there is nothing to override-pin.
    type Overrides = ();

    fn normalize(_config: &mut Config) {}

    fn validate(config: &Config) -> Vec<FieldError> {
        // The typed config self-validates at deserialize (parse-don't-validate
        // newtypes), so only the minted identity needs a domain check here.
        let mut errors = Vec::new();
        if config.mount.unique_id.trim().is_empty() {
            errors.push(FieldError {
                path: "mount.unique_id".to_string(),
                msg: "must not be empty (it is the device's stable ASCOM UniqueID)".to_string(),
            });
        }
        errors
    }

    fn secret_pointers() -> &'static [&'static str] {
        &["/server/auth/password_hash"]
    }

    fn override_paths(_overrides: &()) -> Vec<String> {
        Vec::new()
    }

    fn apply_overrides(_config: &mut Config, _overrides: &()) {}

    fn locked_paths() -> &'static [&'static str] {
        &["mount.unique_id"]
    }

    /// The `transport` block (a `Usb|Udp` tagged enum) is rendered read-only —
    /// changing the transport from the web form is an escape-hatch best done in
    /// the config file. `server.port` and `mount.enabled` are self-lockout fields.
    fn read_only_paths() -> &'static [&'static str] {
        &[
            "transport.kind",
            "transport.port",
            "transport.address",
            "transport.baud_rate",
            "transport.command_timeout",
            "transport.polling_interval",
            "server.port",
            "mount.enabled",
        ]
    }
}

/// State the config actions need.
#[derive(Clone)]
pub struct ConfigActionCtx {
    /// Effective config this server instance is running.
    pub effective: Config,
    /// Where `config.apply` persists; what reload re-reads.
    pub path: PathBuf,
    /// Fired (after the response flushes) to trigger the in-process reload.
    pub reload: ReloadSignal,
}

/// The config-action names a ctx-bearing device should append to its
/// `SupportedActions` (alongside the mount's own `ApParkAction` names).
pub fn config_action_names(ctx: &Option<ConfigActionCtx>) -> Vec<String> {
    if ctx.is_some() {
        ConfigAction::ALL
            .iter()
            .map(|action| action.name().to_string())
            .collect()
    } else {
        Vec::new()
    }
}

/// Dispatch a config vendor action against the context. Returns
/// `ACTION_NOT_IMPLEMENTED` for any non-config action name, so the device's
/// `action()` can fall through to this after trying its `ApParkAction` set.
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
            let response = actions::config_get::<StarAdvDriver>(&ctx.effective, &())
                .map_err(StarAdvError::from)?;
            Ok(serde_json::to_string(&response).map_err(StarAdvError::from)?)
        }
        ConfigAction::Schema => {
            let response = actions::config_schema::<StarAdvDriver>();
            Ok(serde_json::to_string(&response).map_err(StarAdvError::from)?)
        }
        ConfigAction::Apply => {
            let response = match actions::config_apply::<StarAdvDriver>(
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

            Ok(serde_json::to_string(&response).map_err(StarAdvError::from)?)
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_empty_unique_id() {
        let config = Config::default(); // unique_id empty by default
        assert!(StarAdvDriver::validate(&config)
            .iter()
            .any(|e| e.path == "mount.unique_id"));
    }

    #[test]
    fn validate_accepts_populated_unique_id() {
        let mut config = Config::default();
        config.mount.unique_id = "star-adv-id".to_string();
        assert!(StarAdvDriver::validate(&config).is_empty());
    }

    #[test]
    fn editability_tiers() {
        assert_eq!(StarAdvDriver::locked_paths(), &["mount.unique_id"]);
        assert!(StarAdvDriver::read_only_paths().contains(&"transport.kind"));
        assert!(StarAdvDriver::read_only_paths().contains(&"server.port"));
        assert!(StarAdvDriver::read_only_paths().contains(&"mount.enabled"));
    }

    #[test]
    fn config_action_names_gated_on_ctx() {
        assert!(config_action_names(&None).is_empty());
    }
}
