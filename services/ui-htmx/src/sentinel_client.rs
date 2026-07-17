//! Sentinel restart client.
//!
//! Speaks Sentinel's Service Restart API (`POST /api/services/{name}/restart`,
//! see `docs/services/sentinel.md` §Service Restart API) over an
//! [`HttpClient`]. Sentinel reports the restart's outcome as a domain result
//! on HTTP 200 ([`RestartOutcome`]); 404/409 are addressing errors (no
//! discovered service by that name / a restart already in flight), surfaced
//! as typed [`SentinelClientError`] variants so the page can name the reason.
//! Handlers depend on the [`SentinelClient`] trait so tests can inject stubs.

use std::sync::Arc;

use async_trait::async_trait;

use crate::io::HttpClient;

/// Sentinel's domain outcome for an accepted restart request (HTTP 200).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RestartOutcome {
    /// `"ok"` (the platform accepted the restart) or `"failed"`.
    pub status: String,
    /// Present on `"ok"`: `"healthy"`, `"timeout"`, or `"skipped"`.
    #[serde(default)]
    pub recovery: Option<String>,
    /// Present on `"failed"`: what went wrong with the command.
    #[serde(default)]
    pub detail: Option<String>,
}

impl RestartOutcome {
    /// The platform accepted and ran the restart.
    pub fn is_ok(&self) -> bool {
        self.status == "ok"
    }

    /// Sentinel's recovery check never confirmed recovery within its budget.
    /// (The driver may still come back — the budget is Sentinel's, not the
    /// driver's — so the page keeps its own reconnect poll either way.)
    pub fn recovery_timed_out(&self) -> bool {
        self.recovery.as_deref() == Some("timeout")
    }
}

/// A failure asking Sentinel to restart a service.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SentinelClientError {
    /// Sentinel answered 404: no discovered service carries the name.
    #[error("Sentinel does not supervise this driver: {0}")]
    UnknownService(String),
    /// Sentinel answered 409: a restart of the service is already in flight.
    #[error("Sentinel rejected the restart: {0}")]
    Rejected(String),
    /// Network failure or an unexpected HTTP status (Sentinel is down, or its
    /// auth/TLS layer refused us).
    #[error("could not reach Sentinel: {0}")]
    Transport(String),
    /// The response body could not be decoded.
    #[error("could not decode the Sentinel response: {0}")]
    Decode(String),
}

/// Asks Sentinel to restart a supervised service. Handlers depend on this
/// trait so tests can inject canned outcomes.
#[async_trait]
pub trait SentinelClient: Send + Sync {
    /// `POST /api/services/{service}/restart` — `service` is the discovered
    /// service name (the unit name minus the `rusty-photon-` prefix).
    async fn restart(&self, service: &str) -> Result<RestartOutcome, SentinelClientError>;
}

/// `SentinelClient` backed by Sentinel's REST API.
pub struct HttpSentinelClient {
    http: Arc<dyn HttpClient>,
    base_url: String,
}

impl HttpSentinelClient {
    pub fn new(http: Arc<dyn HttpClient>, base_url: &str) -> Self {
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }
}

/// Extract the `error` string a 4xx body carries (`{"error":"…"}`), falling
/// back to the raw body so an unexpected shape is still surfaced verbatim.
fn error_reason(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
        .unwrap_or_else(|| body.to_string())
}

