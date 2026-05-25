//! Driver config-action client.
//!
//! Speaks the cross-driver config-action protocol (`config.get` / `config.apply`
//! ASCOM actions) defined in [`docs/plans/ui-design/config-actions.md`] and
//! implemented for `dsd-fp2` in [`docs/services/dsd-fp2.md`]. `AlpacaConfigClient`
//! shapes the `PUT .../action` request over an [`HttpClient`], unwraps the Alpaca
//! envelope, and parses the inner JSON body. The page handlers depend on the
//! `ConfigClient` trait so they can be driven by a stub in tests.
//!
//! [`docs/plans/ui-design/config-actions.md`]: ../../../docs/plans/ui-design/config-actions.md
//! [`docs/services/dsd-fp2.md`]: ../../../docs/services/dsd-fp2.md

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::io::HttpClient;

/// ASCOM `ACTION_NOT_IMPLEMENTED` (`0x40C`). Returned by `action()` when the
/// target is not a config-capable driver.
pub const ACTION_NOT_IMPLEMENTED: i32 = 0x40C;

/// A failure reading or applying a driver's configuration.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ConfigClientError {
    /// Network failure, or a non-2xx HTTP status (the driver is down, or its
    /// auth/TLS layer refused us).
    #[error("could not reach the driver: {0}")]
    Transport(String),
    /// The action returned an ASCOM error (`ErrorNumber != 0`).
    #[error("driver returned ASCOM error {code}: {message}")]
    Ascom { code: i32, message: String },
    /// The envelope or inner body could not be decoded.
    #[error("could not decode the driver response: {0}")]
    Decode(String),
}

impl ConfigClientError {
    /// Whether this is the `ACTION_NOT_IMPLEMENTED` ASCOM error — i.e. the target
    /// driver does not expose the config actions.
    pub fn is_action_not_implemented(&self) -> bool {
        matches!(self, Self::Ascom { code, .. } if *code == ACTION_NOT_IMPLEMENTED)
    }
}

/// `config.get` response body: the effective config (secrets redacted) plus the
/// CLI-override-pinned field paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigGetResponse {
    pub config: Value,
    #[serde(default)]
    pub overrides: Vec<String>,
}

/// Outcome of a `config.apply`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApplyStatus {
    /// Persisted; an in-process reload is in flight.
    Applying,
    /// Persisted; nothing needed a reload.
    Ok,
    /// Validation failed; the file is unchanged.
    Invalid,
}

/// A single field-level validation error from `config.apply`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldError {
    pub path: String,
    pub msg: String,
}

/// `config.apply` response body. All classification arrays default to empty so a
/// partial body still decodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigApplyResponse {
    pub status: ApplyStatus,
    #[serde(default)]
    pub applied: Vec<String>,
    #[serde(default)]
    pub reload: Vec<String>,
    #[serde(default)]
    pub restart_required: Vec<String>,
    #[serde(default)]
    pub skipped_override: Vec<String>,
    #[serde(default)]
    pub persisted_to: Option<String>,
    #[serde(default)]
    pub errors: Vec<FieldError>,
}

/// Reads and applies a single driver's configuration. Handlers depend on this
/// trait so tests can inject canned responses.
#[async_trait]
pub trait ConfigClient: Send + Sync {
    async fn get_config(&self) -> Result<ConfigGetResponse, ConfigClientError>;
    async fn apply_config(&self, config: &Value) -> Result<ConfigApplyResponse, ConfigClientError>;
}

/// The Alpaca envelope every ASCOM response is wrapped in. We only read the
/// fields we need; the transaction ids are ignored.
#[derive(Debug, Deserialize)]
struct AlpacaEnvelope {
    #[serde(rename = "Value", default)]
    value: Value,
    #[serde(rename = "ErrorNumber", default)]
    error_number: i32,
    #[serde(rename = "ErrorMessage", default)]
    error_message: String,
}

/// `ConfigClient` backed by a driver's ASCOM Alpaca `action` endpoint.
pub struct AlpacaConfigClient {
    http: Arc<dyn HttpClient>,
    action_url: String,
}

impl AlpacaConfigClient {
    /// Target `{base_url}/api/v1/{device_type}/{device_number}/action`.
    pub fn new(
        http: Arc<dyn HttpClient>,
        base_url: &str,
        device_type: &str,
        device_number: u32,
    ) -> Self {
        let action_url = format!(
            "{}/api/v1/{}/{}/action",
            base_url.trim_end_matches('/'),
            device_type,
            device_number
        );
        Self { http, action_url }
    }

    /// Call an ASCOM action and return the parsed inner JSON body. For these
    /// vendor actions the driver returns a JSON string in `Value`, so we unwrap
    /// the envelope and then parse that string.
    async fn call_action(
        &self,
        action: &str,
        parameters: &str,
    ) -> Result<Value, ConfigClientError> {
        let response = self
            .http
            .put_form(
                &self.action_url,
                &[
                    ("Action", action),
                    ("Parameters", parameters),
                    ("ClientID", "1"),
                    ("ClientTransactionID", "1"),
                ],
            )
            .await
            .map_err(|e| ConfigClientError::Transport(e.to_string()))?;

        if !(200..300).contains(&response.status) {
            return Err(ConfigClientError::Transport(format!(
                "HTTP {} from {}",
                response.status, self.action_url
            )));
        }

        let envelope: AlpacaEnvelope = serde_json::from_str(&response.body)
            .map_err(|e| ConfigClientError::Decode(e.to_string()))?;
        if envelope.error_number != 0 {
            return Err(ConfigClientError::Ascom {
                code: envelope.error_number,
                message: envelope.error_message,
            });
        }

        let inner = envelope.value.as_str().ok_or_else(|| {
            ConfigClientError::Decode("action Value was not a JSON string".to_string())
        })?;
        serde_json::from_str(inner).map_err(|e| ConfigClientError::Decode(e.to_string()))
    }
}

