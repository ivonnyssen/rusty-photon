//! Cucumber `World` for the dsd-fp2 BDD suite.
//!
//! The world owns one `DsdFp2Device` plus the `MockTransportFactory`'s
//! `MockState` handle so steps can both drive the device and inspect the
//! simulator. Scenarios run in-process — no subprocess, no
//! `bdd_infra::ServiceHandle`.

use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::cover_calibrator::{CalibratorStatus, CoverStatus};
use ascom_alpaca::ASCOMError;
use cucumber::World;
use dsd_fp2::{
    Config, CoverCalibratorConfig, DsdFp2Device, FlatPanelManager, MockState, MockTransportFactory,
    SerialConfig, ServerConfig,
};

#[derive(Debug, Default, World)]
pub struct Fp2World {
    pub factory: Option<MockTransportFactory>,
    pub manager: Option<Arc<FlatPanelManager>>,
    pub device: Option<Arc<DsdFp2Device>>,
    /// Stashed result of the last fallible call (`open_cover`, `calibrator_on`, …)
    /// so subsequent Then steps can assert against it.
    pub last_error: Option<ASCOMError>,
}

impl Fp2World {
    /// Build the device with a fresh mock factory + manager.
    pub fn build_with(&mut self, state: MockState) {
        let factory = MockTransportFactory::with_state(state);
        let config = Config {
            serial: SerialConfig {
                port: "/dev/mock".to_string(),
                // Long polling interval keeps the poll task from racing
                // assertions; tests trigger refreshes manually.
                polling_interval: Duration::from_secs(3600),
                ..Default::default()
            },
            server: ServerConfig {
                port: 0,
                discovery_port: None,
                tls: None,
                auth: None,
            },
            cover_calibrator: CoverCalibratorConfig::default(),
        };
        let manager = FlatPanelManager::new(config, Arc::new(factory.clone()));
        let device = Arc::new(DsdFp2Device::new(
            CoverCalibratorConfig::default(),
            manager.clone(),
        ));
        self.factory = Some(factory);
        self.manager = Some(manager);
        self.device = Some(device);
        self.last_error = None;
    }

    /// Construct with a default simulator (closed cover, light off).
    pub fn build(&mut self) {
        self.build_with(MockState::default());
    }

    pub fn device(&self) -> &DsdFp2Device {
        self.device
            .as_ref()
            .expect("world.device not built")
            .as_ref()
    }

    pub fn factory(&self) -> &MockTransportFactory {
        self.factory.as_ref().expect("world.factory not built")
    }

    pub fn manager(&self) -> &Arc<FlatPanelManager> {
        self.manager.as_ref().expect("world.manager not built")
    }

    /// Drive one poll cycle synchronously, mirroring what the while-open
    /// task would do, so scenarios that depend on cached state can `Then`
    /// directly without sleeping.
    ///
    /// This **must** update every field that `device::derive_cover_state`
    /// and `derive_calibrator_state` read — without it scenarios pass
    /// only when tokio's `interval(d)` happens to fire its immediate
    /// first tick between this call and the assertion (a race that holds
    /// inconsistently across platforms; see PR #283 review).
    pub async fn refresh_cache(&self) {
        let snap = self.manager().snapshot();
        let factory = self.factory();
        let state = factory.state();
        let motor_running = state.motor_running().await;
        let cover_angle = state.cover_angle().await;
        let light_on = state.light_on().await;
        let brightness = state.brightness().await;

        // Mirror the mock's `[GOPS]` mapping: 0 angle → 1 (open),
        // 270 → 0 (closed), anything else → 255 (in-between). The mock's
        // SMOV completes moves instantly, so motor_running is false here
        // even right after open_cover/close_cover.
        let cover_raw = if motor_running {
            255
        } else if cover_angle == 0 {
            1
        } else if cover_angle == 270 {
            0
        } else {
            255
        };

        let mut s = snap.write().await;
        s.motor_running = Some(motor_running);
        s.cover_raw = Some(cover_raw);
        s.light_on = Some(light_on);
        s.brightness = Some(brightness);
    }
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