#[async_trait]
impl SentinelClient for HttpSentinelClient {
    async fn restart(&self, service: &str) -> Result<RestartOutcome, SentinelClientError> {
        let url = format!("{}/api/services/{service}/restart", self.base_url);
        let response = self
            .http
            .post(&url)
            .await
            .map_err(|e| SentinelClientError::Transport(e.to_string()))?;
        match response.status {
            200..=299 => serde_json::from_str(&response.body)
                .map_err(|e| SentinelClientError::Decode(e.to_string())),
            404 => Err(SentinelClientError::UnknownService(error_reason(
                &response.body,
            ))),
            409 => Err(SentinelClientError::Rejected(error_reason(&response.body))),
            status => Err(SentinelClientError::Transport(format!(
                "HTTP {status} from {url}"
            ))),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::io::{HttpResponse, MockHttpClient};

    fn client_returning(status: u16, body: &str) -> HttpSentinelClient {
        let body = body.to_string();
        let mut mock = MockHttpClient::new();
        mock.expect_post()
            .withf(|url| url == "http://sentinel:11114/api/services/dsd-fp2/restart")
            .returning(move |_| {
                let body = body.clone();
                Box::pin(async move { Ok(HttpResponse { status, body }) })
            });
        HttpSentinelClient::new(Arc::new(mock), "http://sentinel:11114/")
    }

    #[tokio::test]
    async fn ok_outcome_is_parsed() {
        let client = client_returning(
            200,
            r#"{"service":"dsd-fp2","status":"ok","recovery":"healthy"}"#,
        );
        let outcome = client.restart("dsd-fp2").await.unwrap();
        assert!(outcome.is_ok());
        assert!(!outcome.recovery_timed_out());
        assert_eq!(outcome.recovery.as_deref(), Some("healthy"));
    }

    #[tokio::test]
    async fn recovery_timeout_is_flagged() {
        let client = client_returning(
            200,
            r#"{"service":"dsd-fp2","status":"ok","recovery":"timeout"}"#,
        );
        let outcome = client.restart("dsd-fp2").await.unwrap();
        assert!(outcome.is_ok());
        assert!(outcome.recovery_timed_out());
    }

    #[tokio::test]
    async fn failed_outcome_carries_detail() {
        let client = client_returning(
            200,
            r#"{"service":"dsd-fp2","status":"failed","detail":"restart `x` exited with 1"}"#,
        );
        let outcome = client.restart("dsd-fp2").await.unwrap();
        assert!(!outcome.is_ok());
        assert_eq!(outcome.detail.as_deref(), Some("restart `x` exited with 1"));
    }

    #[tokio::test]
    async fn not_found_maps_to_unknown_service_with_reason() {
        let client = client_returning(404, r#"{"error":"no configured service named 'dsd-fp2'"}"#);
        let err = client.restart("dsd-fp2").await.unwrap_err();
        assert!(
            matches!(&err, SentinelClientError::UnknownService(reason)
                if reason == "no configured service named 'dsd-fp2'"),
            "{err:?}"
        );
        assert!(err.to_string().contains("does not supervise"), "{err}");
    }

    #[tokio::test]
    async fn conflict_maps_to_rejected_with_reason() {
        let client = client_returning(
            409,
            r#"{"error":"a restart of 'dsd-fp2' is already running"}"#,
        );
        let err = client.restart("dsd-fp2").await.unwrap_err();
        assert!(
            matches!(&err, SentinelClientError::Rejected(reason)
                if reason == "a restart of 'dsd-fp2' is already running"),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn unexpected_status_is_a_transport_error() {
        let client = client_returning(500, "boom");
        let err = client.restart("dsd-fp2").await.unwrap_err();
        assert!(matches!(err, SentinelClientError::Transport(_)), "{err:?}");
    }

    #[tokio::test]
    async fn malformed_ok_body_is_a_decode_error() {
        let client = client_returning(200, "not json");
        let err = client.restart("dsd-fp2").await.unwrap_err();
        assert!(matches!(err, SentinelClientError::Decode(_)), "{err:?}");
    }

    #[tokio::test]
    async fn non_envelope_error_body_is_surfaced_verbatim() {
        let client = client_returning(404, "plain text");
        let err = client.restart("dsd-fp2").await.unwrap_err();
        assert!(
            matches!(&err, SentinelClientError::UnknownService(reason) if reason == "plain text"),
            "{err:?}"
        );
    }
}
