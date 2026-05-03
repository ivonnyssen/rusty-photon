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
pub mod session;
pub mod tls_cmd;

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::signal;
use tracing::{debug, info};

use rp_tls::config::TlsConfig;

use crate::config::Config;
use crate::equipment::EquipmentRegistry;
use crate::error::Result;
use crate::events::EventBus;
use crate::mcp::McpHandler;
use crate::persistence::ImageCache;
use crate::routes::{build_router, AppState};
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
        let config = self.config.expect("config is required");
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
        let session = Arc::new(SessionManager::new(event_bus.clone(), &config.plugins));

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

        let mcp = McpHandler::new(
            equipment.clone(),
            event_bus.clone(),
            session_config,
            image_cache.clone(),
            site,
        )
        .with_planner_config(targets, default_min_alt);

        let state = AppState {
            equipment,
            mcp,
            session: session.clone(),
            image_cache,
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
}

impl BoundServer {
    pub fn listen_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn start(self) -> Result<()> {
        match self.tls {
            Some(ref tls_config) => {
                info!("rp service started on {} (TLS)", self.local_addr);
                rp_tls::server::serve_tls(
                    self.listener,
                    self.router,
                    tls_config,
                    shutdown_signal(),
                )
                .await
                .map_err(|e| crate::error::RpError::Server(e.to_string()))?;
            }
            None => {
                info!("rp service started on {}", self.local_addr);
                axum::serve(self.listener, self.router)
                    .with_graceful_shutdown(shutdown_signal())
                    .await?;
            }
        }

        debug!("rp service shut down");
        Ok(())
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => debug!("received Ctrl+C"),
        () = terminate => debug!("received SIGTERM"),
    }
}
