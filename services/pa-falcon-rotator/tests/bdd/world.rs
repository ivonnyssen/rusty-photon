//! World struct for pa-falcon-rotator BDD tests
//!
//! Drives the service binary via Alpaca HTTP, using `MockSerialPortFactory`
//! for stable responses. Every Phase 2 scenario is tagged `@wip` so the
//! World only needs to compile, not actually function end-to-end.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::{Rotator, Switch, TypedDevice};
use ascom_alpaca::Client as AlpacaClient;
use bdd_infra::ServiceHandle;
use cucumber::World;
use pa_falcon_rotator::Config;
use tempfile::TempDir;

// Several fields below are wired up by Phase 3 step bodies; Phase 2 only
// instantiates them so the `World` derives compile.
#[allow(dead_code)]
#[derive(Debug, Default, World)]
pub struct FalconRotatorWorld {
    pub service_handle: Option<ServiceHandle>,
    pub rotator: Option<Arc<dyn Rotator>>,
    pub status_switch: Option<Arc<dyn Switch>>,
    pub config: Option<Config>,
    pub temp_dir: Option<TempDir>,
    pub last_error: Option<String>,
    pub last_error_code: Option<u16>,

    /// Captured property reads.
    pub position_result: Option<f64>,
    pub mechanical_position_result: Option<f64>,
    pub target_position_result: Option<f64>,
    pub is_moving_result: Option<bool>,
    pub reverse_result: Option<bool>,
    pub step_size_result: Option<f64>,

    /// Captured switch reads.
    pub switch_value_result: Option<f64>,
    pub switch_bool_result: Option<bool>,
    pub max_switch_result: Option<usize>,
}

// Methods below are wired up by Phase 3 step bodies; Phase 2 only checks
// they compile.
#[allow(dead_code)]
impl FalconRotatorWorld {
    /// Convenience accessor for the Rotator client.
    pub fn rotator(&self) -> &Arc<dyn Rotator> {
        self.rotator.as_ref().expect("rotator not acquired")
    }

    /// Convenience accessor for the Status Switch client.
    pub fn status_switch(&self) -> &Arc<dyn Switch> {
        self.status_switch
            .as_ref()
            .expect("status switch not acquired")
    }

    /// Build a JSON config from current world state, write to temp file, and return path.
    fn write_config(&mut self) -> String {
        let config = self.config.clone().unwrap_or_default();

        let config_json = serde_json::json!({
            "serial": {
                "port": config.serial.port,
                "baud_rate": config.serial.baud_rate,
                "timeout": format!("{}ms", config.serial.timeout.as_millis()),
            },
            "server": {
                "port": 0,
                "discovery_port": null,
            },
            "rotator": {
                "name": config.rotator.name,
                "unique_id": config.rotator.unique_id,
                "description": config.rotator.description,
                "enabled": config.rotator.enabled,
            },
            "switch": {
                "name": config.switch.name,
                "unique_id": config.switch.unique_id,
                "description": config.switch.description,
                "enabled": config.switch.enabled,
            },
        });

        let dir = self
            .temp_dir
            .get_or_insert_with(|| TempDir::new().expect("failed to create temp dir"));
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, config_json.to_string()).expect("failed to write config");
        config_path.to_str().unwrap().to_string()
    }

    /// Start the service binary and acquire both device clients.
    pub async fn start_service(&mut self) {
        let config_path = self.write_config();

        let handle = ServiceHandle::start(env!("CARGO_PKG_NAME"), &config_path).await;
        let (rotator, status_switch) = self.acquire_devices(&handle).await;
        self.rotator = Some(rotator);
        self.status_switch = Some(status_switch);
        self.service_handle = Some(handle);
    }

    /// Poll until the server returns both a Rotator and a Switch device.
    async fn acquire_devices(&self, handle: &ServiceHandle) -> (Arc<dyn Rotator>, Arc<dyn Switch>) {
        let addr = SocketAddr::from(([127, 0, 0, 1], handle.port));
        let client = AlpacaClient::new_from_addr(addr);
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if let Ok(devices) = client.get_devices().await {
                let mut rotator = None;
                let mut status_switch = None;
                for device in devices {
                    #[allow(unreachable_patterns)]
                    match device {
                        TypedDevice::Rotator(r) => rotator = Some(r),
                        TypedDevice::Switch(s) => status_switch = Some(s),
                        _ => {}
                    }
                }
                if let (Some(r), Some(s)) = (rotator, status_switch) {
                    return (r, s);
                }
            }
        }
        panic!("pa-falcon-rotator did not become healthy within 30 seconds");
    }
}
