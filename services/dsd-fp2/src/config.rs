//! Configuration types for the Deep Sky Dad FP2 driver.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// Top-level service configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub serial: SerialConfig,
    pub server: ServerConfig,
    pub cover_calibrator: CoverCalibratorConfig,
    /// Opt in to `ServiceLifetime` transport mode (eager hardware
    /// validation at startup; service exits non-zero if the FP2
    /// doesn't respond). Default `false` preserves pre-Phase-2
    /// lazy-acquire behaviour. See
    /// `docs/plans/eager-hardware-validation.md`.
    #[serde(default)]
    pub validate_on_start: bool,
}

/// Serial port configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerialConfig {
    pub port: String,
    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,
    #[serde(default = "default_polling_interval", with = "humantime_serde")]
    pub polling_interval: Duration,
    #[serde(default = "default_timeout", with = "humantime_serde")]
    pub timeout: Duration,
}

/// Server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
    #[serde(default = "default_discovery_port")]
    pub discovery_port: Option<u16>,
    #[serde(default)]
    pub tls: Option<rp_tls::config::TlsConfig>,
    #[serde(default)]
    pub auth: Option<rp_auth::config::AuthConfig>,
}

/// CoverCalibrator device configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverCalibratorConfig {
    pub name: String,
    pub unique_id: String,
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_brightness")]
    pub max_brightness: u32,
}

fn default_baud_rate() -> u32 {
    115_200
}

fn default_polling_interval() -> Duration {
    Duration::from_millis(500)
}

fn default_timeout() -> Duration {
    Duration::from_secs(3)
}

fn default_discovery_port() -> Option<u16> {
    Some(ascom_alpaca::discovery::DEFAULT_DISCOVERY_PORT)
}

fn default_true() -> bool {
    true
}

fn default_max_brightness() -> u32 {
    4096
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            port: "/dev/ttyACM0".to_string(),
            baud_rate: default_baud_rate(),
            polling_interval: default_polling_interval(),
            timeout: default_timeout(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 11119,
            discovery_port: default_discovery_port(),
            tls: None,
            auth: None,
        }
    }
}

impl Default for CoverCalibratorConfig {
    fn default() -> Self {
        Self {
            name: "Deep Sky Dad FP2".to_string(),
            unique_id: "dsd-fp2-001".to_string(),
            description: "Deep Sky Dad Flat Panel 2 (motorised flat field panel)".to_string(),
            enabled: true,
            max_brightness: default_max_brightness(),
        }
    }
}

/// Load configuration from a JSON file.
pub fn load_config(path: &Path) -> std::result::Result<Config, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let config: Config = serde_json::from_str(&content)?;
    Ok(config)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let c = Config::default();
        assert_eq!(c.serial.port, "/dev/ttyACM0");
        assert_eq!(c.serial.baud_rate, 115_200);
        assert_eq!(c.serial.polling_interval, Duration::from_millis(500));
        assert_eq!(c.serial.timeout, Duration::from_secs(3));
        assert_eq!(c.server.port, 11119);
        assert!(c.cover_calibrator.enabled);
        assert_eq!(c.cover_calibrator.max_brightness, 4096);
        assert_eq!(c.cover_calibrator.name, "Deep Sky Dad FP2");
        assert_eq!(c.cover_calibrator.unique_id, "dsd-fp2-001");
    }

    #[test]
    fn config_serialises_to_json() {
        let c = Config::default();
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("/dev/ttyACM0"));
        assert!(json.contains("115200"));
        assert!(json.contains("11119"));
        assert!(json.contains("Deep Sky Dad FP2"));
    }

    #[test]
    fn config_deserialises_with_defaults() {
        let json = r#"{
            "serial": { "port": "/dev/ttyACM5" },
            "server": { "port": 9000 },
            "cover_calibrator": {
                "name": "FP2",
                "unique_id": "fp2-x",
                "description": "x"
            }
        }"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.serial.port, "/dev/ttyACM5");
        assert_eq!(c.serial.baud_rate, 115_200);
        assert_eq!(c.serial.polling_interval, Duration::from_millis(500));
        assert_eq!(c.server.port, 9000);
        assert_eq!(c.cover_calibrator.max_brightness, 4096);
        assert!(c.cover_calibrator.enabled);
    }

    #[test]
    fn config_deserialises_full_override() {
        let json = r#"{
            "serial": {
                "port": "/dev/serial/by-id/usb-foo",
                "baud_rate": 9600,
                "polling_interval": "250ms",
                "timeout": "2s"
            },
            "server": {
                "port": 12345
            },
            "cover_calibrator": {
                "name": "Test FP",
                "unique_id": "tfp-1",
                "description": "test",
                "enabled": false,
                "max_brightness": 2048
            }
        }"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.serial.port, "/dev/serial/by-id/usb-foo");
        assert_eq!(c.serial.baud_rate, 9600);
        assert_eq!(c.serial.polling_interval, Duration::from_millis(250));
        assert_eq!(c.serial.timeout, Duration::from_secs(2));
        assert_eq!(c.server.port, 12345);
        assert!(!c.cover_calibrator.enabled);
        assert_eq!(c.cover_calibrator.max_brightness, 2048);
    }

    #[test]
    fn load_config_reads_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "serial": { "port": "/dev/ttyACM9" },
                "server": { "port": 8888 },
                "cover_calibrator": {
                    "name": "From File",
                    "unique_id": "ff-1",
                    "description": "loaded"
                }
            }"#,
        )
        .unwrap();
        let c = load_config(&path).unwrap();
        assert_eq!(c.serial.port, "/dev/ttyACM9");
        assert_eq!(c.server.port, 8888);
        assert_eq!(c.cover_calibrator.name, "From File");
    }

    #[test]
    fn load_config_missing_file_errors() {
        let path = std::path::PathBuf::from("/tmp/dsd_fp2_nonexistent_98765.json");
        load_config(&path).unwrap_err();
    }

    #[test]
    fn load_config_invalid_json_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "not json").unwrap();
        load_config(&path).unwrap_err();
    }

    #[test]
    fn config_clone_and_debug_work() {
        let c = Config::default();
        let cloned = c.clone();
        assert_eq!(cloned.server.port, c.server.port);
        let dbg = format!("{:?}", c);
        assert!(dbg.contains("Config"));
        assert!(dbg.contains("CoverCalibratorConfig"));
    }
}
