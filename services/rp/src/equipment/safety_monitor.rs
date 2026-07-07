use std::sync::Arc;

use ascom_alpaca::api::{SafetyMonitor, TypedDevice};
use tracing::{debug, error};

use super::alpaca::{
    build_alpaca_client, retry_connect_attempt, AttemptOutcome, GET_DEVICES_TIMEOUT,
};
use crate::config;

pub struct SafetyMonitorEntry {
    pub id: String,
    pub connected: bool,
    pub config: config::SafetyMonitorConfig,
    pub device: Option<Arc<dyn SafetyMonitor>>,
}

pub(super) async fn connect_safety_monitor(
    config: &config::SafetyMonitorConfig,
) -> SafetyMonitorEntry {
    debug!(sm_id = %config.id, alpaca_url = %config.alpaca_url, device_number = config.device_number, "connecting to safety monitor");

    let client = match build_alpaca_client(&config.alpaca_url, config.auth.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            error!(sm_id = %config.id, error = %e, "failed to create Alpaca client for safety monitor");
            return SafetyMonitorEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    let label = format!("safety monitor {}", config.id);
    let outcome = retry_connect_attempt(&label, |_attempt| async {
        let devices = match tokio::time::timeout(GET_DEVICES_TIMEOUT, client.get_devices()).await {
            Ok(Ok(devices)) => devices,
            Ok(Err(e)) => return AttemptOutcome::Transient(format!("get_devices: {e}")),
            Err(_) => {
                return AttemptOutcome::Transient(format!(
                    "get_devices: timeout after {:?}",
                    GET_DEVICES_TIMEOUT
                ));
            }
        };

        let mut sm_index = 0u32;
        let mut found_sm: Option<Arc<dyn SafetyMonitor>> = None;
        for device in devices {
            if let TypedDevice::SafetyMonitor(sm) = device {
                if sm_index == config.device_number {
                    found_sm = Some(sm);
                    break;
                }
                sm_index += 1;
            }
        }

        let sm = match found_sm {
            Some(s) => s,
            None => {
                return AttemptOutcome::Permanent(format!(
                    "safety monitor at index {} not found on Alpaca server",
                    config.device_number
                ));
            }
        };

        match sm.set_connected(true).await {
            Ok(()) => AttemptOutcome::Ok(sm),
            Err(e) => AttemptOutcome::Transient(format!("set_connected: {e}")),
        }
    })
    .await;

    match outcome {
        Ok(sm) => {
            debug!(sm_id = %config.id, "safety monitor connected successfully");
            SafetyMonitorEntry {
                id: config.id.clone(),
                connected: true,
                config: config.clone(),
                device: Some(sm),
            }
        }
        Err(msg) => {
            // A safety monitor that cannot be read counts as unsafe
            // (fail-unsafe, rp.md § Safety) — so a failed connect here
            // will gate the session until the device becomes reachable.
            error!(sm_id = %config.id, error = %msg, "failed to connect safety monitor");
            SafetyMonitorEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            }
        }
    }
}
