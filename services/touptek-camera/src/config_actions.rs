//! `ConfigurableDriver` impl for touptek-camera — wires the cross-driver
//! `config.get` / `config.apply` / `config.schema` protocol
//! (`docs/services/config-actions.md`) to this service's [`Config`].
//!
//! Editability tiers:
//! - **Locked (identity):** none. ASCOM `UniqueID`s are derived from the SDK
//!   device id (see `docs/services/touptek-camera.md` "Device identity"), not
//!   minted into config, so there is no identity field to lock.
//! - **Hard read-only:** `server.port` (a BFF could not follow the rebind).
//! - **Editable:** the per-id `devices` map (`name` / `description`).

use rusty_photon_config::actions::{ConfigurableDriver, FieldError};

use crate::config::{CliOverrides, Config};

/// Zero-sized marker implementing [`ConfigurableDriver`] for the touptek-camera
/// [`Config`]. The generic `rusty_photon_driver::dispatch` routes the three
/// config actions against this.
pub struct TouptekCameraDriver;

impl ConfigurableDriver for TouptekCameraDriver {
    type Config = Config;
    type Overrides = CliOverrides;

    fn normalize(_config: &mut Config) {}

    fn validate(config: &Config) -> Vec<FieldError> {
        let mut errors = Vec::new();
        for (id, device) in &config.devices {
            if let Some(name) = &device.name {
                if name.trim().is_empty() {
                    errors.push(FieldError {
                        path: format!("devices.{id}.name"),
                        msg: "device name must not be empty".to_string(),
                    });
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
        &["server.port"]
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::config::DeviceOverride;

    #[test]
    fn empty_device_name_is_rejected() {
        let mut config = Config::default();
        config.devices.insert(
            "sim-0".to_string(),
            DeviceOverride {
                name: Some("  ".to_string()),
                description: None,
            },
        );
        let errors = TouptekCameraDriver::validate(&config);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].path, "devices.sim-0.name");
    }

    #[test]
    fn valid_config_has_no_errors() {
        let mut config = Config::default();
        config.devices.insert(
            "sim-0".to_string(),
            DeviceOverride {
                name: Some("Main Imaging".to_string()),
                description: Some("desc".to_string()),
            },
        );
        assert!(TouptekCameraDriver::validate(&config).is_empty());
    }

    #[test]
    fn no_locked_identity_fields() {
        assert!(TouptekCameraDriver::locked_paths().is_empty());
    }

    #[test]
    fn port_is_read_only() {
        assert!(TouptekCameraDriver::read_only_paths().contains(&"server.port"));
    }

    #[test]
    fn port_override_is_pinned_and_applied() {
        let overrides = CliOverrides { port: Some(12321) };
        assert_eq!(
            TouptekCameraDriver::override_paths(&overrides),
            vec!["server.port".to_string()]
        );
        let mut config = Config::default();
        TouptekCameraDriver::apply_overrides(&mut config, &overrides);
        assert_eq!(config.server.port, 12321);
    }
}
