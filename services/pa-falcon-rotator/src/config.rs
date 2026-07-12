//! Configuration types for the pa-falcon-rotator driver

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// Main configuration structure
///
/// `deny_unknown_fields` so typoed or removed keys fail loudly at load
/// instead of being silently ignored.
#[derive(Debug, Clone, Serialize, Deserialize, Default, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub serial: SerialConfig,
    pub server: ServerConfig,
    pub rotator: RotatorConfig,
    pub switch: SwitchConfig,
}

/// Serial port configuration
///
/// `deny_unknown_fields` so typoed or removed keys fail loudly at load
/// instead of being silently ignored.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SerialConfig {
    pub port: String,
    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,
    // `humantime_serde` stores the duration as a string; schemars describes it
    // as a string so the schema matches the wire form.
    #[serde(default = "default_timeout", with = "humantime_serde")]
    #[schemars(with = "String")]
    pub timeout: Duration,
}

/// Server configuration
///
/// `deny_unknown_fields` so typoed or removed keys fail loudly at load
/// instead of being silently ignored.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub port: u16,
    /// Alpaca UDP discovery responder port (normally 32227). Absent/`null` —
    /// the default — disables discovery: many rusty-photon servers on one
    /// host would collide on the shared discovery port, so it is a per-host
    /// opt-in for single-driver deployments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery_port: Option<u16>,
    #[serde(default)]
    pub tls: Option<rp_tls::config::TlsConfig>,
    #[serde(default)]
    pub auth: Option<rp_auth::config::AuthConfig>,
}

/// Rotator device configuration
///
/// `deny_unknown_fields` so typoed or removed keys fail loudly at load
/// instead of being silently ignored.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RotatorConfig {
    pub name: String,
    #[serde(default)]
    pub unique_id: String,
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Status Switch device configuration
///
/// `deny_unknown_fields` so typoed or removed keys fail loudly at load
/// instead of being silently ignored.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SwitchConfig {
    pub name: String,
    #[serde(default)]
    pub unique_id: String,
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_baud_rate() -> u32 {
    9600
}

fn default_timeout() -> Duration {
    Duration::from_secs(2)
}

fn default_true() -> bool {
    true
}

/// Platform-dependent default serial port. Both values are placeholders the
/// operator replaces with the real device path: the driver restart-loops
/// until then, on Windows (`COM3`) exactly as on Unix (`/dev/ttyUSB0`).
#[cfg(windows)]
const DEFAULT_SERIAL_PORT: &str = "COM3";
#[cfg(not(windows))]
const DEFAULT_SERIAL_PORT: &str = "/dev/ttyUSB0";

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_SERIAL_PORT.to_string(),
            baud_rate: default_baud_rate(),
            timeout: default_timeout(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 11118,
            discovery_port: None,
            tls: None,
            auth: None,
        }
    }
}

impl Default for RotatorConfig {
    fn default() -> Self {
        Self {
            name: "Pegasus Falcon Rotator".to_string(),
            unique_id: String::new(),
            description: "Pegasus Astro Falcon Rotator (firmware >= 1.3)".to_string(),
            enabled: true,
        }
    }
}

impl Default for SwitchConfig {
    fn default() -> Self {
        Self {
            name: "Pegasus Falcon Status".to_string(),
            unique_id: String::new(),
            description: "Pegasus Falcon Rotator status sensors (voltage + limit-hit)".to_string(),
            enabled: true,
        }
    }
}

/// Load configuration from a JSON file
pub fn load_config(
    path: &Path,
) -> std::result::Result<Config, Box<dyn std::error::Error + Send + Sync>> {
    let content = std::fs::read_to_string(path)?;
    let config: Config = serde_json::from_str(&content)?;
    Ok(config)
}

/// CLI overrides layered over the file config. Tracks which fields are pinned by
/// a command-line flag so the config actions can distinguish file vs. override
/// layers (see `docs/services/falcon-rotator.md` "Config Actions").
#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    /// `--port` → `serial.port`.
    pub serial_port: Option<String>,
    /// `--server-port` → `server.port`.
    pub server_port: Option<u16>,
}

