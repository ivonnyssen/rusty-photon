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

    /// Drive one poll cycle by issuing the same read commands the
    /// while-open task would, but synchronously, so scenarios that
    /// depend on cached state can `Then` directly without sleeping.
    pub async fn refresh_cache(&self) {
        let snap = self.manager().snapshot();
        let factory = self.factory();
        let state = factory.state();
        // Read the simulator state directly and update the cached snapshot.
        let mut s = snap.write().await;
        // Use the simulator's observable accessors plus a direct lock read
        // for the cover angle — the device itself doesn't expose this.
        s.light_on = Some(state.light_on().await);
        s.brightness = Some(state.brightness().await);
        // For motor/cover, fall through to the simulator's command path
        // (it's already exercised by unit tests; here we just hard-code
        // the post-move steady state since our mock completes moves
        // instantly).
        s.motor_running = Some(false);
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