#[async_trait]
impl ConfigClient for AlpacaConfigClient {
    async fn get_config(&self) -> Result<ConfigGetResponse, ConfigClientError> {
        let inner = self.call_action("config.get", "").await?;
        serde_json::from_value(inner).map_err(|e| ConfigClientError::Decode(e.to_string()))
    }

    async fn apply_config(&self, config: &Value) -> Result<ConfigApplyResponse, ConfigClientError> {
        let parameters =
            serde_json::to_string(config).map_err(|e| ConfigClientError::Decode(e.to_string()))?;
        let inner = self.call_action("config.apply", &parameters).await?;
        serde_json::from_value(inner).map_err(|e| ConfigClientError::Decode(e.to_string()))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::io::{HttpResponse, MockHttpClient};
    use serde_json::json;

    /// Wrap an inner JSON body the way the driver does: serialize it to a string
    /// and place it in the Alpaca envelope's `Value`.
    fn ok_envelope(inner: Value) -> String {
        let value_string = serde_json::to_string(&inner).unwrap();
        json!({
            "Value": value_string,
            "ClientTransactionID": 1,
            "ServerTransactionID": 7,
            "ErrorNumber": 0,
            "ErrorMessage": ""
        })
        .to_string()
    }

    fn client_returning(body: HttpResponse) -> AlpacaConfigClient {
        let mut http = MockHttpClient::new();
        http.expect_put_form().returning(move |_, _| {
            let body = body.clone();
            Box::pin(async move { Ok(body) })
        });
        AlpacaConfigClient::new(Arc::new(http), "http://driver:11119", "covercalibrator", 0)
    }

    #[tokio::test]
    async fn get_config_parses_the_inner_body() {
        let inner = json!({
            "config": { "serial": { "port": "/dev/ttyACM0" } },
            "overrides": ["serial.port"]
        });
        let client = client_returning(HttpResponse {
            status: 200,
            body: ok_envelope(inner),
        });

        let resp = client.get_config().await.unwrap();
        assert_eq!(
            resp.config.pointer("/serial/port").and_then(Value::as_str),
            Some("/dev/ttyACM0")
        );
        assert_eq!(resp.overrides, vec!["serial.port".to_string()]);
    }

    #[tokio::test]
    async fn apply_config_parses_status_and_reload() {
        let inner = json!({
            "status": "applying",
            "reload": ["cover_calibrator.max_brightness"],
            "persisted_to": "/tmp/dsd-fp2.json"
        });
        let client = client_returning(HttpResponse {
            status: 200,
            body: ok_envelope(inner),
        });

        let resp = client.apply_config(&json!({})).await.unwrap();
        assert_eq!(resp.status, ApplyStatus::Applying);
        assert_eq!(
            resp.reload,
            vec!["cover_calibrator.max_brightness".to_string()]
        );
    }

    #[tokio::test]
    async fn apply_config_surfaces_invalid_field_errors() {
        let inner = json!({
            "status": "invalid",
            "errors": [{ "path": "serial.baud_rate", "msg": "must be greater than 0" }]
        });
        let client = client_returning(HttpResponse {
            status: 200,
            body: ok_envelope(inner),
        });

        let resp = client.apply_config(&json!({})).await.unwrap();
        assert_eq!(resp.status, ApplyStatus::Invalid);
        assert_eq!(resp.errors.len(), 1);
        assert_eq!(resp.errors[0].path, "serial.baud_rate");
    }

    #[tokio::test]
    async fn ascom_error_number_maps_to_action_not_implemented() {
        let body = json!({
            "Value": "",
            "ErrorNumber": 0x40C,
            "ErrorMessage": "unknown action \"config.get\""
        })
        .to_string();
        let client = client_returning(HttpResponse { status: 200, body });

        let err = client.get_config().await.unwrap_err();
        assert!(err.is_action_not_implemented(), "{err:?}");
    }

    #[tokio::test]
    async fn http_non_2xx_is_a_transport_error() {
        let client = client_returning(HttpResponse {
            status: 401,
            body: "unauthorized".to_string(),
        });
        let err = client.get_config().await.unwrap_err();
        assert!(matches!(err, ConfigClientError::Transport(_)), "{err:?}");
    }

    #[tokio::test]
    async fn transport_failure_is_a_transport_error() {
        let mut http = MockHttpClient::new();
        http.expect_put_form().returning(|_, _| {
            Box::pin(async {
                Err::<HttpResponse, _>(crate::io::HttpError("connection refused".to_string()))
            })
        });
        let client =
            AlpacaConfigClient::new(Arc::new(http), "http://driver:11119", "covercalibrator", 0);
        let err = client.get_config().await.unwrap_err();
        assert!(matches!(err, ConfigClientError::Transport(_)), "{err:?}");
    }

    #[test]
    fn action_url_is_well_formed_and_trims_trailing_slash() {
        let http = Arc::new(MockHttpClient::new());
        let client = AlpacaConfigClient::new(http, "http://driver:11119/", "covercalibrator", 0);
        assert_eq!(
            client.action_url,
            "http://driver:11119/api/v1/covercalibrator/0/action"
        );
    }
}
