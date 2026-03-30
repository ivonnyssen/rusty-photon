//! World struct for PPBA Driver BDD tests

use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::{ObservingConditions, Switch, TypedDevice};
use ascom_alpaca::{ASCOMError, ASCOMResult, Client};
use cucumber::World;

use crate::steps::infrastructure::ServiceHandle;

#[derive(Debug, Default, World)]
pub struct PpbaWorld {
    /// Handle to the running ppba-driver process
    pub ppba: Option<ServiceHandle>,

    /// Base URL of the running server (e.g. "http://127.0.0.1:12345")
    pub base_url: Option<String>,

    /// Config JSON built up during Given steps, written to temp file before start
    pub config: serde_json::Value,

    /// Typed ASCOM Switch device client
    pub switch: Option<Arc<dyn Switch>>,

    /// Typed ASCOM ObservingConditions device client
    pub oc: Option<Arc<dyn ObservingConditions>>,

    /// ASCOM error from the last "try" operation
    pub last_error: Option<ASCOMError>,

    /// TLS test state
    pub tls_pki_dir: Option<tempfile::TempDir>,

    /// Auth test password (plaintext, for building requests)
    pub auth_password: Option<String>,
}

impl PpbaWorld {
    /// Start the ppba-driver binary with the current config.
    /// Writes config to a temp file, spawns the process, waits for ready,
    /// then discovers devices via the typed ASCOM client.
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

        let handle = ServiceHandle::start(
            env!("CARGO_MANIFEST_DIR"),
            env!("CARGO_PKG_NAME"),
            config_path.to_str().unwrap(),
        )
        .await;
        self.base_url = Some(handle.base_url.clone());
        self.ppba = Some(handle);

        // Wait for the server to be ready
        self.wait_for_ready().await;

        // Discover devices via typed ASCOM client.
        // Creates a fresh Client on each attempt because the random ClientID
        // may exceed i32::MAX, which the server rejects with 400 (it parses
        // integers as i32 per ASCOM spec). Retrying gives a fresh random ID.
        let base_url = self.base_url.as_ref().unwrap();
        for attempt in 0..20 {
            let client = Client::new(base_url).unwrap();
            match client.get_devices().await {
                Ok(devices) => {
                    for device in devices {
                        #[allow(unreachable_patterns)]
                        match device {
                            TypedDevice::Switch(s) => self.switch = Some(s),
                            TypedDevice::ObservingConditions(oc) => self.oc = Some(oc),
                            _ => {}
                        }
                    }
                    return;
                }
                Err(_) if attempt < 19 => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(e) => panic!("Failed to discover devices after 20 attempts: {e}"),
            }
        }
    }

    /// Get a reference to the typed Switch device.
    pub fn switch_ref(&self) -> &dyn Switch {
        self.switch
            .as_ref()
            .expect("switch device not discovered")
            .as_ref()
    }

    /// Get a reference to the typed ObservingConditions device.
    pub fn oc_ref(&self) -> &dyn ObservingConditions {
        self.oc.as_ref().expect("OC device not discovered").as_ref()
    }

    /// Capture an ASCOM result: store the error on Err, clear on Ok.
    pub fn capture_result<T>(&mut self, result: ASCOMResult<T>) {
        match result {
            Ok(_) => self.last_error = None,
            Err(e) => self.last_error = Some(e),
        }
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
    pub async fn wait_for_switch_data(&self) {
        let switch = self.switch.as_ref().expect("switch device not discovered");
        for _ in 0..120 {
            if switch.get_switch_value(10).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        panic!("switch data did not become available within 30 seconds");
    }

    /// Poll until OC data is available (temperature returns a non-error value).
    pub async fn wait_for_oc_data(&self) {
        let oc = self.oc.as_ref().expect("OC device not discovered");
        for _ in 0..120 {
            if oc.temperature().await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        panic!("OC data did not become available within 30 seconds");
    }
}
