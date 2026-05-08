use std::time::Duration;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct CoverCalibratorConfig {
    pub id: String,
    pub alpaca_url: String,
    #[serde(default)]
    pub device_number: u32,
    /// Poll interval when waiting for cover/calibrator state changes (default `"3s"`)
    #[serde(
        default = "default_cover_calibrator_poll_interval",
        with = "humantime_serde"
    )]
    pub poll_interval: Duration,
    /// Optional HTTP Basic Auth credentials for connecting to auth-enabled Alpaca services
    #[serde(default)]
    pub auth: Option<rp_auth::config::ClientAuthConfig>,
}

fn default_cover_calibrator_poll_interval() -> Duration {
    Duration::from_secs(3)
}
