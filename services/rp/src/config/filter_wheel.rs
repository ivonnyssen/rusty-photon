use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FilterWheelConfig {
    pub id: String,
    #[serde(default)]
    pub camera_id: String,
    pub alpaca_url: String,
    #[serde(default)]
    pub device_number: u32,
    #[serde(default)]
    pub filters: Vec<String>,
    /// Optional HTTP Basic Auth credentials for connecting to auth-enabled Alpaca services
    #[serde(default)]
    pub auth: Option<rp_auth::config::ClientAuthConfig>,
}
