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
pub mod probe;
pub mod roster;
pub mod rp_client;
pub mod sentinel_client;
/// Test-only Server-Sent-Events fixture routes (UI-testing plan §9 Tier 2) —
/// compiled ONLY under the `test-sse` cargo feature, so they ship nothing in the
/// real binary. `#[coverage(off)]` keeps this test-only code (and the streaming
/// `async-stream` machinery it uses) out of the coverage numbers even when the
/// feature is on.
#[cfg(feature = "test-sse")]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod sse_fixtures;
pub mod sse_proxy;

use std::collections::BTreeMap;
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
    ConfigGetResponse, ConfigSchemaResponse, FieldError, RestConfigClient,
};
pub use io::{HttpClient, ReqwestHttpClient};
pub use sentinel_client::{
    HttpSentinelClient, RestartOutcome, SentinelClient, SentinelClientError,
};

use pages::{Banner, DriverLink, FieldModel, Page};

/// One configured driver: its display strings plus the client that speaks the
/// config-action protocol to it.
#[derive(Clone)]
struct DriverHandle {
    title: String,
    subtitle: String,
    client: Arc<dyn ConfigClient>,
    /// This driver's name in Sentinel's `services` map. `Some` only when the
    /// BFF has a `sentinel` block configured (then it defaults to the driver's
    /// own service id) — it gates every restart affordance.
    sentinel_service: Option<String>,
}

impl DriverHandle {
    fn page<'a>(&'a self, service: &'a str) -> Page<'a> {
        Page {
            service,
            title: &self.title,
            subtitle: &self.subtitle,
            can_restart: self.sentinel_service.is_some(),
        }
    }
}

/// State for the rp-backed surfaces (`/equipment`, `/stream`, roster-derived
/// config pages), present when the BFF config carries an `rp` target.
pub struct RpState {
    /// rp's config over REST — shared with the `/config/rp` `DriverHandle`.
    pub(crate) config_client: Arc<dyn ConfigClient>,
    /// rp's non-config REST surface (equipment status, session status).
    pub(crate) api: Arc<dyn rp_client::RpApi>,
    /// Bounded-timeout prober for the roster's capability tiers.
    pub(crate) probe_http: Arc<dyn probe::ProbeHttp>,
    /// The CA roster-derived device clients trust (the rp target's — one
    /// observatory, one CA).
    pub(crate) ca_cert_path: Option<std::path::PathBuf>,
    /// rp's base URL, for the SSE proxy's subscribe URL and error banners.
    pub(crate) base_url: String,
    /// A raw `reqwest` client for the SSE proxy's long-lived streaming GET —
    /// the buffered [`HttpClient`] seam can't stream. CA-trusting; `stream_auth`
    /// carries the Basic credentials to present.
    pub(crate) stream_client: reqwest::Client,
    pub(crate) stream_auth: Option<(String, String)>,
}

/// Shared handler state: every configured driver, keyed by service id, plus
/// the optional rp-backed surface state and the optional Sentinel restart
/// client the restart affordances call.
#[derive(Clone)]
pub struct AppState {
    drivers: Arc<BTreeMap<String, DriverHandle>>,
    rp: Option<Arc<RpState>>,
    sentinel: Option<Arc<dyn SentinelClient>>,
    /// Ends open SSE proxy streams on service shutdown — axum's graceful
    /// shutdown does not close them on its own (axum #2673); `main` links this
    /// to the `ServiceRunner` shutdown (the same pattern as rp's `sse_shutdown`).
    sse_shutdown: tokio_util::sync::CancellationToken,
}

impl AppState {
    /// The rp-backed surface state, when an `rp` target is configured.
    pub(crate) fn rp(&self) -> Option<&Arc<RpState>> {
        self.rp.as_ref()
    }

    /// The token that ends open SSE streams on shutdown.
    pub(crate) fn sse_shutdown(&self) -> &tokio_util::sync::CancellationToken {
        &self.sse_shutdown
    }

    /// Link the SSE shutdown token to the service lifecycle (called by `main`;
    /// the constructors default to a fresh token so tests need no wiring).
    #[must_use]
    pub fn with_sse_shutdown(mut self, token: tokio_util::sync::CancellationToken) -> Self {
        self.sse_shutdown = token;
        self
    }
}

