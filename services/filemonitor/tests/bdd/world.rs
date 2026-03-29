use ascom_alpaca::api::{SafetyMonitor, TypedDevice};
use ascom_alpaca::Client as AlpacaClient;
use cucumber::World;
use serde_json::Value;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

use crate::steps::infrastructure::ServiceHandle;

/// Serializable rule config (no filemonitor lib imports).
#[derive(Debug, Clone)]
pub struct ParsingRuleConfig {
    pub rule_type: String,
    pub pattern: String,
    pub safe: bool,
}

#[derive(Debug, Default, World)]
pub struct FilemonitorWorld {
    // Process handle
    pub filemonitor: Option<ServiceHandle>,
    pub monitor: Option<Arc<dyn SafetyMonitor>>,

    // Config building
    pub rules: Vec<ParsingRuleConfig>,
    pub case_sensitive: bool,
    pub polling_interval: u64,

    // Temp file management
    pub temp_dir: Option<TempDir>,
    pub temp_file_path: Option<PathBuf>,

    // Result capture
    pub safety_result: Option<bool>,
    pub last_error: Option<String>,

    // Config validation (for configuration.feature)
    pub loaded_config: Option<Value>,
    pub config_path: Option<String>,

    // TLS test state
    pub tls_pki_dir: Option<TempDir>,

    // Auth test state
    pub auth_password: Option<String>,
}

impl FilemonitorWorld {
    pub fn create_temp_file(&mut self, content: &str) -> PathBuf {
        let dir = self
            .temp_dir
            .get_or_insert_with(|| TempDir::new().expect("failed to create temp dir"));
        let path = dir.path().join("monitored.txt");
        std::fs::write(&path, content).expect("failed to write temp file");
        self.temp_file_path = Some(path.clone());
        path
    }

    /// Convenience accessor for the typed SafetyMonitor device.
    pub fn monitor(&self) -> &Arc<dyn SafetyMonitor> {
        self.monitor.as_ref().expect("monitor not acquired")
    }

    /// Build a JSON config from the accumulated world state.
    pub fn build_config_json(&self) -> Value {
        let file_path = self
            .temp_file_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "nonexistent.txt".to_string());

        let rules: Vec<Value> = self
            .rules
            .iter()
            .map(|r| {
                serde_json::json!({
                    "type": r.rule_type,
                    "pattern": r.pattern,
                    "safe": r.safe,
                })
            })
            .collect();

        let polling_interval = if self.polling_interval > 0 {
            self.polling_interval
        } else {
            60
        };

        serde_json::json!({
            "device": {
                "name": "Test",
                "unique_id": "test-001",
                "description": "Test device",
            },
            "file": {
                "path": file_path,
                "polling_interval_seconds": polling_interval,
            },
            "parsing": {
                "rules": rules,
                "case_sensitive": self.case_sensitive,
            },
            "server": {
                "port": 0,
                "device_number": 0,
                "discovery_port": null,
            },
        })
    }

    /// Write config to temp dir, start the binary, acquire typed client.
    pub async fn start_filemonitor(&mut self) {
        let config_json = self.build_config_json();
        let dir = self
            .temp_dir
            .get_or_insert_with(|| TempDir::new().expect("failed to create temp dir"));
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, config_json.to_string()).expect("failed to write config");

        let handle = ServiceHandle::start(
            env!("CARGO_MANIFEST_DIR"),
            env!("CARGO_PKG_NAME"),
            config_path.to_str().unwrap(),
        )
        .await;
        let monitor = self.acquire_monitor(&handle).await;
        self.monitor = Some(monitor);
        self.filemonitor = Some(handle);
    }

    /// Start filemonitor from an external config file (modifying port to 0).
    pub async fn start_filemonitor_with_config(&mut self, path: &str) {
        let content = std::fs::read_to_string(path).expect("failed to read config file");
        let mut config: Value =
            serde_json::from_str(&content).expect("failed to parse config file");
        config["server"]["port"] = serde_json::json!(0);
        config["server"]["discovery_port"] = serde_json::json!(null);

        let dir = self
            .temp_dir
            .get_or_insert_with(|| TempDir::new().expect("failed to create temp dir"));
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, config.to_string()).expect("failed to write config");

        let handle = ServiceHandle::start(
            env!("CARGO_MANIFEST_DIR"),
            env!("CARGO_PKG_NAME"),
            config_path.to_str().unwrap(),
        )
        .await;
        let monitor = self.acquire_monitor(&handle).await;
        self.monitor = Some(monitor);
        self.filemonitor = Some(handle);
    }

    /// Poll until the server returns a SafetyMonitor device via the typed client.
    pub async fn acquire_monitor(&self, handle: &ServiceHandle) -> Arc<dyn SafetyMonitor> {
        let addr = SocketAddr::from(([127, 0, 0, 1], handle.port));
        let client = AlpacaClient::new_from_addr(addr);
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if let Ok(mut devices) = client.get_devices().await {
                if let Some(TypedDevice::SafetyMonitor(monitor)) = devices.next() {
                    return monitor;
                }
            }
        }
        panic!("filemonitor did not become healthy within 30 seconds");
    }
}
