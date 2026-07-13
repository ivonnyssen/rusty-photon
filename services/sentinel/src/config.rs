//! Configuration types for the sentinel service

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// Environment variable name for overriding the Pushover API token
const PUSHOVER_API_TOKEN_ENV: &str = "PUSHOVER_API_TOKEN";

/// Environment variable name for overriding the Pushover user key
const PUSHOVER_USER_KEY_ENV: &str = "PUSHOVER_USER_KEY";

/// Main configuration structure
///
/// `deny_unknown_fields` so typoed or removed top-level keys fail loudly at
/// load instead of being silently ignored.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub monitors: Vec<MonitorConfig>,
    #[serde(default)]
    pub notifiers: Vec<NotifierConfig>,
    #[serde(default)]
    pub transitions: Vec<TransitionConfig>,
    #[serde(default)]
    pub dashboard: DashboardConfig,
    /// Path to CA certificate for trusting TLS-enabled services
    #[serde(default)]
    pub ca_cert: Option<String>,
    /// Supervised services, keyed by an operator-chosen name. The registry has
    /// two consumers: the dashboard's `POST /api/services/{name}/restart`
    /// endpoint and the operation watchdog's corrective ladder
    /// (`operations.<family>.service` references keys here). See
    /// [`ServiceConfig`].
    #[serde(default)]
    pub services: std::collections::HashMap<String, ServiceConfig>,
    /// Optional push-based operation watchdog. Absent means safety polling
    /// only (today's behavior). See [`OperationWatchdogConfig`].
    #[serde(default)]
    pub operation_watchdog: Option<OperationWatchdogConfig>,
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
#[serde(tag = "type", deny_unknown_fields)]
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
        #[serde(default = "default_polling_interval", with = "humantime_serde")]
        polling_interval: Duration,
        /// URL scheme: "http" (default) or "https" for TLS-enabled services
        #[serde(default = "default_scheme")]
        scheme: String,
        /// Optional HTTP Basic Auth credentials for connecting to auth-enabled services
        #[serde(default)]
        auth: Option<rp_auth::config::ClientAuthConfig>,
    },
}

impl MonitorConfig {
    pub fn name(&self) -> &str {
        match self {
            MonitorConfig::AlpacaSafetyMonitor { name, .. } => name,
        }
    }

    pub fn polling_interval(&self) -> Duration {
        match self {
            MonitorConfig::AlpacaSafetyMonitor {
                polling_interval, ..
            } => *polling_interval,
        }
    }
}

/// Notifier configuration with tagged enum for extensibility
#[derive(Clone, Serialize, Deserialize, derive_more::Debug)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum NotifierConfig {
    #[serde(rename = "pushover")]
    Pushover {
        #[serde(default)]
        #[debug("<redacted>")]
        api_token: String,
        #[serde(default)]
        #[debug("<redacted>")]
        user_key: String,
        #[serde(default = "default_pushover_title")]
        default_title: String,
        #[serde(default)]
        default_priority: i8,
        #[serde(default = "default_pushover_sound")]
        default_sound: String,
        /// Override the Pushover API endpoint. Defaults to the public
        /// `https://api.pushover.net/1/messages.json`. Set this to point at a
        /// local stub (BDD tests) or a self-hosted Pushover-compatible relay.
        #[serde(default)]
        api_url: Option<String>,
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
#[serde(deny_unknown_fields)]
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

/// Operation watchdog configuration — the push-based event monitor that
/// subscribes to an rp event stream and tracks per-operation deadlines.
///
/// Optional: when the `operation_watchdog` block is absent, sentinel runs
/// safety polling only. See `docs/services/sentinel.md` §Operation Watchdog.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationWatchdogConfig {
    /// Base URL of the rp instance to watch. The watchdog subscribes to
    /// `{rp_url}/api/events/subscribe`.
    pub rp_url: String,
    /// Consecutive reconnect attempts before escalating "rp unresponsive".
    /// `0` means never give up (keep retrying without escalating).
    #[serde(default = "default_reconnect_max_attempts")]
    pub reconnect_max_attempts: u32,
    /// Delay between reconnect attempts.
    #[serde(default = "default_reconnect_backoff", with = "humantime_serde")]
    pub reconnect_backoff: Duration,
    /// Buffer added to `max_duration_ms` for operation families that have no
    /// explicit `operations` entry.
    #[serde(default = "default_watchdog_buffer", with = "humantime_serde")]
    pub default_buffer: Duration,
    /// Which notifier `type`s receive escalations. Empty means every
    /// configured notifier.
    #[serde(default)]
    pub notifiers: Vec<String>,
    /// Escalation message template. Placeholders: `{operation}`,
    /// `{operation_id}`, `{elapsed}`, `{reason}`, `{action}` (the
    /// corrective-action summary, empty for `notify_only`).
    #[serde(default = "default_watchdog_message_template")]
    pub message_template: String,
    /// Per-operation-family policy overrides, keyed by family (the event
    /// name with its `_started` / `_complete` / `_failed` suffix stripped).
    /// `operations.<family>.service` references the **top-level**
    /// [`Config::services`] map.
    #[serde(default)]
    pub operations: std::collections::HashMap<String, OperationPolicy>,
}

