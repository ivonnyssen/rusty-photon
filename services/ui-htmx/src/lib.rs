#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! `ui-htmx` — the server-rendered web configuration UI (BFF) for rusty-photon.
//!
//! A client of the drivers (not part of `rp`): it reads and writes each driver's
//! configuration through the driver's own `config.get` / `config.schema` /
//! `config.apply` ASCOM actions, rendering the form **generically from the
//! driver's JSON Schema** (see [`pages`]) with axum + Maud + HTMX. One BFF
//! configures **any** number of drivers, addressed by service id under
//! `/config/{service}`. See
//! [`docs/services/ui-htmx.md`](../../../docs/services/ui-htmx.md).

pub mod assets;
pub mod config;
pub mod driver_client;
/// Test-only `/fixtures/*` routes (UI-testing plan §9 Tier 1) — compiled ONLY
/// under the `test-fixtures` cargo feature, so they ship nothing in the real
/// binary. `#[coverage(off)]` keeps this test-only code out of the coverage
/// numbers even when the feature is on (e.g. the `--all-features` coverage build).
#[cfg(feature = "test-fixtures")]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod fixtures;
pub mod io;
pub mod pages;
/// Test-only Server-Sent-Events fixture routes (UI-testing plan §9 Tier 2) —
/// compiled ONLY under the `test-sse` cargo feature, so they ship nothing in the
/// real binary. `#[coverage(off)]` keeps this test-only code (and the streaming
/// `async-stream` machinery it uses) out of the coverage numbers even when the
/// feature is on.
#[cfg(feature = "test-sse")]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod sse_fixtures;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::extract::{Form, Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use maud::Markup;
use serde::Deserialize;

pub use config::{load_config, Config};
pub use driver_client::{
    AlpacaConfigClient, ApplyStatus, ConfigApplyResponse, ConfigClient, ConfigClientError,
    ConfigGetResponse, ConfigSchemaResponse, FieldError,
};
pub use io::{HttpClient, ReqwestHttpClient};

use pages::{Banner, DriverLink, FieldModel, Page};

/// One configured driver: its display strings plus the client that speaks the
/// config-action protocol to it.
struct DriverHandle {
    title: String,
    subtitle: String,
    client: Arc<dyn ConfigClient>,
}

impl DriverHandle {
    fn page<'a>(&'a self, service: &'a str) -> Page<'a> {
        Page {
            service,
            title: &self.title,
            subtitle: &self.subtitle,
        }
    }
}

/// Shared handler state: every configured driver, keyed by service id.
#[derive(Clone)]
pub struct AppState {
    drivers: Arc<BTreeMap<String, DriverHandle>>,
}

impl AppState {
    /// Build the production state: an `AlpacaConfigClient` over a `reqwest`-backed
    /// `HttpClient` (CA trust + optional Basic auth) for every configured driver.
    pub fn from_config(config: &Config) -> Result<Self, Box<dyn std::error::Error>> {
        let mut drivers = BTreeMap::new();
        for (service, target) in &config.drivers.0 {
            // Reject credentials embedded in the URL (`http://user:pass@host`).
            // They would otherwise leak into error messages (rendered in the
            // page) and debug logs that echo the request URL. Credentials belong
            // in the `auth` field, sent as a redactable `Authorization` header.
            let parsed = reqwest::Url::parse(&target.base_url).map_err(|e| {
                format!(
                    "invalid base_url {:?} for driver {service:?}: {e}",
                    target.base_url
                )
            })?;
            if !parsed.username().is_empty() || parsed.password().is_some() {
                return Err(format!(
                    "driver {service:?} base_url must not contain credentials \
                     (user:pass@…); put them in the `auth` field instead"
                )
                .into());
            }

            let reqwest_client = match &target.auth {
                Some(auth) => ReqwestHttpClient::with_auth(
                    target.ca_cert_path.as_deref(),
                    auth.username.clone(),
                    auth.password.clone(),
                )?,
                None => ReqwestHttpClient::new(target.ca_cert_path.as_deref())?,
            };
            let http: Arc<dyn HttpClient> = Arc::new(reqwest_client);
            let client = AlpacaConfigClient::new(
                http,
                &target.base_url,
                &target.device_type,
                target.device_number,
            );
            drivers.insert(
                service.clone(),
                DriverHandle {
                    title: target.name.clone().unwrap_or_else(|| service.clone()),
                    subtitle: format!("{service} · {}", target.device_type),
                    client: Arc::new(client),
                },
            );
        }
        Ok(Self {
            drivers: Arc::new(drivers),
        })
    }

