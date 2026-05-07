use std::time::Duration;

use serde::Deserialize;

/// `rp` deployments have at most one mount — piggyback rigs share one
/// mount across multiple optical trains (multiple cameras / focusers /
/// filter wheels). Multi-mount support is in `rp.md` Future
/// Considerations. The singular `Option` reflects that contract in the
/// type; `None` is valid for camera-only / flats-rig configurations.
#[derive(Debug, Clone, Deserialize)]
pub struct MountConfig {
    pub alpaca_url: String,
    #[serde(default)]
    pub device_number: u32,
    /// Mechanical settle time applied after the mount reports
    /// `Slewing == false`, before `slew` returns. Set per-rig (gear
    /// backlash, mount mass, etc.) — defaults to zero. Per-call
    /// `settle_after` on `slew` overrides this value (including
    /// `"0s"` to skip).
    #[serde(default, with = "humantime_serde")]
    pub settle_after_slew: Option<Duration>,
    /// Optional HTTP Basic Auth credentials for connecting to auth-enabled Alpaca services
    #[serde(default)]
    pub auth: Option<rp_auth::config::ClientAuthConfig>,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use crate::config::load_config;

    #[test]
    fn mount_config_minimal_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "mount": {
                        "alpaca_url": "http://localhost:11122"
                    }
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        let m = config.equipment.mount.as_ref().unwrap();
        assert_eq!(m.alpaca_url, "http://localhost:11122");
        assert_eq!(m.device_number, 0);
        assert!(m.settle_after_slew.is_none());
        assert!(m.auth.is_none());
    }

    #[test]
    fn mount_config_with_settle_and_auth() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "mount": {
                        "alpaca_url": "http://localhost:11122",
                        "device_number": 1,
                        "settle_after_slew": "3s",
                        "auth": {"username": "u", "password": "p"}
                    }
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        let m = config.equipment.mount.as_ref().unwrap();
        assert_eq!(m.device_number, 1);
        assert_eq!(m.settle_after_slew, Some(Duration::from_secs(3)));
        let auth = m.auth.as_ref().unwrap();
        assert_eq!(auth.username, "u");
        assert_eq!(auth.password, "p");
    }

    #[test]
    fn mount_config_omitted_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        assert!(config.equipment.mount.is_none());
    }
}