/// Validate a configured base URL and build the `reqwest`-backed `HttpClient`
/// for it (CA trust + optional Basic auth). Rejects credentials embedded in the
/// URL (`http://user:pass@host`) — they would otherwise leak into error messages
/// (rendered in the page) and debug logs that echo the request URL; credentials
/// belong in the `auth` field, sent as a redactable `Authorization` header.
fn build_http_client(
    what: &str,
    base_url: &str,
    auth: Option<&config::DriverAuth>,
    ca_cert_path: Option<&std::path::Path>,
) -> Result<Arc<dyn HttpClient>, Box<dyn std::error::Error + Send + Sync>> {
    // Deliberately no `{base_url}` echo: a malformed URL can carry embedded
    // credentials, and this message reaches rendered error cards (e.g. the
    // unusable-roster-entry card) and logs. `what` names the target; the
    // parse error says why; the operator has the URL in their own config.
    let parsed =
        reqwest::Url::parse(base_url).map_err(|e| format!("invalid base_url for {what}: {e}"))?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(format!(
            "{what} base_url must not contain credentials \
             (user:pass@…); put them in the `auth` field instead"
        )
        .into());
    }
    let reqwest_client = match auth {
        Some(auth) => ReqwestHttpClient::with_auth(
            ca_cert_path,
            auth.username.clone(),
            auth.password.clone(),
        )?,
        None => ReqwestHttpClient::new(ca_cert_path)?,
    };
    Ok(Arc::new(reqwest_client))
}

/// The reserved service id for rp's own config page (`/config/rp`).
const RP_SERVICE: &str = "rp";

impl AppState {
    /// Build the production state: an `AlpacaConfigClient` over a `reqwest`-backed
    /// `HttpClient` (CA trust + optional Basic auth) for every configured driver,
    /// plus — when an `rp` target is configured — a `RestConfigClient` under the
    /// reserved `rp` key for rp's own config page, plus an `HttpSentinelClient`
    /// when a `sentinel` block is configured.
    pub fn from_config(config: &Config) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let sentinel: Option<Arc<dyn SentinelClient>> = match &config.sentinel {
            Some(target) => {
                let http = build_http_client(
                    "sentinel",
                    &target.base_url,
                    target.auth.as_ref(),
                    target.ca_cert_path.as_deref(),
                )?;
                Some(Arc::new(HttpSentinelClient::new(http, &target.base_url)))
            }
            None => None,
        };

        let mut drivers = BTreeMap::new();
        for (service, target) in &config.drivers.0 {
            // `rp` is reserved for the rp target so `/config/rp` is unambiguous.
            if service == RP_SERVICE {
                return Err(format!(
                    "the drivers map must not contain an entry named {RP_SERVICE:?}; \
                     configure the rp orchestrator via the top-level `rp` target instead"
                )
                .into());
            }
            let http = build_http_client(
                &format!("driver {service:?}"),
                &target.base_url,
                target.auth.as_ref(),
                target.ca_cert_path.as_deref(),
            )?;
            let client = AlpacaConfigClient::new(
                http,
                &target.base_url,
                &target.device_type,
                target.device_number,
            );
            // The restart affordances render only with a Sentinel to call;
            // the Sentinel-side name defaults to the driver's own service id.
            let sentinel_service = sentinel.as_ref().map(|_| {
                target
                    .sentinel_service
                    .clone()
                    .unwrap_or_else(|| service.clone())
            });
            drivers.insert(
                service.clone(),
                DriverHandle {
                    title: target.name.clone().unwrap_or_else(|| service.clone()),
                    subtitle: format!("{service} · {}", target.device_type),
                    client: Arc::new(client),
                    sentinel_service,
                },
            );
        }
        let mut rp_state = None;
        if let Some(rp) = &config.rp {
            let http = build_http_client(
                "rp",
                &rp.base_url,
                rp.auth.as_ref(),
                rp.ca_cert_path.as_deref(),
            )?;
            let config_client: Arc<dyn ConfigClient> =
                Arc::new(RestConfigClient::new(Arc::clone(&http), &rp.base_url));
            drivers.insert(
                RP_SERVICE.to_string(),
                DriverHandle {
                    title: "rp".to_string(),
                    subtitle: "rp · orchestrator (REST)".to_string(),
                    client: Arc::clone(&config_client),
                    // rp has no in-process reload — every apply is
                    // restart-required — so the Sentinel affordance matters
                    // most here. Sentinel-side name: the `rp` convention.
                    sentinel_service: sentinel.as_ref().map(|_| RP_SERVICE.to_string()),
                },
            );
            rp_state = Some(Arc::new(RpState {
                api: Arc::new(rp_client::RestRpApi::new(Arc::clone(&http), &rp.base_url)),
                config_client,
                probe_http: Arc::new(
                    probe::ReqwestProbeHttp::new(rp.ca_cert_path.as_deref())
                        .map_err(|e| format!("rp target: {e}"))?,
                ),
                ca_cert_path: rp.ca_cert_path.clone(),
                base_url: rp.base_url.clone(),
                stream_client: rp_tls::client::build_reqwest_client(rp.ca_cert_path.as_deref())
                    .map_err(|e| format!("rp target: failed to build stream client: {e}"))?,
                stream_auth: rp
                    .auth
                    .as_ref()
                    .map(|a| (a.username.clone(), a.password.clone())),
            }));
        }
        Ok(Self {
            drivers: Arc::new(drivers),
            rp: rp_state,
            sentinel,
            sse_shutdown: tokio_util::sync::CancellationToken::new(),
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
                sentinel_service: None,
            },
        );
        Self {
            drivers: Arc::new(drivers),
            rp: None,
            sentinel: None,
            sse_shutdown: tokio_util::sync::CancellationToken::new(),
        }
    }

    /// [`AppState::with_client`] plus a Sentinel client (tests inject stubs for
    /// both), with the Sentinel-side name defaulting to the service id.
    pub fn with_client_and_sentinel(
        service: &str,
        client: Arc<dyn ConfigClient>,
        sentinel: Arc<dyn SentinelClient>,
    ) -> Self {
        let mut drivers = BTreeMap::new();
        drivers.insert(
            service.to_string(),
            DriverHandle {
                title: service.to_string(),
                subtitle: service.to_string(),
                client,
                sentinel_service: Some(service.to_string()),
            },
        );
        Self {
            drivers: Arc::new(drivers),
            rp: None,
            sentinel: Some(sentinel),
            sse_shutdown: tokio_util::sync::CancellationToken::new(),
        }
    }

    /// Build rp-only state from explicit parts (tests inject stubs for the
    /// equipment / stream handlers). The `rp` config-page handle is wired to
    /// the same `config_client`, mirroring production.
    pub fn with_rp_parts(
        config_client: Arc<dyn ConfigClient>,
        api: Arc<dyn rp_client::RpApi>,
        probe_http: Arc<dyn probe::ProbeHttp>,
    ) -> Self {
        let mut drivers = BTreeMap::new();
        drivers.insert(
            RP_SERVICE.to_string(),
            DriverHandle {
                title: "rp".to_string(),
                subtitle: "rp · orchestrator (REST)".to_string(),
                client: Arc::clone(&config_client),
                sentinel_service: None,
            },
        );
        Self {
            drivers: Arc::new(drivers),
            sentinel: None,
            rp: Some(Arc::new(RpState {
                config_client,
                api,
                probe_http,
                ca_cert_path: None,
                base_url: "http://rp.test".to_string(),
                stream_client: reqwest::Client::new(),
                stream_auth: None,
            })),
            sse_shutdown: tokio_util::sync::CancellationToken::new(),
        }
    }

    /// Point the test state's SSE proxy at an explicit rp base URL (unit tests
    /// stub rp's subscribe endpoint with an in-test axum server — ADR-004).
    #[must_use]
    pub fn with_rp_base_url(mut self, base_url: &str) -> Self {
        if let Some(rp) = self.rp.take() {
            // The Arc is freshly built by `with_rp_parts`, so this unwrap-free
            // rebuild keeps the test constructor simple.
            let rp = Arc::new(RpState {
                config_client: Arc::clone(&rp.config_client),
                api: Arc::clone(&rp.api),
                probe_http: Arc::clone(&rp.probe_http),
                ca_cert_path: rp.ca_cert_path.clone(),
                base_url: base_url.trim_end_matches('/').to_string(),
                stream_client: rp.stream_client.clone(),
                stream_auth: rp.stream_auth.clone(),
            });
            self.rp = Some(rp);
        }
        self
    }
}