    /// Build single-driver state from an explicit client (tests inject a stub).
    pub fn with_client(service: &str, client: Arc<dyn ConfigClient>) -> Self {
        let mut drivers = BTreeMap::new();
        drivers.insert(
            service.to_string(),
            DriverHandle {
                title: service.to_string(),
                subtitle: service.to_string(),
                client,
            },
        );
        Self {
            drivers: Arc::new(drivers),
        }
    }
}

/// Build the BFF axum router.
pub fn build_router(state: AppState) -> Router {
    let router = Router::new()
        .route("/", get(index))
        .route("/config/{service}", get(config_get).post(config_post))
        .route("/config/{service}/status", get(config_status))
        .route("/health", get(health))
        .route("/assets/app.css", get(assets::app_css))
        .route("/assets/htmx.min.js", get(assets::htmx_js));
    // Test-only `/fixtures/*` routes, present only when the `test-fixtures`
    // feature is on (the BDD suite's binary). This `let` shadow is the standard
    // cfg-gated router-extend; the merge runs at startup so it stays covered.
    #[cfg(feature = "test-fixtures")]
    let router = router.merge(fixtures::routes());
    // Test-only SSE fixture routes (plan §9 Tier 2), present only with `test-sse`.
    #[cfg(feature = "test-sse")]
    let router = router.merge(sse_fixtures::routes());
    router.with_state(state)
}

async fn index(State(state): State<AppState>) -> Markup {
    let links: Vec<DriverLink> = state
        .drivers
        .iter()
        .map(|(service, handle)| DriverLink {
            service: service.clone(),
            title: handle.title.clone(),
        })
        .collect();
    pages::index_page(&links)
}

async fn health() -> &'static str {
    "OK"
}

fn is_htmx(headers: &HeaderMap) -> bool {
    headers.contains_key("HX-Request")
}

/// Wrap a `#config-card` fragment in the full page, unless this is an HTMX
/// request (then the bare fragment is returned for an `outerHTML` swap).
fn respond(card: Markup, headers: &HeaderMap, title: &str) -> Response {
    if is_htmx(headers) {
        card.into_response()
    } else {
        pages::layout(title, card).into_response()
    }
}

/// Page title for the config routes (used when wrapping a card in the full-page
/// layout for a non-HTMX request).
fn page_title(service: &str) -> String {
    format!("{service} · configuration")
}

/// The `?unlock=<field>` query for the config GET routes: the escape hatch that
/// renders one locked/identity field editable. Only names that are actually
/// locked in the driver's schema are honoured (see [`pages::unlocked_from_query`]).
#[derive(Debug, Default, Deserialize)]
struct UnlockQuery {
    unlock: Option<String>,
}

/// Fetch the driver's schema + config and render the filled form, or an error
/// card on any failure. Shared by the GET and reconnect handlers.
async fn render_form(
    handle: &DriverHandle,
    service: &str,
    unlock: Option<&str>,
    banner: Option<Banner>,
) -> Markup {
    let schema = match handle.client.get_schema().await {
        Ok(schema) => schema,
        Err(err) => return pages::error_card(service, &err),
    };
    let model = FieldModel::from_schema(&schema);
    let unlocked = pages::unlocked_from_query(&model, unlock);
    match handle.client.get_config().await {
        Ok(resp) => pages::config_card(
            &handle.page(service),
            &model,
            &resp.config,
            &resp.overrides,
            &unlocked,
            &[],
            banner,
        ),
        Err(err) => pages::error_card(service, &err),
    }
}