/// Per-operation-family watchdog policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationPolicy {
    /// Buffer added to this family's `max_duration_ms`. Falls back to the
    /// watchdog's `default_buffer` when absent.
    #[serde(default, with = "humantime_serde")]
    pub buffer: Option<Duration>,
    /// Corrective-action policy on expiry.
    #[serde(default)]
    pub on_expiry: OnExpiry,
    /// Service (a key into [`OperationWatchdogConfig::services`]) that owns
    /// this family. Required for `abort_then_restart`; an `abort_then_restart`
    /// family with no resolvable `service` degrades to `notify_only`.
    #[serde(default)]
    pub service: Option<String>,
}

/// Corrective-action policy selector — what the watchdog does when an
/// operation of this family misses its deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnExpiry {
    /// Notify through the `Notifier` chain only (the default, and the only
    /// action for liveness triggers).
    #[default]
    NotifyOnly,
    /// Run the corrective ladder (health → abort → restart) against the
    /// family's `service`, then notify.
    AbortThenRestart,
}

/// One supervised service: how the watchdog ladder probes/aborts it and how
/// both the ladder and the restart endpoint restart it. Commands run through
/// the platform shell (`sh -c` on unix, `cmd /C` on windows).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfig {
    /// Alpaca API base of the service, e.g. `http://host:port/api/v1`. The
    /// watchdog ladder appends `{device-type}/{device_number}/connected`
    /// (health) or the family's abort verb to it. Optional so non-Alpaca
    /// services (e.g. `rp`) can be restart-only entries; without it the
    /// ladder cannot probe or abort and falls through to restart.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Alpaca device number for the health-check / abort URLs.
    #[serde(default)]
    pub device_number: u32,
    /// Shell command that restarts the service. `null` marks the service as
    /// not restartable (the ladder stops at abort; the restart endpoint
    /// answers 409) — the canonical example is a remote MCU we cannot
    /// `systemctl`.
    #[serde(default)]
    pub restart_command: Option<String>,
    /// Shell command whose exit 0 means the service is healthy (e.g.
    /// `systemctl --user is-active dsd-fp2`). When set, the restart endpoint
    /// polls it after `restart_command` to confirm recovery; when absent,
    /// recovery confirmation is skipped. Not used by the watchdog ladder
    /// (which probes over HTTP via `base_url`).
    #[serde(default)]
    pub health_command: Option<String>,
    /// Time budget for `restart_command` to exit *and* the service to become
    /// healthy/responsive again — one budget for both phases, used by the
    /// restart endpoint and the watchdog ladder's restart rung.
    #[serde(default = "default_max_restart_duration", with = "humantime_serde")]
    pub max_restart_duration: Duration,
    /// Optional HTTP health supervision. Presence of this block opts the
    /// service in: sentinel probes `health.url` on a cadence and autonomously
    /// runs `restart_command` after consecutive failures.
    #[serde(default)]
    pub health: Option<HealthConfig>,
}