impl CliOverrides {
    /// Dotted JSON paths currently pinned by an active override.
    pub fn pinned_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        if self.serial_port.is_some() {
            paths.push("serial.port".to_string());
        }
        if self.server_port.is_some() {
            paths.push("server.port".to_string());
        }
        paths
    }

    /// Apply the overrides onto `config` in place.
    pub fn apply(&self, config: &mut Config) {
        if let Some(port) = &self.serial_port {
            config.serial.port = port.clone();
        }
        if let Some(port) = self.server_port {
            config.server.port = port;
        }
    }
}

/// Load the effective config: the file at `path` if it exists, else
/// `Config::default()`, with CLI `overrides` applied on top. A present-but-
/// corrupt file is surfaced (naming the path) rather than silently reset.
pub fn load_effective_config(
    path: &Path,
    overrides: &CliOverrides,
) -> std::result::Result<Config, Box<dyn std::error::Error + Send + Sync>> {
    let mut config = match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content)
            .map_err(|e| format!("config file {} is not valid JSON: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(e) => return Err(format!("could not read config file {}: {e}", path.display()).into()),
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
    fn default_config_has_expected_values() {
        let config = Config::default();

        assert_eq!(config.rotator.name, "Pegasus Falcon Rotator");
        assert!(config.rotator.unique_id.is_empty());
        assert!(config.rotator.enabled);

        assert_eq!(config.switch.name, "Pegasus Falcon Status");
        assert!(config.switch.unique_id.is_empty());
        assert!(config.switch.enabled);

        #[cfg(not(windows))]
        assert_eq!(config.serial.port, "/dev/ttyUSB0");
        #[cfg(windows)]
        assert_eq!(config.serial.port, "COM3");
        assert_eq!(config.serial.baud_rate, 9600);
        assert_eq!(config.serial.timeout, Duration::from_secs(2));

        assert_eq!(config.server.port, 11118);
        // Discovery is opt-in (absent by default) — see ServerConfig.
        assert!(config.server.discovery_port.is_none());
        assert!(config.server.tls.is_none());
        assert!(config.server.auth.is_none());
    }

    #[test]
    fn rotator_config_default() {
        let config = RotatorConfig::default();
        assert_eq!(config.name, "Pegasus Falcon Rotator");
        assert!(config.unique_id.is_empty());
        assert_eq!(
            config.description,
            "Pegasus Astro Falcon Rotator (firmware >= 1.3)"
        );
        assert!(config.enabled);
    }

    #[test]
    fn switch_config_default() {
        let config = SwitchConfig::default();
        assert_eq!(config.name, "Pegasus Falcon Status");
        assert!(config.unique_id.is_empty());
        assert!(config.description.contains("voltage"));
        assert!(config.description.contains("limit"));
        assert!(config.enabled);
    }

    #[test]
    fn serial_config_default() {
        let config = SerialConfig::default();
        #[cfg(not(windows))]
        assert_eq!(config.port, "/dev/ttyUSB0");
        #[cfg(windows)]
        assert_eq!(config.port, "COM3");
        assert_eq!(config.baud_rate, 9600);
        assert_eq!(config.timeout, Duration::from_secs(2));
    }

    #[test]
    fn server_config_default_uses_falcon_port() {
        let config = ServerConfig::default();
        assert_eq!(config.port, 11118);
    }

    #[test]
    fn config_serializes_to_json() {
        let config = Config::default();
        let json = serde_json::to_string(&config).unwrap();

        assert!(json.contains("Pegasus Falcon Rotator"));
        assert!(json.contains("Pegasus Falcon Status"));
        #[cfg(not(windows))]
        assert!(json.contains("/dev/ttyUSB0"));
        #[cfg(windows)]
        assert!(json.contains("COM3"));
        assert!(json.contains("9600"));
        assert!(json.contains("11118"));
    }

    #[test]
    fn config_deserializes_from_json() {
        let json = r#"{
            "serial": {
                "port": "/dev/ttyUSB1",
                "baud_rate": 9600,
                "timeout": "3s"
            },
            "server": { "port": 12345 },
            "rotator": {
                "name": "Test Rotator",
                "unique_id": "test-rotator-001",
                "description": "Test rotator description",
                "enabled": true
            },
            "switch": {
                "name": "Test Status",
                "unique_id": "test-status-001",
                "description": "Test status description",
                "enabled": false
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.serial.port, "/dev/ttyUSB1");
        assert_eq!(config.serial.timeout, Duration::from_secs(3));
        assert_eq!(config.server.port, 12345);
        assert_eq!(config.rotator.name, "Test Rotator");
        assert!(config.rotator.enabled);
        assert_eq!(config.switch.name, "Test Status");
        assert!(!config.switch.enabled);
    }

    #[test]
    fn config_deserializes_with_defaults() {
        let json = r#"{
            "serial": { "port": "/dev/ttyUSB2" },
            "server": { "port": 9999 },
            "rotator": {
                "name": "Minimal Rotator",
                "unique_id": "min-rotator-001",
                "description": "Minimal config"
            },
            "switch": {
                "name": "Minimal Status",
                "unique_id": "min-status-001",
                "description": "Minimal status"
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.serial.baud_rate, 9600);
        assert_eq!(config.serial.timeout, Duration::from_secs(2));
        assert!(config.rotator.enabled);
        assert!(config.switch.enabled);
    }

    #[test]
    fn load_config_from_file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");

        let json = serde_json::to_string(&Config::default()).unwrap();
        std::fs::write(&path, json).unwrap();

        let loaded = load_config(&path).unwrap();
        #[cfg(not(windows))]
        assert_eq!(loaded.serial.port, "/dev/ttyUSB0");
        #[cfg(windows)]
        assert_eq!(loaded.serial.port, "COM3");
        assert_eq!(loaded.server.port, 11118);
        assert_eq!(loaded.rotator.name, "Pegasus Falcon Rotator");
        assert_eq!(loaded.switch.name, "Pegasus Falcon Status");
    }

    #[test]
    fn load_config_nonexistent_file_errors() {
        let path = std::path::PathBuf::from("/tmp/pa_falcon_rotator_nonexistent_12345.json");
        let result = load_config(&path);
        assert!(result.is_err());
    }

    #[test]
    fn load_config_invalid_json_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "this is not json").unwrap();

        let result = load_config(&path);
        assert!(result.is_err());
    }

    #[test]
    fn config_clone_round_trip() {
        let config = Config::default();
        let cloned = config.clone();
        assert_eq!(config.rotator.name, cloned.rotator.name);
        assert_eq!(config.switch.name, cloned.switch.name);
    }

    #[test]
    fn config_debug_contains_struct_names() {
        let config = Config::default();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("Config"));
        assert!(debug_str.contains("SerialConfig"));
        assert!(debug_str.contains("ServerConfig"));
        assert!(debug_str.contains("RotatorConfig"));
        assert!(debug_str.contains("SwitchConfig"));
    }

    #[test]
    fn config_rejects_unknown_top_level_field() {
        let err = serde_json::from_str::<Config>(r#"{"typoed_key": 1}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("typoed_key"), "{err}");
    }

    #[test]
    fn serial_config_rejects_unknown_field() {
        let err = serde_json::from_str::<SerialConfig>(
            r#"{"port": "/dev/ttyUSB0", "baud_rate": 9600, "timeout": "2s", "flow_control": "none"}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("flow_control"), "{err}");
    }

    #[test]
    fn server_config_rejects_unknown_field() {
        let err =
            serde_json::from_str::<ServerConfig>(r#"{"port": 11118, "bind_address": "0.0.0.0"}"#)
                .unwrap_err()
                .to_string();
        assert!(err.contains("bind_address"), "{err}");
    }

    #[test]
    fn rotator_config_rejects_unknown_field() {
        let err = serde_json::from_str::<RotatorConfig>(
            r#"{"name": "T", "description": "T", "enable": true}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("enable"), "{err}");
    }

    #[test]
    fn switch_config_rejects_unknown_field() {
        let err = serde_json::from_str::<SwitchConfig>(
            r#"{"name": "T", "description": "T", "enable": true}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("enable"), "{err}");
    }
}
