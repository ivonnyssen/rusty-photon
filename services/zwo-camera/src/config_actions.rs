//! `ConfigurableDriver` impl for zwo-camera — wires the cross-driver
//! `config.get` / `config.apply` / `config.schema` protocol
//! (`docs/services/config-actions.md`) to this service's [`Config`].
//!
//! Editability tiers:
//! - **Locked (identity):** none. ASCOM `UniqueID`s are derived from the
//!   camera/EFW SDK serial (see `docs/services/zwo-camera.md` "Device identity"),
//!   not minted into config, so there is no identity field to lock.
//! - **Hard read-only:** `server.port` (a BFF could not follow the rebind) and
//!   `filterwheel.enabled` (toggling adds/removes endpoints → restart-required).
//! - **Editable:** the per-serial `devices` map (`name` / `description` /
//!   `filter_names`).

use rusty_photon_config::actions::{ConfigurableDriver, FieldError};

use crate::config::{CliOverrides, Config};

/// Zero-sized marker implementing [`ConfigurableDriver`] for the zwo-camera
/// [`Config`]. The generic `rusty_photon_driver::dispatch` routes the three
/// config actions against this.
pub struct ZwoCameraDriver;

impl ConfigurableDriver for ZwoCameraDriver {
    type Config = Config;
    type Overrides = CliOverrides;

    fn normalize(_config: &mut Config) {}

    fn validate(config: &Config) -> Vec<FieldError> {
        let mut errors = Vec::new();
        for (serial, device) in &config.devices {
            if let Some(names) = &device.filter_names {
                for (index, name) in names.iter().enumerate() {
                    if name.trim().is_empty() {
                        errors.push(FieldError {
                            path: format!("devices.{serial}.filter_names.{index}"),
                            msg: "filter name must not be empty".to_string(),
                        });
                    }
                }
            }
        }
        errors
    }

    /// No secrets in v0 (TLS / auth are Future Work).
    fn secret_pointers() -> &'static [&'static str] {
        &[]
    }

    fn override_paths(overrides: &CliOverrides) -> Vec<String> {
        overrides.pinned_paths()
    }

    fn apply_overrides(config: &mut Config, overrides: &CliOverrides) {
        overrides.apply(config);
    }

    // `locked_paths()` intentionally not overridden (defaults to `&[]`): the
    // hardware-derived UniqueID means there is no locked identity field.

    fn read_only_paths() -> &'static [&'static str] {
        &["server.port", "filterwheel.enabled"]
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::config::DeviceOverride;

    #[test]
    fn empty_filter_name_is_rejected() {
        let mut config = Config::default();
        config.devices.insert(
            "EFW-1".to_string(),
            DeviceOverride {
                filter_names: Some(vec!["L".to_string(), "  ".to_string()]),
                ..Default::default()
            },
        );
        let errors = ZwoCameraDriver::validate(&config);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].path, "devices.EFW-1.filter_names.1");
    }

    #[test]
    fn valid_config_has_no_errors() {
        let mut config = Config::default();
        config.devices.insert(
            "ASI2600MM-0".to_string(),
            DeviceOverride {
                name: Some("Main".to_string()),
                description: Some("desc".to_string()),
                filter_names: None,
            },
        );
        assert!(ZwoCameraDriver::validate(&config).is_empty());
    }

    #[test]
    fn no_locked_identity_fields() {
        assert!(ZwoCameraDriver::locked_paths().is_empty());
    }

    #[test]
    fn port_and_filterwheel_toggle_are_read_only() {
        let read_only = ZwoCameraDriver::read_only_paths();
        assert!(read_only.contains(&"server.port"));
        assert!(read_only.contains(&"filterwheel.enabled"));
    }

    #[test]
    fn port_override_is_pinned_and_applied() {
        let overrides = CliOverrides { port: Some(12321) };
        assert_eq!(
            ZwoCameraDriver::override_paths(&overrides),
            vec!["server.port".to_string()]
        );
        let mut config = Config::default();
        ZwoCameraDriver::apply_overrides(&mut config, &overrides);
        assert_eq!(config.server.port, 12321);
    }
}
