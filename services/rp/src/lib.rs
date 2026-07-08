#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
pub mod config;
pub mod equipment;
pub mod error;
pub mod events;
pub mod hash_password_cmd;
pub mod imaging;
pub mod mcp;
pub mod persistence;
pub mod planner;
pub mod routes;
pub mod safety;
pub mod session;
pub mod tls_cmd;

use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tracing::{debug, info};

use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rp_tls::config::TlsConfig;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::equipment::EquipmentRegistry;
use crate::error::Result;
use crate::events::EventBus;
use crate::mcp::McpHandler;
use crate::persistence::ImageCache;
use crate::routes::{build_router, AppState};
use crate::safety::{AlpacaSafetyProbe, SafetyEnforcer};
use crate::session::{SessionConfig, SessionManager};

/// Builder for the rp server.
///
/// Configures equipment, event bus, session manager, and MCP handler,
/// then binds the server. The returned [`BoundServer`] can be inspected
/// (e.g. `listen_addr()`) before calling `start()`.
pub struct ServerBuilder {
    config: Option<Config>,
}

impl ServerBuilder {
    pub fn new() -> Self {
        Self { config: None }
    }

    pub fn with_config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    pub async fn build(self) -> Result<BoundServer> {
        let config = self.config.ok_or_else(|| {
            crate::error::RpError::Config(
                "ServerBuilder::build: config is required \u{2014} call .with_config(...) first"
                    .to_string(),
            )
        })?;
        let bind_addr = format!("{}:{}", config.server.bind_address, config.server.port);

        debug!("initializing equipment registry");
        let equipment = Arc::new(EquipmentRegistry::new(&config.equipment).await);

        // Validate the configured site against the mount's reported
        // SiteLatitude/SiteLongitude. A mismatch beyond 0.01° aborts
        // startup — see docs/services/rp.md §"Site Validation Against
        // the ASCOM Mount". When site config is omitted, the mount
        // lacks the property, or no mount is configured, this is a
        // debug-logged no-op.
        equipment.validate_site(config.site.as_ref()).await?;

        debug!("initializing event bus");
        let event_bus = Arc::new(EventBus::from_config(&config.plugins));

        debug!("initializing session manager");
        // The planner's record_exposure counters, shared between the
        // MCP handler (the tools read and write them) and the session
        // manager (a fresh session start clears them — a new
        // session_id is a new night).
        let planner_progress = Arc::new(std::sync::Mutex::new(
            crate::planner::progress::SessionProgress::default(),
        ));
        let session = Arc::new(
            SessionManager::new(event_bus.clone(), &config.plugins)
                .with_progress_store(planner_progress.clone()),
        );

        let session_config = SessionConfig {
            data_directory: config.session.data_directory.clone(),
        };

        let image_cache = ImageCache::new(
            config.imaging.cache_max_mib,
            config.imaging.cache_max_images,
            std::path::PathBuf::from(&config.session.data_directory),
        );

        // Build the observer site (if configured) once, here, so the
        // tzf-rs DefaultFinder is constructed exactly once per process
        // and the IANA timezone is logged on the same path that
        // populates McpHandler.
        let site = if let Some(site_cfg) = config.site.as_ref() {
            let site =
                rp_ephemeris::Site::new(site_cfg.latitude_degrees, site_cfg.longitude_degrees)
                    .map_err(|e| crate::error::RpError::Config(format!("site: {e}")))?;
            tracing::info!("{}", site);
            Some(site)
        } else {
            None
        };

        let targets = crate::planner::decision::parse_targets_from_value(&config.targets);
        // `planner.min_altitude_degrees` is the planner-wide default
        // floor for `get_next_target` (per-target overrides apply).
        // Range-validate at startup so a config typo (e.g. `200`) fails
        // loud rather than silently changing planner behaviour.
        let default_min_alt = match config
            .planner
            .get("min_altitude_degrees")
            .and_then(|v| v.as_f64())
        {
            Some(v) if (-90.0..=90.0).contains(&v) => v,
            Some(v) => {
                return Err(crate::error::RpError::Config(format!(
                    "planner.min_altitude_degrees must be in [-90, 90]; got {v}"
                )));
            }
            None => 20.0,
        };

        // Build the plate-solver HTTP client when the operator
        // configured one. Failure to build (e.g. invalid TLS bag in
        // reqwest's builder) aborts startup loud rather than
        // silently disabling the tool — same posture as Phase 6c-1
        // wrapper config validation.
        let (plate_solver_client, plate_solver_default_radius) = match &config.plate_solver {
            Some(ps_cfg) => {
                let client =
                    rp_plate_solver::PlateSolverClient::new(ps_cfg.url.clone(), ps_cfg.timeout)
                        .map_err(|e| {
                            crate::error::RpError::Config(format!(
                                "plate_solver: failed to build HTTP client: {e}"
                            ))
                        })?;
                let arc: Arc<dyn rp_plate_solver::PlateSolveClient> = Arc::new(client);
                (Some(arc), ps_cfg.default_search_radius_deg)
            }
            None => (None, None),
        };

        let mcp = McpHandler::new(
            equipment.clone(),
            event_bus.clone(),
            session_config,
            image_cache.clone(),
            site,
        )
        .with_planner_config(targets, default_min_alt)
        .with_progress_store(planner_progress)
        .with_plate_solver(plate_solver_client, plate_solver_default_radius)
        .with_centering_config(config.centering.clone());

        // Cancellation token for in-flight SSE streams
        // (`/api/events/subscribe`). Cloned into AppState so the handler can
        // end its stream, and stored on BoundServer so `start()` can cancel it
        // when the lifecycle shutdown fires — otherwise a long-lived SSE body
        // would block axum's graceful shutdown from ever completing.
        let sse_shutdown = CancellationToken::new();

        // Safety enforcement (rp.md § Safety): the gate flag is read by the
        // `/mcp` middleware, the session registry is shared with the
        // enforcer so an unsafe transition can terminate every open MCP
        // session. `None` when no safety monitors are configured — sessions
        // then run ungated and no polling task is spawned.
        let safety_ok = Arc::new(AtomicBool::new(true));
        let mcp_sessions = Arc::new(LocalSessionManager::default());
        let safety = SafetyEnforcer::from_registry(
            equipment.clone(),
            event_bus.clone(),
            session.clone(),
            mcp_sessions.clone(),
            safety_ok.clone(),
            config.safety.poll_interval,
        );

        let state = AppState {
            equipment,
            mcp,
            session: session.clone(),
            image_cache,
            sse_shutdown: sse_shutdown.clone(),
            safety_ok,
            mcp_sessions,
        };

        let router = build_router(state);

        // Layer authentication if configured
        let router = match &config.server.auth {
            Some(auth) => {
                if config.server.tls.is_none() {
                    tracing::warn!(
                        "Authentication is enabled but TLS is not. \
                         Credentials will be transmitted in cleartext. \
                         Consider enabling TLS (see `rp init-tls`)."
                    );
                }
                rp_auth::layer(router, auth)
            }
            None => router,
        };

        let tls = config.server.tls.clone();

        let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
        let local_addr = listener.local_addr()?;

        // Set the MCP base URL on the session manager
        let scheme = if tls.is_some() { "https" } else { "http" };
        let base_url = format!("{scheme}://{local_addr}");
        session.set_mcp_base_url(base_url).await;

        // This println is parsed by BDD tests to discover the bound port.
        // It must go to stdout (not tracing/stderr) so the subprocess output can be read.
        println!("Bound rp server bound_addr={}", local_addr);
        info!("rp service bound on {}", local_addr);

        Ok(BoundServer {
            listener,
            router,
            local_addr,
            tls,
            sse_shutdown,
            safety,
        })
    }
}

