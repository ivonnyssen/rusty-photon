#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! `ui-htmx` — the server-rendered web configuration UI (BFF) for rusty-photon.
//!
//! A client of the drivers (not part of `rp`): it reads and writes each driver's
//! configuration through the driver's own `config.get` / `config.apply` ASCOM
//! actions, rendering hand-built forms with axum + Maud + HTMX. See
//! [`docs/services/ui-htmx.md`](../../../docs/services/ui-htmx.md).

pub mod assets;
pub mod config;
pub mod driver_client;
pub mod io;
pub mod pages;

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use maud::Markup;

pub use config::{load_config, Config};
pub use driver_client::{
    AlpacaConfigClient, ApplyStatus, ConfigApplyResponse, ConfigClient, ConfigClientError,
    ConfigGetResponse, FieldError,
};
pub use io::{HttpClient, ReqwestHttpClient};

use pages::Banner;

/// Shared handler state. Phase 2 holds a single driver client (`dsd-fp2`).
#[derive(Clone)]
pub struct AppState {
    dsd_fp2: Arc<dyn ConfigClient>,
}

impl AppState {
    /// Build the production state: an `AlpacaConfigClient` over a `reqwest`-backed
    /// `HttpClient` (CA trust + optional Basic auth) for the configured driver.
    pub fn from_config(config: &Config) -> Result<Self, Box<dyn std::error::Error>> {
        let target = &config.drivers.dsd_fp2;

        // Reject credentials embedded in the URL (`http://user:pass@host`). They
        // would otherwise leak into error messages (rendered in the page) and
        // debug logs that echo the request URL. Credentials belong in the
        // `auth` field, which is sent as a redactable `Authorization` header.
        let parsed = reqwest::Url::parse(&target.base_url)
            .map_err(|e| format!("invalid driver base_url {:?}: {e}", target.base_url))?;
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(
                "driver base_url must not contain credentials (user:pass@…); \
                 put them in the `auth` field instead"
                    .into(),
            );
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
        let dsd_fp2: Arc<dyn ConfigClient> = Arc::new(client);
        Ok(Self { dsd_fp2 })
    }

    /// Build state from an explicit client (tests inject a stub).
    pub fn with_client(dsd_fp2: Arc<dyn ConfigClient>) -> Self {
        Self { dsd_fp2 }
    }
}

/// Build the BFF axum router.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/config/dsd-fp2", get(config_get).post(config_post))
        .route("/config/dsd-fp2/status", get(config_status))
        .route("/health", get(health))
        .route("/assets/app.css", get(assets::app_css))
        .route("/assets/htmx.min.js", get(assets::htmx_js))
        .with_state(state)
}

async fn index() -> Markup {
    pages::index_page()
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

/// Page title for the dsd-fp2 config routes (used when wrapping a card in the
/// full-page layout for a non-HTMX request).
const CONFIG_TITLE: &str = "dsd-fp2 · configuration";

async fn config_get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let card = match state.dsd_fp2.get_config().await {
        Ok(resp) => pages::config_card(&resp.config, &resp.overrides, &[], None),
        Err(err) => pages::error_card(&err),
    };
    respond(card, &headers, CONFIG_TITLE)
}

async fn config_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    // Route every path through `respond` so a non-HTMX POST (JS disabled, or
    // the `<form method="post">` fallback) still gets the full-page layout
    // instead of a bare `#config-card` fragment.
    let card = match pages::merge_form(&form) {
        Err(err) => pages::message_error_card(&err.to_string()),
        // BFF-side field errors (e.g. a port out of range) — re-render with the
        // errors instead of sending a value the driver rejects with a
        // non-field-level parse error.
        Ok(merged) if !merged.errors.is_empty() => pages::config_card(
            &merged.config,
            &merged.overrides,
            &merged.errors,
            Some(Banner::Invalid),
        ),
        Ok(merged) => match state.dsd_fp2.apply_config(&merged.config).await {
            Ok(resp) => match resp.status {
                ApplyStatus::Applying => pages::reconnecting_card(),
                // Persisted with no reload needed. Re-fetch so the success
                // state shows the driver's real effective config (it may have
                // normalized values — e.g. a trimmed serial.port — written
                // through override-pinned fields, or redacted secrets) rather
                // than echoing the submitted blob. If the refresh hiccups, the
                // apply already succeeded, so fall back to the submitted values.
                ApplyStatus::Ok => match state.dsd_fp2.get_config().await {
                    Ok(fresh) => pages::config_card(
                        &fresh.config,
                        &fresh.overrides,
                        &[],
                        Some(Banner::Saved),
                    ),
                    Err(_) => pages::config_card(
                        &merged.config,
                        &merged.overrides,
                        &[],
                        Some(Banner::Saved),
                    ),
                },
                ApplyStatus::Invalid => pages::config_card(
                    &merged.config,
                    &merged.overrides,
                    &resp.errors,
                    Some(Banner::Invalid),
                ),
            },
            Err(err) => pages::error_card(&err),
        },
    };
    respond(card, &headers, CONFIG_TITLE)
}

async fn config_status(State(state): State<AppState>) -> Markup {
    match state.dsd_fp2.get_config().await {
        Ok(resp) => pages::config_card(
            &resp.config,
            &resp.overrides,
            &[],
            Some(Banner::Reconnected),
        ),
        Err(_) => pages::reconnecting_card(),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    fn config_with_base_url(base_url: &str) -> Config {
        let mut config = Config::default();
        config.drivers.dsd_fp2.base_url = base_url.to_string();
        config
    }

    #[test]
    fn from_config_rejects_url_credentials() {
        let config = config_with_base_url("http://obs:secret@127.0.0.1:11119");
        // `AppState` isn't `Debug` (holds `Arc<dyn ConfigClient>`), so match
        // rather than `unwrap_err`.
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
}