/// Why [`resolve_service`] found no usable driver behind a `/config/{service}`
/// key. Each cause renders its own honest card ([`pages::resolve_failure_card`])
/// — an unreachable rp or an unusable roster entry must not masquerade as
/// "no such driver".
#[derive(Debug)]
pub(crate) enum ResolveError {
    /// No static entry, and the key names nothing in the roster (also: not a
    /// roster key at all, or no rp target is configured).
    Unknown,
    /// The key names a roster device but rp's config could not be fetched.
    RpUnreachable(String),
    /// The roster entry exists but no client could be built from it (e.g. a
    /// malformed or credentialed `alpaca_url`).
    BadRosterEntry(String),
}

/// Resolve a `/config/{service}` key to its driver handle: a static entry from
/// the config's `drivers` map (or the reserved `rp` entry), else a
/// **roster-derived** `rp:{kind}:{id}` target synthesized from rp's config —
/// the device's `alpaca_url`/number from its roster entry, the ASCOM type from
/// its kind, called without credentials (rp redacts per-device auth; an authed
/// device needs its own static `drivers` entry).
async fn resolve_service(state: &AppState, service: &str) -> Result<DriverHandle, ResolveError> {
    if let Some(handle) = state.drivers.get(service) {
        return Ok(handle.clone());
    }
    let (kind, id) = roster::parse_service_key(service).ok_or(ResolveError::Unknown)?;
    let rp = state.rp().ok_or(ResolveError::Unknown)?;
    let resp = rp
        .config_client
        .get_config()
        .await
        .map_err(|e| ResolveError::RpUnreachable(e.to_string()))?;
    let entry = roster::find_entry(&resp.config, kind, id).ok_or(ResolveError::Unknown)?;
    let http = build_http_client(
        "roster device",
        &entry.alpaca_url,
        None,
        rp.ca_cert_path.as_deref(),
    )
    .map_err(|e| {
        tracing::debug!("roster-derived client for {service} failed: {e}");
        ResolveError::BadRosterEntry(e.to_string())
    })?;
    let client = AlpacaConfigClient::new(
        http,
        &entry.alpaca_url,
        kind.ascom_type(),
        entry.device_number,
    );
    Ok(DriverHandle {
        title: entry.display_name().to_string(),
        subtitle: format!("{} · {} (via rp roster)", entry.id, kind.ascom_type()),
        client: Arc::new(client),
        // Roster devices are hardware rp manages, not OS services Sentinel
        // supervises — no restart affordance.
        sentinel_service: None,
    })
}