impl Default for ServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// A fully bound rp server ready to accept connections.
pub struct BoundServer {
    listener: tokio::net::TcpListener,
    router: axum::Router,
    local_addr: SocketAddr,
    tls: Option<TlsConfig>,
    /// Cancelled by `start()` when the lifecycle shutdown future resolves, so
    /// in-flight `/api/events/subscribe` SSE streams end and axum's graceful
    /// shutdown can drain. A clone lives in `AppState` for the handler.
    sse_shutdown: CancellationToken,
    /// Safety polling loop, spawned by `start()` and cancelled on shutdown.
    /// `None` when no safety monitors are configured.
    safety: Option<SafetyEnforcer<AlpacaSafetyProbe>>,
}

impl BoundServer {
    pub fn listen_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn start(self, shutdown: impl Future<Output = ()> + Send + 'static) -> Result<()> {
        // Chain the lifecycle shutdown to the SSE cancellation token: when the
        // signal fires, cancel in-flight `/api/events/subscribe` streams first
        // so their long-lived response bodies end, then let axum's graceful
        // shutdown drain the remaining connections. Without this, a connected
        // SSE consumer would keep the server from ever shutting down. The
        // safety polling loop rides the same signal.
        let safety_cancel = CancellationToken::new();
        let safety_task = self.safety.map(|enforcer| {
            let cancel = safety_cancel.clone();
            tokio::spawn(enforcer.run(cancel))
        });
        let sse_shutdown = self.sse_shutdown;
        let graceful = async move {
            shutdown.await;
            sse_shutdown.cancel();
            safety_cancel.cancel();
        };

        match self.tls {
            Some(ref tls_config) => {
                info!("rp service started on {} (TLS)", self.local_addr);
                rp_tls::server::serve_tls(self.listener, self.router, tls_config, graceful)
                    .await
                    .map_err(|e| crate::error::RpError::Server(e.to_string()))?;
            }
            None => {
                info!("rp service started on {}", self.local_addr);
                axum::serve(self.listener, self.router)
                    .with_graceful_shutdown(graceful)
                    .await?;
            }
        }

        // The safety loop was cancelled by `graceful`; join it so the
        // process doesn't exit mid-transition.
        if let Some(task) = safety_task {
            let _ = task.await;
        }

        debug!("rp service shut down");
        Ok(())
    }
}
