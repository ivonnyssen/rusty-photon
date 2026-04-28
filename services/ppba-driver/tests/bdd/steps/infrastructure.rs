#![allow(dead_code)]
//! Test infrastructure: ppba-driver process management and config helpers.

pub use bdd_infra::ServiceHandle;

// ---------------------------------------------------------------------------
// Config helpers
// ---------------------------------------------------------------------------

/// Build a default test config JSON with both devices enabled.
/// Uses port 0 for OS-assigned port and a short polling interval for fast tests.
pub fn default_test_config() -> serde_json::Value {
    serde_json::json!({
        "serial": {
            "port": "/dev/mock",
            "baud_rate": 9600,
            "polling_interval_ms": 200,
            "timeout_secs": 2
        },
        "server": { "port": 0, "discovery_port": null },
        "switch": {
            "enabled": true,
            "name": "Pegasus PPBA Switch",
            "unique_id": "ppba-switch-001",
            "description": "Pegasus Astro PPBA Gen2 Power Control",
            "device_number": 0
        },
        "observingconditions": {
            "enabled": true,
            "name": "Pegasus PPBA Weather",
            "unique_id": "ppba-observingconditions-001",
            "description": "Pegasus Astro PPBA Environmental Sensors",
            "device_number": 0,
            "averaging_period_ms": 300000
        }
    })
}

/// Build a test config with only the switch device enabled.
pub fn switch_only_config() -> serde_json::Value {
    let mut config = default_test_config();
    config["observingconditions"]["enabled"] = serde_json::json!(false);
    config
}

/// Build a test config with only the OC device enabled.
pub fn oc_only_config() -> serde_json::Value {
    let mut config = default_test_config();
    config["switch"]["enabled"] = serde_json::json!(false);
    config
}

/// Build a test config with both devices disabled.
pub fn both_disabled_config() -> serde_json::Value {
    let mut config = default_test_config();
    config["switch"]["enabled"] = serde_json::json!(false);
    config["observingconditions"]["enabled"] = serde_json::json!(false);
    config
}
