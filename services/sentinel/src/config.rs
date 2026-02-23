//! Configuration types for the sentinel service

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Environment variable name for overriding the Pushover API token
const PUSHOVER_API_TOKEN_ENV: &str = "PUSHOVER_API_TOKEN";

/// Environment variable name for overriding the Pushover user key
const PUSHOVER_USER_KEY_ENV: &str = "PUSHOVER_USER_KEY";

/// Main configuration structure
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub monitors: Vec<MonitorConfig>,
    #[serde(default)]
    pub notifiers: Vec<NotifierConfig>,
    #[serde(default)]
    pub transitions: Vec<TransitionConfig>,
    #[serde(default)]
    pub dashboard: DashboardConfig,
}

impl Config {
    /// Resolve secrets from environment variables, overriding config file values.
    ///
    /// For each Pushover notifier, `PUSHOVER_API_TOKEN` and `PUSHOVER_USER_KEY`
    /// environment variables override the corresponding JSON config values when
    /// set and non-empty. Returns an error if either field is still empty after
    /// resolution.
    pub fn resolve_secrets(&mut self) -> crate::Result<()> {
        let env_api_token = std::env::var(PUSHOVER_API_TOKEN_ENV)
            .ok()
            .filter(|v| !v.is_empty());
        let env_user_key = std::env::var(PUSHOVER_USER_KEY_ENV)
            .ok()
            .filter(|v| !v.is_empty());

        for notifier in &mut self.notifiers {
            match notifier {
                NotifierConfig::Pushover {
                    api_token,
                    user_key,
                    ..
                } => {
                    if let Some(ref token) = env_api_token {
                        tracing::debug!(
                            "Overriding Pushover api_token from {} environment variable",
                            PUSHOVER_API_TOKEN_ENV
                        );
                        *api_token = token.clone();
                    }
                    if let Some(ref key) = env_user_key {
                        tracing::debug!(
                            "Overriding Pushover user_key from {} environment variable",
                            PUSHOVER_USER_KEY_ENV
                        );
                        *user_key = key.clone();
                    }

                    if api_token.is_empty() {
                        return Err(crate::SentinelError::Config(
                            "Pushover api_token is empty: set it in the config file or via the PUSHOVER_API_TOKEN environment variable".to_string(),
                        ));
                    }
                    if user_key.is_empty() {
                        return Err(crate::SentinelError::Config(
                            "Pushover user_key is empty: set it in the config file or via the PUSHOVER_USER_KEY environment variable".to_string(),
                        ));
                    }
                }
            }
        }

        Ok(())
    }
}

/// Monitor configuration with tagged enum for extensibility
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MonitorConfig {
    #[serde(rename = "alpaca_safety_monitor")]
    AlpacaSafetyMonitor {
        name: String,
        #[serde(default = "default_host")]
        host: String,
        #[serde(default = "default_alpaca_port")]
        port: u16,
        #[serde(default)]
        device_number: u32,
        #[serde(default = "default_polling_interval")]
        polling_interval_seconds: u64,
    },
}

impl MonitorConfig {
    pub fn name(&self) -> &str {
        match self {
            MonitorConfig::AlpacaSafetyMonitor { name, .. } => name,
        }
    }

    pub fn polling_interval_seconds(&self) -> u64 {
        match self {
            MonitorConfig::AlpacaSafetyMonitor {
                polling_interval_seconds,
                ..
            } => *polling_interval_seconds,
        }
    }
}

/// Notifier configuration with tagged enum for extensibility
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum NotifierConfig {
    #[serde(rename = "pushover")]
    Pushover {
        #[serde(default)]
        api_token: String,
        #[serde(default)]
        user_key: String,
        #[serde(default = "default_pushover_title")]
        default_title: String,
        #[serde(default)]
        default_priority: i8,
        #[serde(default = "default_pushover_sound")]
        default_sound: String,
    },
}

impl NotifierConfig {
    pub fn type_name(&self) -> &str {
        match self {
            NotifierConfig::Pushover { .. } => "pushover",
        }
    }
}

/// Transition rule configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionConfig {
    pub monitor_name: String,
    pub direction: TransitionDirection,
    pub notifiers: Vec<String>,
    #[serde(default = "default_message_template")]
    pub message_template: String,
    #[serde(default)]
    pub priority: Option<i8>,
    #[serde(default)]
    pub sound: Option<String>,
}

/// Direction of a state transition
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionDirection {
    SafeToUnsafe,
    UnsafeToSafe,
    Both,
}