/// Per-service HTTP health supervision: sentinel GETs `url` every
/// `poll_interval` and, after `failure_threshold` consecutive failures, runs
/// the service's `restart_command` — backing off between attempts (doubling
/// from `restart_backoff` up to `restart_backoff_max`) while probing
/// continues at the same cadence. Only a clean 200 counts as alive; the
/// response body is never parsed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthConfig {
    /// Health URL probed with GET, e.g. `http://localhost:11131/health`.
    pub url: String,
    /// Probe cadence. Unchanged during outages — only restarting backs off.
    #[serde(default = "default_health_poll_interval", with = "humantime_serde")]
    pub poll_interval: Duration,
    /// Consecutive failed probes before the first autonomous restart.
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u32,
    /// Wait before a second restart attempt when the first did not cure the
    /// service; doubles after every attempt.
    #[serde(default = "default_restart_backoff", with = "humantime_serde")]
    pub restart_backoff: Duration,
    /// Ceiling for the doubling backoff.
    #[serde(default = "default_restart_backoff_max", with = "humantime_serde")]
    pub restart_backoff_max: Duration,
}

/// Dashboard configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DashboardConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_dashboard_port")]
    pub port: u16,
    #[serde(default = "default_history_size")]
    pub history_size: usize,
    #[serde(default)]
    pub tls: Option<rp_tls::config::TlsConfig>,
    #[serde(default)]
    pub auth: Option<rp_auth::config::AuthConfig>,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: default_dashboard_port(),
            history_size: default_history_size(),
            tls: None,
            auth: None,
        }
    }
}

fn default_scheme() -> String {
    "http".to_string()
}

fn default_host() -> String {
    "localhost".to_string()
}

fn default_alpaca_port() -> u16 {
    11111
}

