use cucumber::World;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;

use crate::steps::infrastructure::FilemonitorHandle;

/// Serializable rule config (no filemonitor lib imports).
#[derive(Debug, Clone)]
pub struct ParsingRuleConfig {
    pub rule_type: String,
    pub pattern: String,
    pub safe: bool,
}

#[derive(Debug, Default, World)]
pub struct FilemonitorWorld {
    // Process handle (replaces Arc<FileMonitorDevice>)
    pub filemonitor: Option<FilemonitorHandle>,
    pub client: Option<reqwest::Client>,

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
                "discovery_port": 0,
            },
        })
    }

    /// Write config to temp dir, start the binary, wait for healthy.
    pub async fn start_filemonitor(&mut self) {
        let config_json = self.build_config_json();
        let dir = self
            .temp_dir
            .get_or_insert_with(|| TempDir::new().expect("failed to create temp dir"));
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, config_json.to_string()).expect("failed to write config");

        let handle = FilemonitorHandle::start(config_path.to_str().unwrap()).await;
        self.wait_for_healthy(&handle).await;
        self.filemonitor = Some(handle);
        self.client = Some(reqwest::Client::new());
    }

    /// Start filemonitor from an external config file (modifying port to 0).
    pub async fn start_filemonitor_with_config(&mut self, path: &str) {
        let content = std::fs::read_to_string(path).expect("failed to read config file");
        let mut config: Value =
            serde_json::from_str(&content).expect("failed to parse config file");
        config["server"]["port"] = serde_json::json!(0);
        config["server"]["discovery_port"] = serde_json::json!(0);

        let dir = self
            .temp_dir
            .get_or_insert_with(|| TempDir::new().expect("failed to create temp dir"));
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, config.to_string()).expect("failed to write config");

        let handle = FilemonitorHandle::start(config_path.to_str().unwrap()).await;
        self.wait_for_healthy(&handle).await;
        self.filemonitor = Some(handle);
        self.client = Some(reqwest::Client::new());
    }

    async fn wait_for_healthy(&self, handle: &FilemonitorHandle) {
        let client = reqwest::Client::new();
        let url = format!("{}/api/v1/safetymonitor/0/connected", handle.base_url);
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if let Ok(resp) = client.get(&url).send().await {
                if resp.status().is_success() {
                    return;
                }
            }
        }
        panic!("filemonitor did not become healthy within 30 seconds");
    }

    pub fn alpaca_url(&self, method: &str) -> String {
        let fm = self.filemonitor.as_ref().expect("filemonitor not started");
        format!("{}/api/v1/safetymonitor/0/{}", fm.base_url, method)
    }

    pub async fn alpaca_get(&self, method: &str) -> Value {
        let client = self.client.as_ref().expect("client not created");
        let url = self.alpaca_url(method);
        let resp = client.get(&url).send().await.expect("HTTP GET failed");
        resp.json::<Value>()
            .await
            .expect("failed to parse response")
    }

    pub async fn alpaca_put_connected(&self, connected: bool) -> Result<(), String> {
        let client = self.client.as_ref().expect("client not created");
        let url = self.alpaca_url("connected");
        let resp = client
            .put(&url)
            .form(&[
                ("Connected", if connected { "true" } else { "false" }),
                ("ClientID", "1"),
                ("ClientTransactionID", "1"),
            ])
            .send()
            .await
            .expect("HTTP PUT failed");
        let json: Value = resp.json().await.expect("failed to parse response");
        let error_number = json["ErrorNumber"].as_i64().unwrap_or(0);
        if error_number != 0 {
            let error_message = json["ErrorMessage"].as_str().unwrap_or("").to_string();
            Err(format!("Error {}: {}", error_number, error_message))
        } else {
            Ok(())
        }
    }

    pub async fn alpaca_get_issafe(&self) -> Result<bool, (i32, String)> {
        let json = self.alpaca_get("issafe").await;
        let error_number = json["ErrorNumber"].as_i64().unwrap_or(0) as i32;
        if error_number != 0 {
            let error_message = json["ErrorMessage"].as_str().unwrap_or("").to_string();
            Err((error_number, error_message))
        } else {
            Ok(json["Value"].as_bool().unwrap_or(false))
        }
    }
}
