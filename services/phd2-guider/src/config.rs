//! Configuration types for the PHD2 guider service

use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// PHD2 service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// HTTP listen address (`serve` mode only; the CLI ignores it).
    /// Typed as `IpAddr` so a malformed address fails at config load.
    #[serde(default = "default_bind_address")]
    pub bind_address: IpAddr,
    /// HTTP port (`serve` mode only). `0` auto-assigns — used by tests.
    #[serde(default = "default_service_port")]
    pub port: u16,
    /// How long `POST /api/v1/guiding/stop` waits for PHD2 to reach
    /// the `Stopped` state (`serve` mode only).
    #[serde(default = "default_stop_timeout", with = "humantime_serde")]
    pub stop_timeout: Duration,
    #[serde(default)]
    pub phd2: Phd2Config,
    #[serde(default)]
    pub settling: SettleParams,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind_address: default_bind_address(),
            port: default_service_port(),
            stop_timeout: default_stop_timeout(),
            phd2: Phd2Config::default(),
            settling: SettleParams::default(),
        }
    }
}

fn default_bind_address() -> IpAddr {
    IpAddr::from([127, 0, 0, 1])
}

fn default_service_port() -> u16 {
    11130
}

fn default_stop_timeout() -> Duration {
    Duration::from_secs(10)
}

/// PHD2 connection settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Phd2Config {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub executable_path: Option<PathBuf>,
    #[serde(default = "default_connection_timeout", with = "humantime_serde")]
    pub connection_timeout: Duration,
    #[serde(default = "default_command_timeout", with = "humantime_serde")]
    pub command_timeout: Duration,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub auto_connect_equipment: bool,
    #[serde(default)]
    pub reconnect: ReconnectConfig,
    /// Environment variables to set when spawning the PHD2 process
    #[serde(default)]
    pub spawn_env: std::collections::HashMap<String, String>,
}

/// Configuration for automatic reconnection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconnectConfig {
    /// Enable automatic reconnection when connection is lost
    #[serde(default = "default_reconnect_enabled")]
    pub enabled: bool,
    /// Interval between reconnection attempts
    #[serde(default = "default_reconnect_interval", with = "humantime_serde")]
    pub interval: Duration,
    /// Maximum number of reconnection attempts (None for unlimited)
    #[serde(default)]
    pub max_retries: Option<u32>,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            enabled: default_reconnect_enabled(),
            interval: default_reconnect_interval(),
            max_retries: None,
        }
    }
}

fn default_reconnect_enabled() -> bool {
    true
}

fn default_reconnect_interval() -> Duration {
    Duration::from_secs(5)
}

impl Default for Phd2Config {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            executable_path: None,
            connection_timeout: default_connection_timeout(),
            command_timeout: default_command_timeout(),
            auto_start: false,
            auto_connect_equipment: false,
            reconnect: ReconnectConfig::default(),
            spawn_env: std::collections::HashMap::new(),
        }
    }
}

fn default_host() -> String {
    "localhost".to_string()
}

fn default_port() -> u16 {
    4400
}

fn default_connection_timeout() -> Duration {
    Duration::from_secs(10)
}

fn default_command_timeout() -> Duration {
    Duration::from_secs(30)
}

/// Settling parameters for guiding operations.
///
/// This struct is the operator-facing config representation: durations are
/// `std::time::Duration` and use humantime strings on the wire (`"10s"`).
/// When sending settle parameters into PHD2's JSON-RPC payload, the call
/// sites in `client.rs` convert `time` and `timeout` to integer seconds via
/// `settle_secs_ceil`, because the PHD2 protocol requires integer values and
/// ceil-rounding avoids truncating sub-second durations down to `0`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettleParams {
    #[serde(default = "default_settle_pixels")]
    pub pixels: f64,
    #[serde(default = "default_settle_time", with = "humantime_serde")]
    pub time: Duration,
    #[serde(default = "default_settle_timeout", with = "humantime_serde")]
    pub timeout: Duration,
}

impl Default for SettleParams {
    fn default() -> Self {
        Self {
            pixels: default_settle_pixels(),
            time: default_settle_time(),
            timeout: default_settle_timeout(),
        }
    }
}

fn default_settle_pixels() -> f64 {
    0.5
}

fn default_settle_time() -> Duration {
    Duration::from_secs(10)
}

fn default_settle_timeout() -> Duration {
    Duration::from_secs(60)
}

/// Load configuration from a JSON file
pub fn load_config(
    path: &Path,
) -> std::result::Result<Config, Box<dyn std::error::Error + Send + Sync>> {
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
    fn test_settle_params_default() {
        let params = SettleParams::default();
        assert_eq!(params.pixels, 0.5);
        assert_eq!(params.time, Duration::from_secs(10));
        assert_eq!(params.timeout, Duration::from_secs(60));
    }

    #[test]
    fn test_phd2_config_default() {
        let config = Phd2Config::default();
        assert_eq!(config.host, "localhost");
        assert_eq!(config.port, 4400);
        assert_eq!(config.connection_timeout, Duration::from_secs(10));
        assert_eq!(config.command_timeout, Duration::from_secs(30));
        assert!(!config.auto_start);
        assert!(!config.auto_connect_equipment);
        assert!(config.reconnect.enabled);
        assert_eq!(config.reconnect.interval, Duration::from_secs(5));
        assert!(config.reconnect.max_retries.is_none());
    }

    #[test]
    fn test_reconnect_config_default() {
        let config = ReconnectConfig::default();
        assert!(config.enabled);
        assert_eq!(config.interval, Duration::from_secs(5));
        assert!(config.max_retries.is_none());
    }

    #[test]
    fn test_reconnect_config_serialization() {
        let config = ReconnectConfig {
            enabled: true,
            interval: Duration::from_secs(10),
            max_retries: Some(5),
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["enabled"], true);
        assert_eq!(json["interval"], "10s");
        assert_eq!(json["max_retries"], 5);
    }

    #[test]
    fn test_settle_params_serialization() {
        let params = SettleParams {
            pixels: 1.5,
            time: Duration::from_secs(15),
            timeout: Duration::from_secs(120),
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["pixels"], 1.5);
        assert_eq!(json["time"], "15s");
        assert_eq!(json["timeout"], "2m");
    }

    #[test]
    fn an_empty_config_loads_with_the_serve_mode_defaults() {
        let config: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(config.bind_address, IpAddr::from([127, 0, 0, 1]));
        assert_eq!(config.port, 11130);
        assert_eq!(config.stop_timeout, Duration::from_secs(10));
        assert_eq!(config.phd2.host, "localhost");
        assert_eq!(config.settling.pixels, 0.5);
    }

    #[test]
    fn the_serve_mode_fields_parse_from_json() {
        let config: Config = serde_json::from_str(
            r#"{
                "bind_address": "0.0.0.0",
                "port": 0,
                "stop_timeout": "1s",
                "phd2": { "host": "127.0.0.1", "port": 14400 }
            }"#,
        )
        .unwrap();
        assert_eq!(config.bind_address, IpAddr::from([0, 0, 0, 0]));
        assert_eq!(config.port, 0);
        assert_eq!(config.stop_timeout, Duration::from_secs(1));
        assert_eq!(config.phd2.port, 14400);
    }

    #[test]
    fn a_malformed_bind_address_fails_at_config_load() {
        let err = serde_json::from_str::<Config>(r#"{"bind_address": "not-an-ip"}"#).unwrap_err();
        assert!(err.to_string().contains("invalid IP address"));
    }
}
