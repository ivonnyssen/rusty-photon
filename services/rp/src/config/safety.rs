use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Safety-enforcement settings (rp.md § Safety). The monitors themselves
/// are equipment (`equipment.safety_monitors`); this block holds the
/// enforcement knobs shared across them.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SafetyConfig {
    /// How often every configured SafetyMonitor is polled (default `"10s"`).
    #[serde(default = "default_safety_poll_interval", with = "humantime_serde")]
    #[schemars(with = "String")]
    pub poll_interval: Duration,
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            poll_interval: default_safety_poll_interval(),
        }
    }
}

fn default_safety_poll_interval() -> Duration {
    Duration::from_secs(10)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use std::time::Duration;

    use crate::config::load_config;
    use crate::config::test_support::MINIMAL_CONFIG_JSON;

    #[test]
    fn safety_block_omitted_applies_default_poll_interval() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, MINIMAL_CONFIG_JSON).unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(config.safety.poll_interval, Duration::from_secs(10));
    }

    #[test]
    fn safety_poll_interval_parses_humantime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "safety": {"poll_interval": "250ms"},
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(config.safety.poll_interval, Duration::from_millis(250));
    }

    #[test]
    fn safety_block_rejects_unknown_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "safety": {"park_on_unsafe": true},
                "server": {}
            }"#,
        )
        .unwrap();

        let err = load_config(&path).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("park_on_unsafe") || msg.contains("unknown field"),
            "expected unknown-field diagnostic, got: {msg}"
        );
    }
}
