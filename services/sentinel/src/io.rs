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
pub struct ReqwestHttpClient {
    client: reqwest::Client,
}

impl ReqwestHttpClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl Default for ReqwestHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HttpClient for ReqwestHttpClient {
    async fn get(&self, url: &str) -> crate::Result<HttpResponse> {
        tracing::debug!("GET {}", url);
        let response = self
            .client
            .get(url)
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
        let response = self
            .client
            .put(url)
            .form(params)
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
        let response = self
            .client
            .post(url)
            .form(params)
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
