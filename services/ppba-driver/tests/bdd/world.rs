//! World struct for PPBA Driver BDD tests

use std::time::Duration;

use cucumber::World;

use crate::steps::infrastructure::{alpaca_error_number, alpaca_get, is_alpaca_error, PpbaHandle};

#[derive(Debug, Default, World)]
pub struct PpbaWorld {
    /// Handle to the running ppba-driver process
    pub ppba: Option<PpbaHandle>,

    /// Base URL of the running server (e.g. "http://127.0.0.1:12345")
    pub base_url: Option<String>,

    /// Config JSON built up during Given steps, written to temp file before start
    pub config: serde_json::Value,

    /// ASCOM error number from the last operation (0 = no error)
    pub last_error_number: Option<i64>,

    /// ASCOM error message from the last operation
    pub last_error_message: Option<String>,
}

impl PpbaWorld {
    /// Start the ppba-driver binary with the current config.
    /// Writes config to a temp file and spawns the process.
    pub async fn start_ppba(&mut self) {
        let config_path = std::env::temp_dir().join(format!(
            "ppba-bdd-config-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        tokio::fs::write(
            &config_path,
            serde_json::to_string_pretty(&self.config).unwrap(),
        )
        .await
        .expect("failed to write test config");

        let handle = PpbaHandle::start(config_path.to_str().unwrap()).await;
        self.base_url = Some(handle.base_url.clone());
        self.ppba = Some(handle);

        // Wait for the server to be ready
        self.wait_for_ready().await;
    }

    /// Return the base URL for the switch device endpoints.
    pub fn switch_url(&self) -> String {
        format!(
            "{}/api/v1/switch/0",
            self.base_url.as_ref().expect("server not started")
        )
    }

    /// Return the base URL for the OC device endpoints.
    pub fn oc_url(&self) -> String {
        format!(
            "{}/api/v1/observingconditions/0",
            self.base_url.as_ref().expect("server not started")
        )
    }

    /// Poll until the server is ready to accept requests.
    /// Tries device endpoints and management endpoint until HTTP 200.
    async fn wait_for_ready(&self) {
        let base = self.base_url.as_ref().expect("server not started");
        let client = reqwest::Client::new();

        // Try device endpoints and management endpoint (always available)
        let urls = [
            format!("{}/api/v1/switch/0/name", base),
            format!("{}/api/v1/observingconditions/0/name", base),
            format!("{}/management/apiversions", base),
        ];

        for _ in 0..120 {
            for url in &urls {
                if let Ok(resp) = client.get(url).send().await {
                    if resp.status().is_success() {
                        return;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        panic!("ppba-driver did not become ready within 30 seconds");
    }

    /// Poll until switch data is available (the status cache has been populated).
    /// Checks GET getswitchvalue?Id=10 (input voltage) until it returns a non-error value.
    pub async fn wait_for_switch_data(&self) {
        let switch_url = self.switch_url();
        for _ in 0..120 {
            let resp = alpaca_get(&switch_url, "getswitchvalue?Id=10").await;
            if !is_alpaca_error(&resp) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        panic!("switch data did not become available within 30 seconds");
    }

    /// Poll until OC data is available (temperature returns a non-error value).
    pub async fn wait_for_oc_data(&self) {
        let oc_url = self.oc_url();
        for _ in 0..120 {
            let resp = alpaca_get(&oc_url, "temperature").await;
            if !is_alpaca_error(&resp) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        panic!("OC data did not become available within 30 seconds");
    }

    /// Capture an ASCOM Alpaca response: if it has an error, store error info;
    /// otherwise clear the error state.
    pub fn capture_response(&mut self, resp: &serde_json::Value) {
        let err = alpaca_error_number(resp);
        if err != 0 {
            self.last_error_number = Some(err);
            self.last_error_message = Some(resp["ErrorMessage"].as_str().unwrap_or("").to_string());
        } else {
            self.last_error_number = None;
            self.last_error_message = None;
        }
    }
}
