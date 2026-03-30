//! HTTP client abstraction for testability

use async_trait::async_trait;

/// HTTP response from a request
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Abstraction over HTTP client for dependency injection
#[async_trait]
#[cfg_attr(test, mockall::automock)]
pub trait HttpClient: Send + Sync {
    /// Send a GET request to the given URL
    async fn get(&self, url: &str) -> crate::Result<HttpResponse>;

    /// Send a PUT request with form-encoded body
    async fn put_form(&self, url: &str, params: &[(&str, &str)]) -> crate::Result<HttpResponse>;

    /// Send a POST request with form-encoded body
    async fn post_form(&self, url: &str, params: &[(&str, &str)]) -> crate::Result<HttpResponse>;
}

/// Production HTTP client using reqwest
#[derive(Default)]
pub struct ReqwestHttpClient {
    client: reqwest::Client,
    auth: Option<(String, String)>,
}

impl ReqwestHttpClient {
    /// Create a new HTTP client with optional CA certificate trust.
    ///
    /// When `ca_cert_path` is `Some`, the PEM-encoded CA certificate at that
    /// path is added as a trusted root, allowing connections to services using
    /// certificates signed by the Rusty Photon CA.
    pub fn new(ca_cert_path: Option<&std::path::Path>) -> crate::Result<Self> {
        let client = rp_tls::client::build_reqwest_client(ca_cert_path).map_err(|e| {
            crate::SentinelError::Config(format!("failed to build HTTP client: {e}"))
        })?;
        Ok(Self { client, auth: None })
    }

    /// Create a new HTTP client with CA trust and HTTP Basic Auth credentials.
    ///
    /// The credentials are sent with every request via the `Authorization: Basic`
    /// header, enabling connections to auth-enabled services.
    pub fn with_auth(
        ca_cert_path: Option<&std::path::Path>,
        username: String,
        password: String,
    ) -> crate::Result<Self> {
        let mut client = Self::new(ca_cert_path)?;
        client.auth = Some((username, password));
        Ok(client)
    }
}

#[async_trait]
impl HttpClient for ReqwestHttpClient {
    async fn get(&self, url: &str) -> crate::Result<HttpResponse> {
        tracing::debug!("GET {}", url);
        let mut request = self.client.get(url);
        if let Some((ref user, ref pass)) = self.auth {
            request = request.basic_auth(user, Some(pass));
        }
        let response = request
            .send()
            .await
            .map_err(|e| crate::SentinelError::Http(format!("GET {} failed: {}", url, e)))?;

        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|e| crate::SentinelError::Http(format!("Reading response body: {}", e)))?;

        tracing::debug!("GET {} -> {} ({} bytes)", url, status, body.len());
        Ok(HttpResponse { status, body })
    }

    async fn put_form(&self, url: &str, params: &[(&str, &str)]) -> crate::Result<HttpResponse> {
        tracing::debug!("PUT {}", url);
        let mut request = self.client.put(url).form(params);
        if let Some((ref user, ref pass)) = self.auth {
            request = request.basic_auth(user, Some(pass));
        }
        let response = request
            .send()
            .await
            .map_err(|e| crate::SentinelError::Http(format!("PUT {} failed: {}", url, e)))?;

        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|e| crate::SentinelError::Http(format!("Reading response body: {}", e)))?;

        tracing::debug!("PUT {} -> {} ({} bytes)", url, status, body.len());
        Ok(HttpResponse { status, body })
    }

    async fn post_form(&self, url: &str, params: &[(&str, &str)]) -> crate::Result<HttpResponse> {
        tracing::debug!("POST {}", url);
        let mut request = self.client.post(url).form(params);
        if let Some((ref user, ref pass)) = self.auth {
            request = request.basic_auth(user, Some(pass));
        }
        let response = request
            .send()
            .await
            .map_err(|e| crate::SentinelError::Http(format!("POST {} failed: {}", url, e)))?;

        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|e| crate::SentinelError::Http(format!("Reading response body: {}", e)))?;

        tracing::debug!("POST {} -> {} ({} bytes)", url, status, body.len());
        Ok(HttpResponse { status, body })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    /// A URL that will always refuse connections (port 1 is reserved and unbound)
    const UNREACHABLE_URL: &str = "http://127.0.0.1:1/test";

    #[tokio::test]
    async fn get_connection_refused_returns_http_error() {
        let client = ReqwestHttpClient::default();
        let err = client.get(UNREACHABLE_URL).await.unwrap_err();

        match &err {
            crate::SentinelError::Http(msg) => {
                assert!(
                    msg.starts_with("GET http://127.0.0.1:1/test failed:"),
                    "{msg}"
                );
            }
            other => panic!("expected SentinelError::Http, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn put_form_connection_refused_returns_http_error() {
        let client = ReqwestHttpClient::default();
        let err = client
            .put_form(UNREACHABLE_URL, &[("key", "value")])
            .await
            .unwrap_err();

        match &err {
            crate::SentinelError::Http(msg) => {
                assert!(
                    msg.starts_with("PUT http://127.0.0.1:1/test failed:"),
                    "{msg}"
                );
            }
            other => panic!("expected SentinelError::Http, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn post_form_connection_refused_returns_http_error() {
        let client = ReqwestHttpClient::default();
        let err = client
            .post_form(UNREACHABLE_URL, &[("key", "value")])
            .await
            .unwrap_err();

        match &err {
            crate::SentinelError::Http(msg) => {
                assert!(
                    msg.starts_with("POST http://127.0.0.1:1/test failed:"),
                    "{msg}"
                );
            }
            other => panic!("expected SentinelError::Http, got {other:?}"),
        }
    }
}
