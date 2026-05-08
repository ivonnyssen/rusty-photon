use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct FocuserConfig {
    pub id: String,
    #[serde(default)]
    pub camera_id: String,
    pub alpaca_url: String,
    #[serde(default)]
    pub device_number: u32,
    /// Operator-supplied lower bound for `move_focuser` validation. The
    /// device-reported `max_step` is the hardware ceiling; these fields
    /// let the operator enforce a tighter safe-travel range.
    #[serde(default)]
    pub min_position: Option<i32>,
    /// Operator-supplied upper bound for `move_focuser` validation.
    #[serde(default)]
    pub max_position: Option<i32>,
    /// Optional HTTP Basic Auth credentials for connecting to auth-enabled Alpaca services
    #[serde(default)]
    pub auth: Option<rp_auth::config::ClientAuthConfig>,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use crate::config::load_config;

    #[test]
    fn focuser_config_minimal_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "focusers": [
                        {
                            "id": "main-focuser",
                            "alpaca_url": "http://localhost:11113"
                        }
                    ]
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(config.equipment.focusers.len(), 1);
        let f = &config.equipment.focusers[0];
        assert_eq!(f.id, "main-focuser");
        assert_eq!(f.alpaca_url, "http://localhost:11113");
        assert_eq!(f.device_number, 0);
        assert!(f.min_position.is_none());
        assert!(f.max_position.is_none());
        assert!(f.auth.is_none());
    }

    #[test]
    fn focuser_config_with_bounds_and_auth() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "focusers": [
                        {
                            "id": "main-focuser",
                            "camera_id": "main-cam",
                            "alpaca_url": "http://localhost:11113",
                            "device_number": 2,
                            "min_position": 0,
                            "max_position": 100000,
                            "auth": {"username": "u", "password": "p"}
                        }
                    ]
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        let f = &config.equipment.focusers[0];
        assert_eq!(f.camera_id, "main-cam");
        assert_eq!(f.device_number, 2);
        assert_eq!(f.min_position, Some(0));
        assert_eq!(f.max_position, Some(100000));
        let auth = f.auth.as_ref().unwrap();
        assert_eq!(auth.username, "u");
        assert_eq!(auth.password, "p");
    }
}
