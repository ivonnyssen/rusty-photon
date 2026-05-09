//! Configuration types.
//!
//! See [`docs/services/star-adventurer-gti.md`](../../../docs/services/star-adventurer-gti.md)
//! §"Configuration" for the canonical schema and field meanings.

use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Top-level configuration deserialised from the JSON config file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub transport: TransportConfig,
    pub server: ServerConfig,
    pub mount: MountConfig,
}

/// Transport block — `usb` (serial) or `udp` (WiFi).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TransportConfig {
    Usb(UsbConfig),
    Udp(UdpConfig),
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self::Usb(UsbConfig::default())
    }
}

/// USB-CDC serial transport config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbConfig {
    pub port: String,
    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,
    #[serde(default = "default_command_timeout", with = "humantime_serde")]
    pub command_timeout: Duration,
    #[serde(default = "default_polling_interval", with = "humantime_serde")]
    pub polling_interval: Duration,
}

/// UDP/WiFi transport config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UdpConfig {
    pub address: IpAddr,
    #[serde(default = "default_udp_port")]
    pub port: u16,
    /// Mandatory on UDP — must be a 192.168.4.0/24 host IP when the mount is
    /// in AP mode.
    pub bind_address: IpAddr,
    #[serde(default = "default_command_timeout", with = "humantime_serde")]
    pub command_timeout: Duration,
    #[serde(default = "default_polling_interval", with = "humantime_serde")]
    pub polling_interval: Duration,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountConfig {
    pub name: String,
    pub unique_id: String,
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// WGS84 degrees, +N. ASCOM convention.
    pub site_latitude_deg: f64,
    /// WGS84 degrees, +E. ASCOM convention.
    pub site_longitude_deg: f64,
    #[serde(default)]
    pub site_elevation_m: f64,

    #[serde(default = "default_settle_after_slew", with = "humantime_serde")]
    pub settle_after_slew: Duration,

    /// Reserved for future expansion. Sidereal only in MVP.
    #[serde(default)]
    pub tracking_rate: TrackingRateName,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TrackingRateName {
    #[default]
    Sidereal,
}

fn default_baud_rate() -> u32 {
    115_200
}
fn default_command_timeout() -> Duration {
    Duration::from_secs(2)
}
fn default_polling_interval() -> Duration {
    Duration::from_millis(200)
}
fn default_udp_port() -> u16 {
    11_880
}
fn default_discovery_port() -> Option<u16> {
    Some(ascom_alpaca::discovery::DEFAULT_DISCOVERY_PORT)
}
fn default_settle_after_slew() -> Duration {
    Duration::from_secs(2)
}
fn default_true() -> bool {
    true
}

impl Default for UsbConfig {
    fn default() -> Self {
        Self {
            port: "/dev/ttyACM0".to_string(),
            baud_rate: default_baud_rate(),
            command_timeout: default_command_timeout(),
            polling_interval: default_polling_interval(),
        }
    }
}

impl Default for UdpConfig {
    fn default() -> Self {
        Self {
            address: "192.168.4.1".parse().unwrap(),
            port: default_udp_port(),
            bind_address: "192.168.4.2".parse().unwrap(),
            command_timeout: default_command_timeout(),
            polling_interval: default_polling_interval(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 11_117,
            discovery_port: default_discovery_port(),
            tls: None,
            auth: None,
        }
    }
}

impl Default for MountConfig {
    fn default() -> Self {
        Self {
            name: "Star Adventurer GTi".to_string(),
            unique_id: "skywatcher-sa-gti-001".to_string(),
            description: "Sky-Watcher Star Adventurer GTi German Equatorial Mount".to_string(),
            enabled: true,
            site_latitude_deg: 0.0,
            site_longitude_deg: 0.0,
            site_elevation_m: 0.0,
            settle_after_slew: default_settle_after_slew(),
            tracking_rate: TrackingRateName::Sidereal,
        }
    }
}

/// Load a [`Config`] from a JSON file.
pub fn load_config(path: &Path) -> std::result::Result<Config, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let config: Config = serde_json::from_str(&content)?;
    Ok(config)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_match_the_design_doc() {
        let cfg = Config::default();
        assert_eq!(cfg.server.port, 11_117);
        assert_eq!(cfg.mount.name, "Star Adventurer GTi");
        assert_eq!(cfg.mount.unique_id, "skywatcher-sa-gti-001");
        assert!(cfg.mount.enabled);
        assert_eq!(cfg.mount.tracking_rate, TrackingRateName::Sidereal);
        assert_eq!(cfg.mount.settle_after_slew, Duration::from_secs(2));
        assert!(matches!(cfg.transport, TransportConfig::Usb(_)));
    }

    #[test]
    fn usb_transport_default_uses_recommended_baud_rate() {
        let usb = UsbConfig::default();
        // Spec says 9600; in practice GTi USB also accepts 115200, which we
        // recommend (matches EQMOD docs).
        assert_eq!(usb.baud_rate, 115_200);
        assert_eq!(usb.command_timeout, Duration::from_secs(2));
        assert_eq!(usb.polling_interval, Duration::from_millis(200));
    }

    #[test]
    fn udp_transport_default_targets_the_ap_mode_address() {
        let udp = UdpConfig::default();
        assert_eq!(udp.address.to_string(), "192.168.4.1");
        assert_eq!(udp.port, 11_880);
        // bind_address is mandatory for UDP and must be on the 192.168.4.0/24
        // subnet when the mount is in AP mode.
        assert!(udp.bind_address.to_string().starts_with("192.168.4."));
    }

    #[test]
    fn config_round_trips_through_json() {
        let cfg = Config::default();
        let json = serde_json::to_string(&cfg).expect("serialise");
        let back: Config = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back.server.port, cfg.server.port);
        assert_eq!(back.mount.name, cfg.mount.name);
    }

    #[test]
    fn transport_config_deserialises_usb_variant() {
        let json = r#"{
            "kind": "usb",
            "port": "/dev/ttyACM0",
            "baud_rate": 9600,
            "command_timeout": "1s",
            "polling_interval": "100ms"
        }"#;
        let t: TransportConfig = serde_json::from_str(json).expect("usb variant");
        match t {
            TransportConfig::Usb(usb) => {
                assert_eq!(usb.port, "/dev/ttyACM0");
                assert_eq!(usb.baud_rate, 9600);
                assert_eq!(usb.command_timeout, Duration::from_secs(1));
                assert_eq!(usb.polling_interval, Duration::from_millis(100));
            }
            other => panic!("expected Usb, got {other:?}"),
        }
    }

    #[test]
    fn transport_config_deserialises_udp_variant_with_bind_address() {
        let json = r#"{
            "kind": "udp",
            "address": "192.168.4.1",
            "port": 11880,
            "bind_address": "192.168.4.7",
            "command_timeout": "2s",
            "polling_interval": "200ms"
        }"#;
        let t: TransportConfig = serde_json::from_str(json).expect("udp variant");
        match t {
            TransportConfig::Udp(udp) => {
                assert_eq!(udp.address.to_string(), "192.168.4.1");
                assert_eq!(udp.bind_address.to_string(), "192.168.4.7");
            }
            other => panic!("expected Udp, got {other:?}"),
        }
    }
}