async fn config_get(
    State(state): State<AppState>,
    Path(service): Path<String>,
    Query(query): Query<UnlockQuery>,
    headers: HeaderMap,
) -> Response {
    let title = page_title(&service);
    let Some(handle) = state.drivers.get(&service) else {
        return respond(pages::unknown_service_card(&service), &headers, &title);
    };
    let card = render_form(handle, &service, query.unlock.as_deref(), None).await;
    respond(card, &headers, &title)
}

async fn config_post(
    State(state): State<AppState>,
    Path(service): Path<String>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let title = page_title(&service);
    let Some(handle) = state.drivers.get(&service) else {
        return respond(pages::unknown_service_card(&service), &headers, &title);
    };

    // The schema is needed both to coerce the submission and to render any
    // re-render with the correct field types and tiers. It is static per driver.
    let model = match handle.client.get_schema().await {
        Ok(schema) => FieldModel::from_schema(&schema),
        Err(err) => return respond(pages::error_card(&service, &err), &headers, &title),
    };
    let page = handle.page(&service);

    let card = match pages::merge_form(&form, &model) {
        Err(err) => pages::message_error_card(&service, &err.to_string()),
        // BFF-side field errors (e.g. a value out of its schema range) — re-render
        // with the errors instead of sending a value the driver would reject with
        // a non-field parse error. The unlocked set is preserved so an identity
        // field the operator was editing stays editable in place.
        Ok(merged) if !merged.errors.is_empty() => pages::config_card(
            &page,
            &model,
            &merged.config,
            &merged.overrides,
            &merged.unlocked,
            &merged.errors,
            Some(Banner::Invalid),
        ),
        Ok(merged) => match handle.client.apply_config(&merged.config).await {
            Ok(resp) => match resp.status {
                ApplyStatus::Applying => pages::reconnecting_card(&service),
                // Persisted with no reload needed. Re-fetch so the success state
                // shows the driver's real effective config (normalized values,
                // override-pinned write-throughs, redacted secrets) rather than
                // echoing the submitted blob. The apply succeeded, so the identity
                // field re-locks; if the refresh hiccups, fall back to the
                // submitted values, also re-locked.
                ApplyStatus::Ok => match handle.client.get_config().await {
                    Ok(fresh) => pages::config_card(
                        &page,
                        &model,
                        &fresh.config,
                        &fresh.overrides,
                        &[],
                        &[],
                        Some(Banner::Saved),
                    ),
                    Err(_) => pages::config_card(
                        &page,
                        &model,
                        &merged.config,
                        &merged.overrides,
                        &[],
                        &[],
                        Some(Banner::Saved),
                    ),
                },
                // Driver rejected the values: keep the unlocked set so a rejected
                // identity edit stays editable while the operator corrects it.
                ApplyStatus::Invalid => pages::config_card(
                    &page,
                    &model,
                    &merged.config,
                    &merged.overrides,
                    &merged.unlocked,
                    &resp.errors,
                    Some(Banner::Invalid),
                ),
            },
            Err(err) => pages::error_card(&service, &err),
        },
    };
    respond(card, &headers, &title)
}