/// Build the BFF axum router.
pub fn build_router(state: AppState) -> Router {
    let router = Router::new()
        .route("/", get(index))
        .route("/config/{service}", get(config_get).post(config_post))
        .route("/config/{service}/status", get(config_status))
        .route(
            "/config/{service}/restart",
            axum::routing::post(config_restart),
        )
        .route("/equipment", get(pages::equipment::page))
        .route(
            "/equipment/{kind}/new",
            get(pages::equipment::new_form).post(pages::equipment::new_submit),
        )
        .route(
            "/equipment/{kind}/{id}/edit",
            get(pages::equipment::edit_form).post(pages::equipment::edit_submit),
        )
        .route(
            "/equipment/{kind}/{id}/delete",
            axum::routing::post(pages::equipment::delete),
        )
        .route("/stream", get(pages::stream::page))
        .route("/stream/events", get(sse_proxy::events))
        .route("/stream/equipment", get(pages::stream::equipment_fragment))
        .route("/health", get(health))
        .route("/assets/app.css", get(assets::app_css))
        .route("/assets/htmx.min.js", get(assets::htmx_js))
        .route("/assets/htmx-ext-sse.js", get(assets::htmx_sse_js));
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
    let roster = match state.rp() {
        None => pages::RosterLinks::NotConfigured,
        Some(rp) => match rp.config_client.get_config().await {
            Ok(resp) => pages::RosterLinks::Entries(
                roster::parse_roster(&resp.config)
                    .iter()
                    .map(|entry| DriverLink {
                        service: entry.service_key(),
                        title: entry.display_name().to_string(),
                    })
                    .collect(),
            ),
            Err(err) => pages::RosterLinks::Unreachable(err.to_string()),
        },
    };
    pages::index_page(&links, &roster)
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
    let handle = match resolve_service(&state, &service).await {
        Ok(handle) => handle,
        Err(err) => {
            return respond(
                pages::resolve_failure_card(&service, &err),
                &headers,
                &title,
            )
        }
    };
    let card = render_form(&handle, &service, query.unlock.as_deref(), None).await;
    respond(card, &headers, &title)
}