/// Dashboard configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_dashboard_port")]
    pub port: u16,
    #[serde(default = "default_history_size")]
    pub history_size: usize,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: default_dashboard_port(),
            history_size: default_history_size(),
        }
    }
}

fn default_host() -> String {
    "localhost".to_string()
}

fn default_alpaca_port() -> u16 {
    11111
}

fn default_polling_interval() -> u64 {
    30
}

fn default_pushover_title() -> String {
    "Observatory Alert".to_string()
}

fn default_pushover_sound() -> String {
    "pushover".to_string()
}

fn default_message_template() -> String {
    "{monitor_name} changed to {new_state}".to_string()
}

fn default_true() -> bool {
    true
}

fn default_dashboard_port() -> u16 {
    11114
}

fn default_history_size() -> usize {
    100
}

/// Load configuration from a JSON file
pub fn load_config(path: &Path) -> crate::Result<Config> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        crate::SentinelError::Config(format!("Failed to read config file {:?}: {}", path, e))
    })?;
    let config: Config = serde_json::from_str(&content)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Mutex to serialize tests that mutate environment variables.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    fn pushover_config(api_token: &str, user_key: &str) -> Config {
        Config {
            notifiers: vec![NotifierConfig::Pushover {
                api_token: api_token.to_string(),
                user_key: user_key.to_string(),
                default_title: default_pushover_title(),
                default_priority: 0,
                default_sound: default_pushover_sound(),
            }],
            ..Config::default()
        }
    }

    #[test]
    fn parse_full_config() {
        let json = r#"{
            "monitors": [
                {
                    "type": "alpaca_safety_monitor",
                    "name": "Roof Safety Monitor",
                    "host": "localhost",
                    "port": 11111,
                    "device_number": 0,
                    "polling_interval_seconds": 30
                }
            ],
            "notifiers": [
                {
                    "type": "pushover",
                    "api_token": "test-token",
                    "user_key": "test-user",
                    "default_title": "Observatory Alert",
                    "default_priority": 0,
                    "default_sound": "pushover"
                }
            ],
            "transitions": [
                {
                    "monitor_name": "Roof Safety Monitor",
                    "direction": "safe_to_unsafe",
                    "notifiers": ["pushover"],
                    "message_template": "ALERT: {monitor_name} changed to {new_state}",
                    "priority": 1,
                    "sound": "siren"
                },
                {
                    "monitor_name": "Roof Safety Monitor",
                    "direction": "unsafe_to_safe",
                    "notifiers": ["pushover"],
                    "message_template": "OK: {monitor_name} is now {new_state}"
                }
            ],
            "dashboard": {
                "enabled": true,
                "port": 11114,
                "history_size": 100
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        assert_eq!(config.monitors.len(), 1);
        assert_eq!(config.monitors[0].name(), "Roof Safety Monitor");
        assert_eq!(config.monitors[0].polling_interval_seconds(), 30);

        assert_eq!(config.notifiers.len(), 1);
        assert_eq!(config.notifiers[0].type_name(), "pushover");

        assert_eq!(config.transitions.len(), 2);
        assert_eq!(
            config.transitions[0].direction,
            TransitionDirection::SafeToUnsafe
        );
        assert_eq!(config.transitions[0].priority, Some(1));
        assert_eq!(config.transitions[0].sound, Some("siren".to_string()));
        assert_eq!(
            config.transitions[1].direction,
            TransitionDirection::UnsafeToSafe
        );
        assert_eq!(config.transitions[1].priority, None);

        assert!(config.dashboard.enabled);
        assert_eq!(config.dashboard.port, 11114);
        assert_eq!(config.dashboard.history_size, 100);
    }

    #[test]
    fn parse_minimal_config() {
        let json = r#"{}"#;
        let config: Config = serde_json::from_str(json).unwrap();

        assert!(config.monitors.is_empty());
        assert!(config.notifiers.is_empty());
        assert!(config.transitions.is_empty());
        assert!(config.dashboard.enabled);
        assert_eq!(config.dashboard.port, 11114);
        assert_eq!(config.dashboard.history_size, 100);
    }

    #[test]
    fn parse_monitor_defaults() {
        let json = r#"{
            "monitors": [{
                "type": "alpaca_safety_monitor",
                "name": "Test Monitor"
            }]
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        match &config.monitors[0] {
            MonitorConfig::AlpacaSafetyMonitor {
                host,
                port,
                device_number,
                polling_interval_seconds,
                ..
            } => {
                assert_eq!(host, "localhost");
                assert_eq!(*port, 11111);
                assert_eq!(*device_number, 0);
                assert_eq!(*polling_interval_seconds, 30);
            }
        }
    }

    #[test]
    fn parse_notifier_defaults() {
        let json = r#"{
            "notifiers": [{
                "type": "pushover",
                "api_token": "tok",
                "user_key": "usr"
            }]
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        match &config.notifiers[0] {
            NotifierConfig::Pushover {
                default_title,
                default_priority,
                default_sound,
                ..
            } => {
                assert_eq!(default_title, "Observatory Alert");
                assert_eq!(*default_priority, 0);
                assert_eq!(default_sound, "pushover");
            }
        }
    }

    #[test]
    fn parse_transition_direction_both() {
        let json = r#"{
            "transitions": [{
                "monitor_name": "Test",
                "direction": "both",
                "notifiers": ["pushover"]
            }]
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.transitions[0].direction, TransitionDirection::Both);
    }

    #[test]
    fn load_config_missing_file() {
        let result = load_config(Path::new("/nonexistent/config.json"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Failed to read config file"));
    }

    #[test]
    fn load_config_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(
            &config_path,
            r#"{"monitors": [{"type": "alpaca_safety_monitor", "name": "Test"}]}"#,
        )
        .unwrap();

        let config = load_config(&config_path).unwrap();
        assert_eq!(config.monitors.len(), 1);
    }

    #[test]
    fn load_config_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, "not json").unwrap();

        let result = load_config(&config_path);
        assert!(result.is_err());
    }

    #[test]
    fn default_config() {
        let config = Config::default();
        assert!(config.monitors.is_empty());
        assert!(config.notifiers.is_empty());
        assert!(config.transitions.is_empty());
        assert!(config.dashboard.enabled);
    }

    #[test]
    fn resolve_secrets_env_overrides_config() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("PUSHOVER_API_TOKEN", "env-token");
        std::env::set_var("PUSHOVER_USER_KEY", "env-key");

        let mut config = pushover_config("json-token", "json-key");
        config.resolve_secrets().unwrap();

        match &config.notifiers[0] {
            NotifierConfig::Pushover {
                api_token,
                user_key,
                ..
            } => {
                assert_eq!(api_token, "env-token");
                assert_eq!(user_key, "env-key");
            }
        }

        std::env::remove_var("PUSHOVER_API_TOKEN");
        std::env::remove_var("PUSHOVER_USER_KEY");
    }

    #[test]
    fn resolve_secrets_falls_back_to_json() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PUSHOVER_API_TOKEN");
        std::env::remove_var("PUSHOVER_USER_KEY");

        let mut config = pushover_config("json-token", "json-key");
        config.resolve_secrets().unwrap();

        match &config.notifiers[0] {
            NotifierConfig::Pushover {
                api_token,
                user_key,
                ..
            } => {
                assert_eq!(api_token, "json-token");
                assert_eq!(user_key, "json-key");
            }
        }
    }

    #[test]
    fn resolve_secrets_error_when_both_empty() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PUSHOVER_API_TOKEN");
        std::env::remove_var("PUSHOVER_USER_KEY");

        let mut config = pushover_config("", "");
        let err = config.resolve_secrets().unwrap_err();
        assert!(err.to_string().contains("api_token is empty"));
    }

    #[test]
    fn resolve_secrets_empty_env_treated_as_unset() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("PUSHOVER_API_TOKEN", "");
        std::env::set_var("PUSHOVER_USER_KEY", "");

        let mut config = pushover_config("json-token", "json-key");
        config.resolve_secrets().unwrap();

        match &config.notifiers[0] {
            NotifierConfig::Pushover {
                api_token,
                user_key,
                ..
            } => {
                assert_eq!(api_token, "json-token");
                assert_eq!(user_key, "json-key");
            }
        }

        std::env::remove_var("PUSHOVER_API_TOKEN");
        std::env::remove_var("PUSHOVER_USER_KEY");
    }

    #[test]
    fn resolve_secrets_no_notifiers_is_ok() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut config = Config::default();
        config.resolve_secrets().unwrap();
    }

    #[test]
    fn parse_pushover_without_credentials() {
        let json = r#"{
            "notifiers": [{
                "type": "pushover"
            }]
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        match &config.notifiers[0] {
            NotifierConfig::Pushover {
                api_token,
                user_key,
                ..
            } => {
                assert_eq!(api_token, "");
                assert_eq!(user_key, "");
            }
        }
    }
}
