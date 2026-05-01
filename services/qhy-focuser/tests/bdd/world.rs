//! World struct for QHY-Focuser BDD tests
//!
//! Uses binary spawning via ServiceHandle and HTTP interaction via AlpacaClient.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::{Focuser, TypedDevice};
use ascom_alpaca::Client as AlpacaClient;
use bdd_infra::ServiceHandle;
use cucumber::World;
use qhy_focuser::Config;
use tempfile::TempDir;

#[derive(Debug, Default, World)]
pub struct QhyFocuserWorld {
    pub focuser_handle: Option<ServiceHandle>,
    pub focuser: Option<Arc<dyn Focuser>>,
    pub config: Option<Config>,
    pub temp_dir: Option<TempDir>,
    pub last_error: Option<String>,
    pub last_error_code: Option<u16>,
    pub position_result: Option<i32>,
    pub temperature_result: Option<f64>,
    pub is_moving_result: Option<bool>,

    /// TLS test state
    pub tls_pki_dir: Option<TempDir>,

    /// Auth test state — plaintext password for HTTP Basic Auth assertions
    pub auth_password: Option<String>,
}

impl QhyFocuserWorld {
    /// Convenience accessor for the Focuser device client.
    pub fn focuser(&self) -> &Arc<dyn Focuser> {
        self.focuser.as_ref().expect("focuser not acquired")
    }

    /// Build a JSON config from current world state, write to temp file, and return path.
    fn write_config(&mut self) -> String {
        let config = self.config.clone().unwrap_or_default();

        let config_json = serde_json::json!({
            "serial": {
                "port": config.serial.port,
                "baud_rate": config.serial.baud_rate,
                "polling_interval": format!("{}ms", config.serial.polling_interval.as_millis()),
                "timeout": format!("{}ms", config.serial.timeout.as_millis()),
            },
            "server": {
                "port": 0,
                "discovery_port": null,
            },
            "focuser": {
                "name": config.focuser.name,
                "unique_id": config.focuser.unique_id,
                "description": config.focuser.description,
                "enabled": config.focuser.enabled,
                "max_step": config.focuser.max_step,
                "speed": config.focuser.speed,
                "reverse": config.focuser.reverse,
            },
        });

        let dir = self
            .temp_dir
            .get_or_insert_with(|| TempDir::new().expect("failed to create temp dir"));
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, config_json.to_string()).expect("failed to write config");
        config_path.to_str().unwrap().to_string()
    }

    /// Start the focuser service binary and acquire a Focuser client.
    pub async fn start_focuser(&mut self) {
        let config_path = self.write_config();

        let handle = ServiceHandle::start(env!("CARGO_PKG_NAME"), &config_path).await;
        let focuser = self.acquire_focuser(&handle).await;
        self.focuser = Some(focuser);
        self.focuser_handle = Some(handle);
    }

    /// Poll until the server returns a Focuser device via the Alpaca client.
    async fn acquire_focuser(&self, handle: &ServiceHandle) -> Arc<dyn Focuser> {
        let addr = SocketAddr::from(([127, 0, 0, 1], handle.port));
        let client = AlpacaClient::new_from_addr(addr);
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if let Ok(mut devices) = client.get_devices().await {
                if let Some(TypedDevice::Focuser(focuser)) = devices.next() {
                    return focuser;
                }
            }
        }
        panic!("qhy-focuser did not become healthy within 30 seconds");
    }
}
