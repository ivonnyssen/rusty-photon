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

    // TLS + auth test state (shared PKI fixture: CA, service cert, credentials)
    pub pki: Option<bdd_infra::tls_auth::PkiFixture>,

    // Config actions test state
    pub last_response: Option<Value>,
    pub last_supported_actions: Option<Vec<String>>,
    pub last_ascom_error: Option<ascom_alpaca::ASCOMError>,

    /// Doctor-subcommand smoke state (staged config file + run output)
    pub doctor_smoke: bdd_infra::doctor_smoke::DoctorSmokeState,
}

impl bdd_infra::doctor_smoke::DoctorSmokeWorld for FilemonitorWorld {
    fn doctor_smoke(&mut self) -> &mut bdd_infra::doctor_smoke::DoctorSmokeState {
        &mut self.doctor_smoke
    }

    fn valid_config(&self) -> Value {
        // The suite's own config helper — the same shape every scenario's
        // start path parses. Doctor only parses, so the placeholder
        // monitored-file path is fine.
        self.build_config_json()
    }
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

    /// The shared PKI fixture (panics if the cert-generation Given hasn't run).
    pub fn pki(&self) -> &bdd_infra::tls_auth::PkiFixture {
        self.pki.as_ref().expect("TLS certs not generated")
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
                "polling_interval": format!("{polling_interval}s"),
            },
            "parsing": {
                "rules": rules,
                "case_sensitive": self.case_sensitive,
            },
            "server": {
                "port": 0,
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

        let handle =
            ServiceHandle::start(env!("CARGO_PKG_NAME"), config_path.to_str().unwrap()).await;
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

        let handle =
            ServiceHandle::start(env!("CARGO_PKG_NAME"), config_path.to_str().unwrap()).await;
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

    /// The OS-assigned port the spawned service bound.
    pub fn bound_port(&self) -> u16 {
        self.filemonitor.as_ref().expect("service not started").port
    }

    /// Call `config.get`, stash the parsed response, and return the `config`
    /// object (so a When step can edit a field and re-`config.apply` it).
    pub async fn current_config(&mut self) -> Value {
        let monitor = Arc::clone(self.monitor());
        let body = monitor
            .action("config.get".to_string(), String::new())
            .await
            .expect("config.get failed");
        let parsed: Value = serde_json::from_str(&body).expect("config.get returned invalid JSON");
        let config = parsed
            .get("config")
            .cloned()
            .expect("config.get response missing `config`");
        self.last_response = Some(parsed);
        config
    }

    /// Call `config.get` and stash the parsed response.
    pub async fn call_config_get(&mut self) {
        self.current_config().await;
    }

    /// Call `config.schema` and stash the parsed response.
    pub async fn call_config_schema(&mut self) {
        let monitor = Arc::clone(self.monitor());
        let body = monitor
            .action("config.schema".to_string(), String::new())
            .await
            .expect("config.schema failed");
        self.last_response =
            Some(serde_json::from_str(&body).expect("config.schema returned invalid JSON"));
    }

    /// Call `config.apply` with `params` and stash the parsed response.
    pub async fn call_config_apply(&mut self, params: Value) {
        let monitor = Arc::clone(self.monitor());
        let body = monitor
            .action("config.apply".to_string(), params.to_string())
            .await
            .expect("config.apply failed");
        self.last_response =
            Some(serde_json::from_str(&body).expect("config.apply returned invalid JSON"));
    }

    /// Poll `config.get` on a fresh client until `file.polling_interval` equals
    /// `expected`, panicking after ~20 s. A fresh client is used each attempt so
    /// a connection dropped by the reload doesn't wedge the poll, and the loop
    /// tolerates the brief blip while the server tears down and rebinds.
    pub async fn wait_for_config_polling_interval(&self, expected: &str) {
        let addr = SocketAddr::from(([127, 0, 0, 1], self.bound_port()));
        for _ in 0..80 {
            if try_get_polling_interval(addr).await.as_deref() == Some(expected) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        panic!("reloaded service did not report polling_interval {expected} within 20s");
    }
}

/// Read `file.polling_interval` via a fresh client, returning `None` on any
/// transport/parse failure (e.g. mid-reload).
async fn try_get_polling_interval(addr: SocketAddr) -> Option<String> {
    let client = AlpacaClient::new_from_addr(addr);
    let mut devices = client.get_devices().await.ok()?;
    if let Some(TypedDevice::SafetyMonitor(monitor)) = devices.next() {
        let body = monitor
            .action("config.get".to_string(), String::new())
            .await
            .ok()?;
        let parsed: Value = serde_json::from_str(&body).ok()?;
        return parsed["config"]["file"]["polling_interval"]
            .as_str()
            .map(str::to_string);
    }
    None
}