async fn config_status(
    State(state): State<AppState>,
    Path(service): Path<String>,
    Query(query): Query<UnlockQuery>,
) -> Markup {
    let Some(handle) = state.drivers.get(&service) else {
        // Unknown service mid-poll: a benign reconnecting fragment keeps the page
        // from erroring; the user can navigate away.
        return pages::reconnecting_card(&service);
    };
    // The reconnect poll renders the refreshed form once the driver answers
    // again, or the reconnecting fragment while it is still down mid-reload.
    let (Ok(schema), Ok(resp)) = (
        handle.client.get_schema().await,
        handle.client.get_config().await,
    ) else {
        return pages::reconnecting_card(&service);
    };
    let model = FieldModel::from_schema(&schema);
    let unlocked = pages::unlocked_from_query(&model, query.unlock.as_deref());
    pages::config_card(
        &handle.page(&service),
        &model,
        &resp.config,
        &resp.overrides,
        &unlocked,
        &[],
        Some(Banner::Reconnected),
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn config_with_base_url(base_url: &str) -> Config {
        let mut config = Config::default();
        config.drivers.0.get_mut("dsd-fp2").unwrap().base_url = base_url.to_string();
        config
    }

    #[test]
    fn from_config_rejects_url_credentials() {
        let config = config_with_base_url("http://obs:secret@127.0.0.1:11119");
        match AppState::from_config(&config) {
            Ok(_) => panic!("expected from_config to reject credentials in base_url"),
            Err(e) => assert!(
                e.to_string().contains("must not contain credentials"),
                "{e}"
            ),
        }
    }

    #[test]
    fn from_config_accepts_plain_url() {
        AppState::from_config(&config_with_base_url("http://127.0.0.1:11119")).unwrap();
    }

    #[test]
    fn from_config_builds_every_driver() {
        let json = r#"{
            "drivers": {
                "dsd-fp2": { "base_url": "http://127.0.0.1:11119" },
                "qhy-focuser": { "base_url": "http://127.0.0.1:11113", "device_type": "focuser" }
            }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        let state = AppState::from_config(&config).unwrap();
        assert!(state.drivers.contains_key("dsd-fp2"));
        assert!(state.drivers.contains_key("qhy-focuser"));
        assert_eq!(
            state.drivers.get("qhy-focuser").unwrap().subtitle,
            "qhy-focuser · focuser"
        );
    }

    /// A `ConfigClient` whose actions report the target is not a config-capable
    /// driver (`ACTION_NOT_IMPLEMENTED`).
    struct NonConfigDriver;

    #[async_trait::async_trait]
    impl ConfigClient for NonConfigDriver {
        async fn get_config(&self) -> Result<ConfigGetResponse, ConfigClientError> {
            Err(not_implemented())
        }
        async fn get_schema(&self) -> Result<ConfigSchemaResponse, ConfigClientError> {
            Err(not_implemented())
        }
        async fn apply_config(
            &self,
            _config: &Value,
        ) -> Result<ConfigApplyResponse, ConfigClientError> {
            unreachable!("apply is not exercised by this test")
        }
    }

    fn not_implemented() -> ConfigClientError {
        ConfigClientError::Ascom {
            code: crate::driver_client::ACTION_NOT_IMPLEMENTED,
            message: "unknown action".to_string(),
        }
    }

    async fn body_of(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn config_get_renders_non_config_driver_banner() {
        let state = AppState::with_client("dsd-fp2", Arc::new(NonConfigDriver));
        let response = config_get(
            State(state),
            Path("dsd-fp2".to_string()),
            Query(UnlockQuery::default()),
            HeaderMap::new(),
        )
        .await;
        let html = body_of(response).await;
        assert!(
            html.contains("does not expose configuration actions"),
            "{html}"
        );
    }

    #[tokio::test]
    async fn config_get_unknown_service_is_an_error_card() {
        let state = AppState::with_client("dsd-fp2", Arc::new(NonConfigDriver));
        let response = config_get(
            State(state),
            Path("does-not-exist".to_string()),
            Query(UnlockQuery::default()),
            HeaderMap::new(),
        )
        .await;
        let html = body_of(response).await;
        assert!(html.contains("No configured driver named"), "{html}");
    }

    /// A `ConfigClient` returning a fixed schema + config — enough to render the
    /// form and assert on the identity-field lock state. `apply` is never hit.
    struct StaticConfigDriver;

    #[async_trait::async_trait]
    impl ConfigClient for StaticConfigDriver {
        async fn get_config(&self) -> Result<ConfigGetResponse, ConfigClientError> {
            Ok(ConfigGetResponse {
                config: json!({
                    "serial": { "port": "/dev/ttyACM0", "baud_rate": 115200, "polling_interval": "500ms", "timeout": "3s" },
                    "server": { "port": 11119, "discovery_port": 32227, "tls": null, "auth": null },
                    "cover_calibrator": { "name": "FP2", "unique_id": "dsd-fp2-001", "description": "panel", "enabled": true, "max_brightness": 4096 }
                }),
                overrides: vec![],
            })
        }
        async fn get_schema(&self) -> Result<ConfigSchemaResponse, ConfigClientError> {
            Ok(ConfigSchemaResponse {
                schema: json!({
                    "type": "object",
                    "properties": {
                        "serial": { "type": "object", "properties": {
                            "port": { "type": "string" },
                            "baud_rate": { "type": "integer", "minimum": 0 }
                        }},
                        "server": { "type": "object", "properties": {
                            "port": { "type": "integer", "minimum": 0, "maximum": 65535 },
                            "discovery_port": { "type": ["integer", "null"], "minimum": 0, "maximum": 65535 }
                        }},
                        "cover_calibrator": { "type": "object", "properties": {
                            "name": { "type": "string" },
                            "unique_id": { "type": "string" },
                            "enabled": { "type": "boolean" },
                            "max_brightness": { "type": "integer", "minimum": 0 }
                        }}
                    }
                }),
                locked_fields: vec!["cover_calibrator.unique_id".to_string()],
                read_only_fields: vec![
                    "server.port".to_string(),
                    "cover_calibrator.enabled".to_string(),
                ],
            })
        }
        async fn apply_config(
            &self,
            _config: &Value,
        ) -> Result<ConfigApplyResponse, ConfigClientError> {
            unreachable!("apply is not exercised by this test")
        }
    }

    async fn render_config_get(unlock: Option<&str>) -> String {
        let state = AppState::with_client("dsd-fp2", Arc::new(StaticConfigDriver));
        let query = UnlockQuery {
            unlock: unlock.map(String::from),
        };
        let response = config_get(
            State(state),
            Path("dsd-fp2".to_string()),
            Query(query),
            HeaderMap::new(),
        )
        .await;
        body_of(response).await
    }

    /// The `<input ...>` tag whose `name` attribute is `name`.
    fn input_tag(html: &str, name: &str) -> String {
        let pos = html.find(&format!(r#"name="{name}""#)).unwrap();
        let start = html[..pos].rfind("<input").unwrap();
        let end = html[start..].find('>').unwrap() + start;
        html[start..=end].to_string()
    }

    #[tokio::test]
    async fn config_get_locks_unique_id_without_unlock_query() {
        let html = render_config_get(None).await;
        let tag = input_tag(&html, "cover_calibrator.unique_id");
        assert!(tag.contains("disabled"), "unique_id not disabled: {tag}");
        assert!(
            html.contains("Unlock to edit"),
            "missing unlock link:\n{html}"
        );
    }

    #[tokio::test]
    async fn config_get_unlocks_unique_id_with_unlock_query() {
        let html = render_config_get(Some("cover_calibrator.unique_id")).await;
        let tag = input_tag(&html, "cover_calibrator.unique_id");
        assert!(!tag.contains("disabled"), "unique_id still disabled: {tag}");
        assert!(
            html.contains("Lock again"),
            "missing lock-again link:\n{html}"
        );
    }

    #[tokio::test]
    async fn config_get_unlock_query_ignores_non_locked_field() {
        // `?unlock=server.port` must not unlock a hard-read-only field.
        let html = render_config_get(Some("server.port")).await;
        assert!(
            input_tag(&html, "server.port").contains("disabled"),
            "server.port unexpectedly enabled"
        );
        assert!(
            input_tag(&html, "cover_calibrator.unique_id").contains("disabled"),
            "unique_id unexpectedly enabled"
        );
    }
}
