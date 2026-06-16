//! Configuration for the zwo-camera service.
//!
//! Phase C (Track A) only *reads* [`ServerConfig::port`] to bind the listener.
//! The `devices` override map and `filterwheel.enabled` toggle are carried so
//! the on-disk shape matches `docs/services/zwo-camera.md`; they are consumed by
//! the device-trait work (Phase E) and config-actions (Phase G).

use std::collections::BTreeMap;
use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ZwoCameraError;

/// The default Alpaca listening port. Next free in the 1112x family; 11121 is
/// `qhy-camera`.
pub const DEFAULT_PORT: u16 = 11122;

/// Effective service configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Config {
    /// Optional per-device overrides keyed by SDK serial (Phase E+).
    pub devices: BTreeMap<String, DeviceOverride>,
    /// Filter-wheel registration toggle (Phase F).
    pub filterwheel: FilterWheelConfig,
    /// HTTP server settings.
    pub server: ServerConfig,
}

/// Friendly overrides for a specific device, keyed by its SDK serial.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct DeviceOverride {
    /// Display name override.
    pub name: Option<String>,
    /// Description override.
    pub description: Option<String>,
    /// Per-slot filter names (EFW only).
    pub filter_names: Option<Vec<String>>,
}

/// Whether discovered EFW filter wheels are registered as ASCOM devices.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct FilterWheelConfig {
    /// Register discovered EFWs as FilterWheel devices when `true`.
    pub enabled: bool,
}

/// HTTP server settings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ServerConfig {
    /// The listening port (one port hosts every enumerated device).
    #[serde(default = "default_port")]
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self { port: DEFAULT_PORT }
    }
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

/// CLI overrides layered on top of the file configuration.
#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    /// `--port`: overrides `server.port`.
    pub port: Option<u16>,
}

impl CliOverrides {
    /// Dotted config paths currently pinned by a CLI override.
    #[must_use]
    pub fn pinned_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        if self.port.is_some() {
            paths.push("server.port".to_owned());
        }
        paths
    }

    /// Apply the overrides onto `config` in place.
    pub fn apply(&self, config: &mut Config) {
        if let Some(port) = self.port {
            config.server.port = port;
        }
    }
}

/// Load the on-disk config (or defaults when the file is absent) and layer CLI
/// overrides on top.
///
/// # Errors
/// Returns [`ZwoCameraError::Config`] when the file exists but cannot be read or
/// parsed.
pub fn load_effective_config(
    path: &Path,
    overrides: &CliOverrides,
) -> Result<Config, ZwoCameraError> {
    let mut config = match std::fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents)
            .map_err(|e| ZwoCameraError::Config(format!("parse {}: {e}", path.display())))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(e) => {
            return Err(ZwoCameraError::Config(format!(
                "read {}: {e}",
                path.display()
            )))
        }
    };
    overrides.apply(&mut config);
    Ok(config)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn default_config_uses_the_reserved_port() {
        let config = Config::default();
        assert_eq!(config.server.port, 11122);
        assert!(!config.filterwheel.enabled);
        assert!(config.devices.is_empty());
    }

    #[test]
    fn a_server_object_without_a_port_keeps_the_default() {
        let config: Config = serde_json::from_str(r#"{"server": {}}"#).unwrap();
        assert_eq!(config.server.port, 11122);
    }

    #[test]
    fn cli_port_override_wins_and_is_pinned() {
        let mut config = Config::default();
        let overrides = CliOverrides { port: Some(12345) };
        overrides.apply(&mut config);
        assert_eq!(config.server.port, 12345);
        assert_eq!(overrides.pinned_paths(), vec!["server.port".to_owned()]);
    }

    #[test]
    fn no_override_pins_nothing() {
        assert!(CliOverrides::default().pinned_paths().is_empty());
    }
}