fn default_polling_interval() -> Duration {
    Duration::from_secs(30)
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

fn default_reconnect_max_attempts() -> u32 {
    5
}

fn default_reconnect_backoff() -> Duration {
    Duration::from_secs(5)
}

fn default_watchdog_buffer() -> Duration {
    Duration::from_secs(10)
}

fn default_max_restart_duration() -> Duration {
    Duration::from_secs(60)
}

fn default_health_poll_interval() -> Duration {
    Duration::from_secs(30)
}

fn default_failure_threshold() -> u32 {
    3
}

fn default_restart_backoff() -> Duration {
    Duration::from_secs(60)
}

fn default_restart_backoff_max() -> Duration {
    Duration::from_secs(900)
}

fn default_watchdog_message_template() -> String {
    "Operation {operation} ({operation_id}) {reason} after {elapsed}{action}".to_string()
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
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Mutex to serialize tests that mutate environment variables.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn pushover_notifier_redacts_secrets_in_debug() {
        let config = pushover_config("super-secret-token", "super-secret-userkey");
        let rendered = format!("{:?}", config.notifiers[0]);
        assert!(
            !rendered.contains("super-secret-token") && !rendered.contains("super-secret-userkey"),
            "pushover credentials leaked into Debug: {rendered}"
        );
        assert!(rendered.contains("<redacted>"));
    }

    fn pushover_config(api_token: &str, user_key: &str) -> Config {
        Config {
            notifiers: vec![NotifierConfig::Pushover {
                api_token: api_token.to_string(),
                user_key: user_key.to_string(),
                default_title: default_pushover_title(),
                default_priority: 0,
                default_sound: default_pushover_sound(),
                api_url: None,
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
                    "polling_interval": "30s"
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
        assert_eq!(
            config.monitors[0].polling_interval(),
            Duration::from_secs(30)
        );

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
                polling_interval,
                ..
            } => {
                assert_eq!(host, "localhost");
                assert_eq!(*port, 11111);
                assert_eq!(*device_number, 0);
                assert_eq!(*polling_interval, Duration::from_secs(30));
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
    fn operation_watchdog_absent_by_default() {
        let config: Config = serde_json::from_str("{}").unwrap();
        assert!(config.operation_watchdog.is_none());
    }

    #[test]
    fn parse_operation_watchdog_full() {
        let json = r#"{
            "services": {
                "mount": {
                    "base_url": "http://localhost:11112/api/v1",
                    "device_number": 2,
                    "restart_command": "systemctl restart mount",
                    "health_command": "systemctl is-active mount",
                    "max_restart_duration": "45s"
                },
                "camera": { "base_url": "http://localhost:11111/api/v1" }
            },
            "operation_watchdog": {
                "rp_url": "http://localhost:8080",
                "reconnect_max_attempts": 3,
                "reconnect_backoff": "2s",
                "default_buffer": "15s",
                "notifiers": ["pushover"],
                "message_template": "{operation} stuck",
                "operations": {
                    "slew": { "buffer": "5s", "on_expiry": "abort_then_restart", "service": "mount" },
                    "park": { "on_expiry": "notify_only" }
                }
            }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        let wd = config.operation_watchdog.unwrap();
        assert_eq!(wd.rp_url, "http://localhost:8080");
        assert_eq!(wd.reconnect_max_attempts, 3);
        assert_eq!(wd.reconnect_backoff, Duration::from_secs(2));
        assert_eq!(wd.default_buffer, Duration::from_secs(15));
        assert_eq!(wd.notifiers, vec!["pushover".to_string()]);
        assert_eq!(wd.message_template, "{operation} stuck");

        let slew = wd.operations.get("slew").unwrap();
        assert_eq!(slew.buffer, Some(Duration::from_secs(5)));
        assert_eq!(slew.on_expiry, OnExpiry::AbortThenRestart);
        assert_eq!(slew.service.as_deref(), Some("mount"));

        let park = wd.operations.get("park").unwrap();
        assert_eq!(park.buffer, None);
        assert_eq!(park.on_expiry, OnExpiry::NotifyOnly);
        assert_eq!(park.service, None);

        let mount = config.services.get("mount").unwrap();
        assert_eq!(
            mount.base_url.as_deref(),
            Some("http://localhost:11112/api/v1")
        );
        assert_eq!(mount.device_number, 2);
        assert_eq!(
            mount.restart_command.as_deref(),
            Some("systemctl restart mount")
        );
        assert_eq!(
            mount.health_command.as_deref(),
            Some("systemctl is-active mount")
        );
        assert_eq!(mount.max_restart_duration, Duration::from_secs(45));

        let camera = config.services.get("camera").unwrap();
        assert_eq!(camera.device_number, 0, "device_number defaults to 0");
        assert_eq!(
            camera.restart_command, None,
            "restart_command defaults to null"
        );
        assert_eq!(
            camera.health_command, None,
            "health_command defaults to null"
        );
        assert_eq!(
            camera.max_restart_duration,
            Duration::from_secs(60),
            "max_restart_duration defaults to 60s"
        );
    }

    #[test]
    fn operation_watchdog_defaults() {
        let json = r#"{
            "operation_watchdog": { "rp_url": "http://rp:9000" }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.services.is_empty());
        let wd = config.operation_watchdog.unwrap();
        assert_eq!(wd.reconnect_max_attempts, 5);
        assert_eq!(wd.reconnect_backoff, Duration::from_secs(5));
        assert_eq!(wd.default_buffer, Duration::from_secs(10));
        assert!(wd.notifiers.is_empty());
        assert!(wd.operations.is_empty());
        assert!(wd.message_template.contains("{operation}"));
    }

    #[test]
    fn service_config_rejects_unknown_field() {
        let json = r#"{
            "services": { "mount": { "base_url": "http://x", "bogus": 1 } }
        }"#;
        let err = serde_json::from_str::<Config>(json).unwrap_err();
        assert!(err.to_string().contains("bogus"), "{err}");
    }

    #[test]
    fn service_config_base_url_is_optional() {
        // A restart-only entry (e.g. a non-Alpaca service like rp) needs no
        // Alpaca base URL; the ladder degrades and the restart endpoint never
        // uses it.
        let json = r#"{
            "services": { "rp": { "restart_command": "systemctl --user restart rp" } }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        let rp = config.services.get("rp").unwrap();
        assert_eq!(rp.base_url, None);
        assert_eq!(
            rp.restart_command.as_deref(),
            Some("systemctl --user restart rp")
        );
    }

    #[test]
    fn parse_service_health_full() {
        let json = r#"{
            "services": {
                "plate-solver": {
                    "restart_command": "systemctl restart plate-solver",
                    "health": {
                        "url": "http://localhost:11131/health",
                        "poll_interval": "10s",
                        "failure_threshold": 5,
                        "restart_backoff": "30s",
                        "restart_backoff_max": "5m"
                    }
                }
            }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        let health = config
            .services
            .get("plate-solver")
            .unwrap()
            .health
            .as_ref()
            .unwrap();
        assert_eq!(health.url, "http://localhost:11131/health");
        assert_eq!(health.poll_interval, Duration::from_secs(10));
        assert_eq!(health.failure_threshold, 5);
        assert_eq!(health.restart_backoff, Duration::from_secs(30));
        assert_eq!(health.restart_backoff_max, Duration::from_secs(300));
    }

    #[test]
    fn service_health_defaults() {
        let json = r#"{
            "services": { "svc": { "health": { "url": "http://x/health" } } }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        let health = config.services.get("svc").unwrap().health.as_ref().unwrap();
        assert_eq!(health.poll_interval, Duration::from_secs(30));
        assert_eq!(health.failure_threshold, 3);
        assert_eq!(health.restart_backoff, Duration::from_secs(60));
        assert_eq!(health.restart_backoff_max, Duration::from_secs(900));
    }

    #[test]
    fn service_health_absent_by_default() {
        let json = r#"{
            "services": { "svc": { "restart_command": "restart" } }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.services.get("svc").unwrap().health.is_none());
    }

    #[test]
    fn service_health_requires_url() {
        let json = r#"{
            "services": { "svc": { "health": { "poll_interval": "10s" } } }
        }"#;
        let err = serde_json::from_str::<Config>(json).unwrap_err();
        assert!(err.to_string().contains("url"), "{err}");
    }

    #[test]
    fn service_health_rejects_unknown_field() {
        let json = r#"{
            "services": { "svc": { "health": { "url": "http://x", "bogus": 1 } } }
        }"#;
        let err = serde_json::from_str::<Config>(json).unwrap_err();
        assert!(err.to_string().contains("bogus"), "{err}");
    }

    #[test]
    fn operation_watchdog_rejects_nested_services() {
        // The registry moved to the top level; a config still nesting it inside
        // the watchdog block must fail loudly, not be silently ignored.
        let json = r#"{
            "operation_watchdog": {
                "rp_url": "http://rp",
                "services": { "mount": { "base_url": "http://x" } }
            }
        }"#;
        let err = serde_json::from_str::<Config>(json).unwrap_err();
        assert!(err.to_string().contains("services"), "{err}");
    }

    #[test]
    fn operation_watchdog_requires_rp_url() {
        let json = r#"{ "operation_watchdog": { "reconnect_backoff": "5s" } }"#;
        let err = serde_json::from_str::<Config>(json).unwrap_err();
        assert!(
            err.to_string().contains("rp_url"),
            "missing rp_url should be reported: {err}"
        );
    }

    #[test]
    fn operation_watchdog_rejects_unknown_field() {
        let json = r#"{
            "operation_watchdog": { "rp_url": "http://rp", "bogus": true }
        }"#;
        let err = serde_json::from_str::<Config>(json).unwrap_err();
        assert!(
            err.to_string().contains("bogus"),
            "unknown field should be rejected: {err}"
        );
    }

    #[test]
    fn on_expiry_defaults_to_notify_only() {
        assert_eq!(OnExpiry::default(), OnExpiry::NotifyOnly);
    }

    #[test]
    fn config_rejects_unknown_top_level_field() {
        let json = r#"{ "webhooks": [] }"#;
        let err = serde_json::from_str::<Config>(json).unwrap_err();
        assert!(err.to_string().contains("webhooks"), "{err}");
    }

    #[test]
    fn monitor_config_rejects_unknown_field() {
        let json = r#"{
            "monitors": [{
                "type": "alpaca_safety_monitor",
                "name": "Test",
                "timeout": "5s"
            }]
        }"#;
        let err = serde_json::from_str::<Config>(json).unwrap_err();
        assert!(err.to_string().contains("timeout"), "{err}");
    }

    #[test]
    fn notifier_config_rejects_unknown_field() {
        let json = r#"{
            "notifiers": [{
                "type": "pushover",
                "webhook_url": "http://example.invalid"
            }]
        }"#;
        let err = serde_json::from_str::<Config>(json).unwrap_err();
        assert!(err.to_string().contains("webhook_url"), "{err}");
    }

    #[test]
    fn transition_config_rejects_unknown_field() {
        let json = r#"{
            "transitions": [{
                "monitor_name": "Test",
                "direction": "both",
                "notifiers": ["pushover"],
                "cooldown": "1m"
            }]
        }"#;
        let err = serde_json::from_str::<Config>(json).unwrap_err();
        assert!(err.to_string().contains("cooldown"), "{err}");
    }

    #[test]
    fn dashboard_config_rejects_unknown_field() {
        let json = r#"{ "dashboard": { "theme": "dark" } }"#;
        let err = serde_json::from_str::<Config>(json).unwrap_err();
        assert!(err.to_string().contains("theme"), "{err}");
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
