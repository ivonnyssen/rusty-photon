//! Cucumber `World` for the dsd-fp2 BDD suite.
//!
//! Spawns the dsd-fp2 binary via [`bdd_infra::ServiceHandle`] and drives
//! it through the typed ASCOM Alpaca `CoverCalibrator` client. Scenarios
//! that need a particular precondition (e.g. cover-open before testing
//! close) prime it through the client itself — there is no in-process
//! handle to the `MockState` simulator.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::cover_calibrator::{CalibratorStatus, CoverStatus};
use ascom_alpaca::api::{CoverCalibrator, TypedDevice};
use ascom_alpaca::{ASCOMError, Client as AlpacaClient};
use bdd_infra::ServiceHandle;
use cucumber::World;
use tempfile::TempDir;

#[derive(Debug, Default, World)]
pub struct Fp2World {
    pub handle: Option<ServiceHandle>,
    pub device: Option<Arc<dyn CoverCalibrator>>,
    pub temp_dir: Option<TempDir>,
    /// Stashed result of the last fallible call so a subsequent Then step
    /// can assert against it.
    pub last_error: Option<ASCOMError>,
}

impl Fp2World {
    /// Write a JSON config for the spawned binary. Uses `/dev/mock` (the
    /// mock factory ignores the path), port 0 for OS-assigned, and a
    /// 100 ms polling interval so wait-for loops converge quickly.
    fn write_config(&mut self) -> String {
        let config = serde_json::json!({
            "serial": {
                "port": "/dev/mock",
                "baud_rate": 115200,
                "polling_interval": "100ms",
                "timeout": "2s"
            },
            "server": {
                "port": 0,
                "discovery_port": null
            },
            "cover_calibrator": {
                "name": "Deep Sky Dad FP2",
                "unique_id": "dsd-fp2-bdd",
                "description": "BDD test instance",
                "enabled": true,
                "max_brightness": 4096
            }
        });

        let dir = self
            .temp_dir
            .get_or_insert_with(|| TempDir::new().expect("failed to create temp dir"));
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, config.to_string()).expect("failed to write config");
        config_path.to_str().unwrap().to_string()
    }

    /// Spawn the dsd-fp2 binary and acquire a `CoverCalibrator` client.
    pub async fn start(&mut self) {
        let config_path = self.write_config();
        let handle = ServiceHandle::start(env!("CARGO_PKG_NAME"), &config_path).await;
        let device = acquire_device(&handle).await;
        self.device = Some(device);
        self.handle = Some(handle);
    }

    pub fn device(&self) -> &Arc<dyn CoverCalibrator> {
        self.device.as_ref().expect("device not acquired")
    }

    /// Poll the device until `cover_state` matches `expected`, panicking
    /// after 5 s. Necessary because `open_cover` / `close_cover` return
    /// before the while-open poll task observes the move completing.
    pub async fn wait_for_cover_state(&self, expected: CoverStatus) {
        for _ in 0..50 {
            if self.device().cover_state().await.unwrap() == expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!(
            "cover_state did not reach {expected:?} within 5s (current: {:?})",
            self.device().cover_state().await.unwrap()
        );
    }

    /// Poll the device until `calibrator_state` matches `expected`,
    /// panicking after 5 s.
    pub async fn wait_for_calibrator_state(&self, expected: CalibratorStatus) {
        for _ in 0..50 {
            if self.device().calibrator_state().await.unwrap() == expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!(
            "calibrator_state did not reach {expected:?} within 5s (current: {:?})",
            self.device().calibrator_state().await.unwrap()
        );
    }
}

/// Poll the Alpaca management endpoint until a `CoverCalibrator` device is
/// advertised. The freshly-spawned server may take a few hundred ms to
/// finish binding and registering the device.
async fn acquire_device(handle: &ServiceHandle) -> Arc<dyn CoverCalibrator> {
    let addr = SocketAddr::from(([127, 0, 0, 1], handle.port));
    let client = AlpacaClient::new_from_addr(addr);
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Ok(mut devices) = client.get_devices().await {
            if let Some(TypedDevice::CoverCalibrator(cc)) = devices.next() {
                return cc;
            }
        }
    }
    panic!("dsd-fp2 did not become healthy within 30 seconds");
}

pub fn cover_status_from_str(s: &str) -> CoverStatus {
    match s {
        "NotPresent" => CoverStatus::NotPresent,
        "Closed" => CoverStatus::Closed,
        "Moving" => CoverStatus::Moving,
        "Open" => CoverStatus::Open,
        "Unknown" => CoverStatus::Unknown,
        "Error" => CoverStatus::Error,
        other => panic!("unknown CoverStatus name: {other:?}"),
    }
}

pub fn calibrator_status_from_str(s: &str) -> CalibratorStatus {
    match s {
        "NotPresent" => CalibratorStatus::NotPresent,
        "Off" => CalibratorStatus::Off,
        "NotReady" => CalibratorStatus::NotReady,
        "Ready" => CalibratorStatus::Ready,
        "Unknown" => CalibratorStatus::Unknown,
        "Error" => CalibratorStatus::Error,
        other => panic!("unknown CalibratorStatus name: {other:?}"),
    }
}