async fn config_post(
    State(state): State<AppState>,
    Path(service): Path<String>,
    headers: HeaderMap,
    // Pairs, not a map: a checkbox group posts one pair per checked box
    // and `serde_urlencoded` would collapse duplicate keys in a map.
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    let form = pages::FormValues::from(form);
    let title = page_title(&service);
    let handle = match resolve_service(&state, &service).await {
        Ok(handle) => handle,
        Err(err) => {
            return respond(
                pages::resolve_failure_card(&service, &err),
                &headers,
                &title,
            )
        }
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
                // submitted values, also re-locked. A populated
                // `restart_required[]` (an `ApplyDisposition::Restart` target —
                // rp) selects the restart callout over the plain saved banner.
                ApplyStatus::Ok => {
                    let banner = if resp.restart_required.is_empty() {
                        Banner::Saved
                    } else {
                        Banner::SavedRestartRequired(resp.restart_required)
                    };
                    match handle.client.get_config().await {
                        Ok(fresh) => pages::config_card(
                            &page,
                            &model,
                            &fresh.config,
                            &fresh.overrides,
                            &[],
                            &[],
                            Some(banner),
                        ),
                        Err(_) => pages::config_card(
                            &page,
                            &model,
                            &merged.config,
                            &merged.overrides,
                            &[],
                            &[],
                            Some(banner),
                        ),
                    }
                }
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
    let Ok(handle) = resolve_service(&state, &service).await else {
        // Any resolve failure mid-poll: a benign reconnecting fragment keeps the
        // page from erroring; the user can navigate away.
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

/// Ask Sentinel to restart the driver's process (the "Restart via Sentinel"
/// affordances post here), then render the outcome: an accepted restart swaps
/// in the reconnect-polling fragment; everything else is an error card naming
/// the reason. See `docs/services/ui-htmx.md` §Restart via Sentinel.
async fn config_restart(
    State(state): State<AppState>,
    Path(service): Path<String>,
    headers: HeaderMap,
) -> Response {
    let title = page_title(&service);
    let Some(handle) = state.drivers.get(&service) else {
        return respond(pages::unknown_service_card(&service), &headers, &title);
    };
    let (Some(sentinel), Some(name)) = (&state.sentinel, &handle.sentinel_service) else {
        // The affordances that post here only render with a Sentinel
        // configured, so this answers only hand-crafted requests.
        return respond(
            pages::message_error_card(
                &service,
                "No Sentinel is configured, so the BFF cannot restart this driver.",
            ),
            &headers,
            &title,
        );
    };
    let card = match sentinel.restart(name).await {
        Ok(outcome) if outcome.is_ok() => {
            pages::restarting_card(&service, outcome.recovery_timed_out())
        }
        Ok(outcome) => pages::message_error_card(
            &service,
            &format!(
                "Sentinel could not restart the driver: {}",
                outcome
                    .detail
                    .as_deref()
                    .unwrap_or("the restart command failed")
            ),
        ),
        Err(err) => pages::message_error_card(&service, &err.to_string()),
    };
    respond(card, &headers, &title)
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

    #[test]
    fn from_config_rejects_sentinel_url_credentials() {
        // The sentinel target goes through the same `build_http_client`
        // validation as drivers/rp: embedded credentials would leak into
        // error strings and request-URL debug logs.
        let json = r#"{ "sentinel": { "base_url": "http://obs:secret@127.0.0.1:11114" } }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        match AppState::from_config(&config) {
            Ok(_) => panic!("expected from_config to reject credentials in sentinel base_url"),
            Err(e) => assert!(
                e.to_string().contains("must not contain credentials"),
                "{e}"
            ),
        }
    }

    #[test]
    fn from_config_rejects_a_driver_named_rp() {
        // `rp` is reserved for the rp target so `/config/rp` stays unambiguous.
        let json = r#"{ "drivers": { "rp": { "base_url": "http://127.0.0.1:11115" } } }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        match AppState::from_config(&config) {
            Ok(_) => panic!("expected from_config to reject a driver named rp"),
            Err(e) => assert!(
                e.to_string()
                    .contains(r#"must not contain an entry named "rp""#),
                "{e}"
            ),
        }
    }

    #[test]
    fn from_config_with_rp_target_serves_the_rp_config_page() {
        let json = r#"{ "rp": { "base_url": "http://127.0.0.1:11115" } }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        let state = AppState::from_config(&config).unwrap();
        let rp = state.drivers.get("rp").unwrap();
        assert_eq!(rp.title, "rp");
        assert_eq!(rp.subtitle, "rp · orchestrator (REST)");
        // The default drivers map is still there alongside the rp entry.
        assert!(state.drivers.contains_key("dsd-fp2"));
    }

    #[test]
    fn from_config_rejects_rp_url_credentials() {
        let json = r#"{ "rp": { "base_url": "http://obs:secret@127.0.0.1:11115" } }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        match AppState::from_config(&config) {
            Ok(_) => panic!("expected from_config to reject credentials in the rp base_url"),
            Err(e) => assert!(
                e.to_string().contains("must not contain credentials"),
                "{e}"
            ),
        }
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

    fn rp_state_with_config_client(config_client: Arc<dyn ConfigClient>) -> AppState {
        AppState::with_rp_parts(
            config_client,
            Arc::new(rp_client::MockRpApi::new()),
            Arc::new(probe::MockProbeHttp::new()),
        )
    }

    #[tokio::test]
    async fn config_get_roster_key_with_unreachable_rp_says_so() {
        // `NonConfigDriver::get_config` errors — resolving a roster key must
        // say rp could not be read, not pretend the driver is unconfigured.
        let state = rp_state_with_config_client(Arc::new(NonConfigDriver));
        let response = config_get(
            State(state),
            Path("rp:cameras:main-cam".to_string()),
            Query(UnlockQuery::default()),
            HeaderMap::new(),
        )
        .await;
        let html = body_of(response).await;
        assert!(html.contains("Could not read rp's roster"), "{html}");
        assert!(!html.contains("No configured driver named"), "{html}");
    }

    /// A roster whose one entry has a credentialed `alpaca_url` —
    /// [`build_http_client`] rejects it deterministically, no network involved.
    struct BadEntryRoster;

    #[async_trait::async_trait]
    impl ConfigClient for BadEntryRoster {
        async fn get_config(&self) -> Result<ConfigGetResponse, ConfigClientError> {
            Ok(ConfigGetResponse {
                config: json!({ "equipment": { "cover_calibrators": [{
                    "id": "flat",
                    "alpaca_url": "http://obs:secret@127.0.0.1:11119",
                    "device_number": 0
                }]}}),
                overrides: vec![],
            })
        }
        async fn get_schema(&self) -> Result<ConfigSchemaResponse, ConfigClientError> {
            unreachable!("resolve fails before any schema fetch")
        }
        async fn apply_config(
            &self,
            _config: &Value,
        ) -> Result<ConfigApplyResponse, ConfigClientError> {
            unreachable!("apply is not exercised by this test")
        }
    }

    #[tokio::test]
    async fn config_get_unusable_roster_entry_points_at_the_equipment_page() {
        let state = rp_state_with_config_client(Arc::new(BadEntryRoster));
        let response = config_get(
            State(state),
            Path("rp:cover_calibrators:flat".to_string()),
            Query(UnlockQuery::default()),
            HeaderMap::new(),
        )
        .await;
        let html = body_of(response).await;
        assert!(html.contains("roster entry can't be used"), "{html}");
        assert!(html.contains(r#"href="/equipment""#), "{html}");
        assert!(!html.contains("No configured driver named"), "{html}");
    }

    /// A roster entry whose `alpaca_url` is malformed AND carries embedded
    /// credentials — `Url::parse` fails before the credential check can.
    struct MalformedUrlRoster;

    #[async_trait::async_trait]
    impl ConfigClient for MalformedUrlRoster {
        async fn get_config(&self) -> Result<ConfigGetResponse, ConfigClientError> {
            Ok(ConfigGetResponse {
                config: json!({ "equipment": { "cover_calibrators": [{
                    "id": "flat",
                    "alpaca_url": "http://obs:hunter2@[oops",
                    "device_number": 0
                }]}}),
                overrides: vec![],
            })
        }
        async fn get_schema(&self) -> Result<ConfigSchemaResponse, ConfigClientError> {
            unreachable!("resolve fails before any schema fetch")
        }
        async fn apply_config(
            &self,
            _config: &Value,
        ) -> Result<ConfigApplyResponse, ConfigClientError> {
            unreachable!("apply is not exercised by this test")
        }
    }

    #[tokio::test]
    async fn config_get_roster_key_without_rp_target_is_unknown() {
        // A roster-shaped key on a BFF with no rp target: nothing to resolve
        // against — the plain unknown-driver card, not an rp error.
        let state = AppState::with_client("dsd-fp2", Arc::new(NonConfigDriver));
        let response = config_get(
            State(state),
            Path("rp:cameras:main-cam".to_string()),
            Query(UnlockQuery::default()),
            HeaderMap::new(),
        )
        .await;
        let html = body_of(response).await;
        assert!(html.contains("No configured driver named"), "{html}");
    }

    #[tokio::test]
    async fn config_get_roster_key_absent_from_roster_is_unknown() {
        // rp answers, but no entry matches the id — honestly "no such driver".
        let state = rp_state_with_config_client(Arc::new(BadEntryRoster));
        let response = config_get(
            State(state),
            Path("rp:cover_calibrators:no-such-id".to_string()),
            Query(UnlockQuery::default()),
            HeaderMap::new(),
        )
        .await;
        let html = body_of(response).await;
        assert!(html.contains("No configured driver named"), "{html}");
    }

    #[tokio::test]
    async fn config_post_resolve_failure_renders_the_same_cards() {
        // The POST path shares resolve_service — its error arm must render
        // the same honest card as GET.
        let state = rp_state_with_config_client(Arc::new(NonConfigDriver));
        let response = config_post(
            State(state),
            Path("rp:cameras:main-cam".to_string()),
            HeaderMap::new(),
            Form(Vec::new()),
        )
        .await;
        let html = body_of(response).await;
        assert!(html.contains("Could not read rp's roster"), "{html}");
    }

    #[tokio::test]
    async fn config_status_resolve_failure_stays_a_benign_reconnect() {
        // Mid-poll resolve failures keep the page politely retrying rather
        // than flashing an error card.
        let state = rp_state_with_config_client(Arc::new(NonConfigDriver));
        let markup = config_status(
            State(state),
            Path("rp:cameras:main-cam".to_string()),
            Query(UnlockQuery::default()),
        )
        .await;
        assert!(markup.into_string().contains("Reconnecting"));
    }

    #[tokio::test]
    async fn unusable_roster_entry_card_never_echoes_url_credentials() {
        // The URL-parse failure path must not leak the raw URL (it can carry
        // embedded credentials) into the rendered card.
        let state = rp_state_with_config_client(Arc::new(MalformedUrlRoster));
        let response = config_get(
            State(state),
            Path("rp:cover_calibrators:flat".to_string()),
            Query(UnlockQuery::default()),
            HeaderMap::new(),
        )
        .await;
        let html = body_of(response).await;
        assert!(html.contains("roster entry can't be used"), "{html}");
        assert!(!html.contains("hunter2"), "{html}");
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

    /// A `ConfigClient` for an `ApplyDisposition::Restart` target (rp): apply
    /// persists and reports the changed paths in `restart_required[]`.
    struct RestartingDriver;

    #[async_trait::async_trait]
    impl ConfigClient for RestartingDriver {
        async fn get_config(&self) -> Result<ConfigGetResponse, ConfigClientError> {
            Ok(ConfigGetResponse {
                config: json!({ "site": { "latitude_degrees": 47.6 } }),
                overrides: vec![],
            })
        }
        async fn get_schema(&self) -> Result<ConfigSchemaResponse, ConfigClientError> {
            Ok(ConfigSchemaResponse {
                schema: json!({
                    "type": "object",
                    "properties": {
                        "site": { "type": "object", "properties": {
                            "latitude_degrees": { "type": "number" }
                        }}
                    }
                }),
                locked_fields: vec![],
                read_only_fields: vec![],
            })
        }
        async fn apply_config(
            &self,
            _config: &Value,
        ) -> Result<ConfigApplyResponse, ConfigClientError> {
            Ok(ConfigApplyResponse {
                status: ApplyStatus::Ok,
                applied: vec![],
                reload: vec![],
                restart_required: vec!["site.latitude_degrees".to_string()],
                skipped_override: vec![],
                persisted_to: Some("/tmp/rp.json".to_string()),
                errors: vec![],
            })
        }
    }

    #[tokio::test]
    async fn config_post_renders_the_restart_callout() {
        let state = AppState::with_client("rp", Arc::new(RestartingDriver));
        let form: Vec<(String, String)> = vec![
            (
                "__config".to_string(),
                r#"{"site":{"latitude_degrees":47.6}}"#.to_string(),
            ),
            ("__overrides".to_string(), "[]".to_string()),
            ("site.latitude_degrees".to_string(), "48.1".to_string()),
        ];

        let response = config_post(
            State(state),
            Path("rp".to_string()),
            HeaderMap::new(),
            Form(form),
        )
        .await;
        let html = body_of(response).await;
        assert!(
            html.contains("take effect when rp is restarted"),
            "missing restart callout:\n{html}"
        );
        assert!(html.contains("site.latitude_degrees"), "{html}");
        assert!(html.contains("banner warn"), "{html}");
    }

    // --- Restart via Sentinel (config-actions plan Phase 4) -----------------

    /// A `SentinelClient` returning a canned outcome, recording the Sentinel-
    /// side service name it was asked to restart.
    struct StubSentinel {
        result: Result<RestartOutcome, SentinelClientError>,
        last_service: std::sync::Mutex<Option<String>>,
    }

    impl StubSentinel {
        fn new(result: Result<RestartOutcome, SentinelClientError>) -> Arc<Self> {
            Arc::new(Self {
                result,
                last_service: std::sync::Mutex::new(None),
            })
        }
    }

    #[async_trait::async_trait]
    impl SentinelClient for StubSentinel {
        async fn restart(&self, service: &str) -> Result<RestartOutcome, SentinelClientError> {
            *self.last_service.lock().unwrap() = Some(service.to_string());
            self.result.clone()
        }
    }

    fn outcome(status: &str, recovery: Option<&str>, detail: Option<&str>) -> RestartOutcome {
        RestartOutcome {
            status: status.to_string(),
            recovery: recovery.map(String::from),
            detail: detail.map(String::from),
        }
    }

    async fn post_restart(state: AppState) -> String {
        let response =
            config_restart(State(state), Path("dsd-fp2".to_string()), HeaderMap::new()).await;
        body_of(response).await
    }

    #[tokio::test]
    async fn config_get_offers_restart_only_with_sentinel_configured() {
        let without = AppState::with_client("dsd-fp2", Arc::new(StaticConfigDriver));
        let html = body_of(
            config_get(
                State(without),
                Path("dsd-fp2".to_string()),
                Query(UnlockQuery::default()),
                HeaderMap::new(),
            )
            .await,
        )
        .await;
        assert!(
            !html.contains("restart-sentinel"),
            "restart affordance rendered with no sentinel configured:\n{html}"
        );

        let sentinel = StubSentinel::new(Ok(outcome("ok", Some("healthy"), None)));
        let with =
            AppState::with_client_and_sentinel("dsd-fp2", Arc::new(StaticConfigDriver), sentinel);
        let html = body_of(
            config_get(
                State(with),
                Path("dsd-fp2".to_string()),
                Query(UnlockQuery::default()),
                HeaderMap::new(),
            )
            .await,
        )
        .await;
        assert!(
            html.contains(r#"hx-post="/config/dsd-fp2/restart""#),
            "missing restart affordance:\n{html}"
        );
    }

    #[tokio::test]
    async fn config_restart_without_sentinel_is_an_error_card() {
        let state = AppState::with_client("dsd-fp2", Arc::new(StaticConfigDriver));
        let html = post_restart(state).await;
        assert!(html.contains("No Sentinel is configured"), "{html}");
    }

    #[tokio::test]
    async fn config_restart_accepted_renders_reconnect_poll() {
        let sentinel = StubSentinel::new(Ok(outcome("ok", Some("healthy"), None)));
        let state = AppState::with_client_and_sentinel(
            "dsd-fp2",
            Arc::new(StaticConfigDriver),
            // Method-call clone: `Arc::clone(&sentinel)` would infer
            // `T = dyn SentinelClient` from the parameter and fail on the
            // concrete `&Arc<StubSentinel>`.
            sentinel.clone(),
        );
        let html = post_restart(state).await;
        assert!(html.contains("Restart requested via Sentinel"), "{html}");
        assert!(
            html.contains(r#"hx-get="/config/dsd-fp2/status""#),
            "restarting card must poll the status route:\n{html}"
        );
        assert_eq!(
            sentinel.last_service.lock().unwrap().as_deref(),
            Some("dsd-fp2"),
            "the Sentinel-side name defaults to the service id"
        );
    }

    #[tokio::test]
    async fn config_restart_recovery_timeout_warns() {
        let sentinel = StubSentinel::new(Ok(outcome("ok", Some("timeout"), None)));
        let state =
            AppState::with_client_and_sentinel("dsd-fp2", Arc::new(StaticConfigDriver), sentinel);
        let html = post_restart(state).await;
        assert!(
            html.contains("did not confirm recovery"),
            "missing recovery-timeout warning:\n{html}"
        );
        assert!(
            html.contains(r#"hx-get="/config/dsd-fp2/status""#),
            "the poll still runs — the budget is Sentinel's, not the driver's:\n{html}"
        );
    }

    #[tokio::test]
    async fn config_restart_failed_command_shows_detail() {
        let sentinel = StubSentinel::new(Ok(outcome(
            "failed",
            None,
            Some("restart `x` exited with 1"),
        )));
        let state =
            AppState::with_client_and_sentinel("dsd-fp2", Arc::new(StaticConfigDriver), sentinel);
        let html = post_restart(state).await;
        assert!(
            html.contains("could not restart the driver: restart `x` exited with 1"),
            "{html}"
        );
    }

    #[tokio::test]
    async fn config_restart_unsupervised_names_the_reason() {
        let sentinel = StubSentinel::new(Err(SentinelClientError::UnknownService(
            "no configured service named 'dsd-fp2'".to_string(),
        )));
        let state =
            AppState::with_client_and_sentinel("dsd-fp2", Arc::new(StaticConfigDriver), sentinel);
        let html = post_restart(state).await;
        assert!(html.contains("does not supervise"), "{html}");
        assert!(html.contains("no configured service named"), "{html}");
    }

    /// Renders like [`StaticConfigDriver`]; `config.apply` reports the change
    /// persisted but needing a process restart — the classification no real
    /// driver emits today, so this arm is unit-driven.
    struct RestartRequiredDriver;

    #[async_trait::async_trait]
    impl ConfigClient for RestartRequiredDriver {
        async fn get_config(&self) -> Result<ConfigGetResponse, ConfigClientError> {
            StaticConfigDriver.get_config().await
        }
        async fn get_schema(&self) -> Result<ConfigSchemaResponse, ConfigClientError> {
            StaticConfigDriver.get_schema().await
        }
        async fn apply_config(
            &self,
            _config: &Value,
        ) -> Result<ConfigApplyResponse, ConfigClientError> {
            Ok(ConfigApplyResponse {
                status: ApplyStatus::Ok,
                applied: Vec::new(),
                reload: Vec::new(),
                restart_required: vec!["server.port".to_string()],
                skipped_override: Vec::new(),
                persisted_to: None,
                errors: Vec::new(),
            })
        }
    }

    /// Submit the static driver's own config back through `config_post` (the
    /// hidden blobs plus every enabled field, as a browser would send them).
    async fn submit_static_form(state: AppState) -> String {
        let config = StaticConfigDriver.get_config().await.unwrap().config;
        let mut form: Vec<(String, String)> = vec![
            (
                "__config".to_string(),
                serde_json::to_string(&config).unwrap(),
            ),
            ("__overrides".to_string(), "[]".to_string()),
            ("__unlocked".to_string(), "[]".to_string()),
        ];
        for (name, value) in [
            ("serial.port", "/dev/ttyACM0"),
            ("serial.baud_rate", "115200"),
            ("server.discovery_port", "32227"),
            ("cover_calibrator.name", "FP2"),
            ("cover_calibrator.max_brightness", "4096"),
        ] {
            form.push((name.to_string(), value.to_string()));
        }
        let response = config_post(
            State(state),
            Path("dsd-fp2".to_string()),
            HeaderMap::new(),
            Form(form),
        )
        .await;
        body_of(response).await
    }

    #[tokio::test]
    async fn apply_with_restart_required_escalates_to_sentinel() {
        let sentinel = StubSentinel::new(Ok(outcome("ok", Some("skipped"), None)));
        let state = AppState::with_client_and_sentinel(
            "dsd-fp2",
            Arc::new(RestartRequiredDriver),
            sentinel,
        );
        let html = submit_static_form(state).await;
        assert!(
            html.contains("take effect when dsd-fp2 is restarted"),
            "missing restart callout:\n{html}"
        );
        assert!(html.contains("server.port"), "{html}");
        assert!(
            html.contains(r#"hx-post="/config/dsd-fp2/restart""#),
            "the callout must offer the Sentinel restart:\n{html}"
        );
    }

    #[tokio::test]
    async fn apply_with_restart_required_without_sentinel_has_no_button() {
        let state = AppState::with_client("dsd-fp2", Arc::new(RestartRequiredDriver));
        let html = submit_static_form(state).await;
        assert!(
            html.contains("take effect when dsd-fp2 is restarted"),
            "missing restart callout:\n{html}"
        );
        assert!(
            !html.contains("restart-sentinel"),
            "no restart button without a sentinel:\n{html}"
        );
    }
}
