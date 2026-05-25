//! Cucumber `World` for the `ui-htmx` config-page BDD suite.
//!
//! The `World` holds a stubbed `ConfigClient` whose canned `config.get` /
//! `config.apply` results are configured by `Given` steps. `When` steps drive
//! the real axum router (built around that stub) via `oneshot`, capturing the
//! rendered HTML and status for `Then` assertions.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::Request;
use cucumber::World;
use serde_json::Value;
use tower::ServiceExt;

use ui_htmx::{
    build_router, AppState, ConfigApplyResponse, ConfigClient, ConfigClientError, ConfigGetResponse,
};

/// A `ConfigClient` that returns whatever the scenario configured.
#[derive(Debug, Clone, Default)]
struct StubConfigClient {
    get: Arc<Mutex<Option<Result<ConfigGetResponse, ConfigClientError>>>>,
    apply: Arc<Mutex<Option<Result<ConfigApplyResponse, ConfigClientError>>>>,
}

#[async_trait]
impl ConfigClient for StubConfigClient {
    async fn get_config(&self) -> Result<ConfigGetResponse, ConfigClientError> {
        self.get
            .lock()
            .unwrap()
            .clone()
            .expect("config.get not configured for this scenario")
    }

    async fn apply_config(
        &self,
        _config: &Value,
    ) -> Result<ConfigApplyResponse, ConfigClientError> {
        self.apply
            .lock()
            .unwrap()
            .clone()
            .expect("config.apply not configured for this scenario")
    }
}

#[derive(Debug, Default, World)]
pub struct UiWorld {
    stub: StubConfigClient,
    pub last_status: u16,
    pub last_body: String,
}

impl UiWorld {
    pub fn set_get(&self, result: Result<ConfigGetResponse, ConfigClientError>) {
        *self.stub.get.lock().unwrap() = Some(result);
    }

    pub fn set_apply(&self, result: Result<ConfigApplyResponse, ConfigClientError>) {
        *self.stub.apply.lock().unwrap() = Some(result);
    }

    fn router(&self) -> axum::Router {
        let client: Arc<dyn ConfigClient> = Arc::new(self.stub.clone());
        build_router(AppState::with_client(client))
    }

    pub async fn get(&mut self, path: &str) {
        let response = self
            .router()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        self.capture(response).await;
    }

    pub async fn post_form(&mut self, path: &str, body: String) {
        let response = self
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Request", "true")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        self.capture(response).await;
    }

    async fn capture(&mut self, response: axum::response::Response) {
        self.last_status = response.status().as_u16();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        self.last_body = String::from_utf8(bytes.to_vec()).unwrap();
    }

    /// The `<input ...>` tag whose `name` attribute is `name`.
    pub fn input_tag(&self, name: &str) -> String {
        let needle = format!("name=\"{name}\"");
        let pos = self
            .last_body
            .find(&needle)
            .unwrap_or_else(|| panic!("no input named {name:?} in:\n{}", self.last_body));
        let start = self.last_body[..pos]
            .rfind("<input")
            .expect("no <input before name attribute");
        let end = self.last_body[start..]
            .find('>')
            .expect("unterminated input tag")
            + start;
        self.last_body[start..=end].to_string()
    }
}
