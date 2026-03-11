pub mod config;
pub mod equipment;
pub mod error;
pub mod events;
pub mod mcp;
pub mod routes;
pub mod session;

use std::sync::Arc;

use tokio::signal;
use tracing::{debug, info};

use crate::config::Config;
use crate::equipment::EquipmentRegistry;
use crate::error::Result;
use crate::events::EventBus;
use crate::mcp::McpHandler;
use crate::routes::{build_router, AppState};
use crate::session::{SessionConfig, SessionManager};

pub async fn start(config: Config) -> Result<()> {
    let bind_addr = format!("{}:{}", config.server.bind_address, config.server.port);

    debug!("initializing equipment registry");
    let equipment = Arc::new(EquipmentRegistry::new(&config.equipment).await);

    debug!("initializing event bus");
    let event_bus = Arc::new(EventBus::from_config(&config.plugins));

    debug!("initializing session manager");
    let session = Arc::new(SessionManager::new(event_bus.clone(), &config.plugins));

    let session_config = SessionConfig {
        data_directory: config.session.data_directory.clone(),
    };

    let mcp = Arc::new(McpHandler {
        equipment: equipment.clone(),
        event_bus: event_bus.clone(),
        session_config,
    });

    let state = AppState {
        equipment,
        mcp,
        session: session.clone(),
    };

    let router = build_router(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    let local_addr = listener.local_addr()?;

    // Set the MCP base URL on the session manager
    let base_url = format!("http://{}", local_addr);
    session.set_mcp_base_url(base_url).await;

    info!("rp service started on {}", local_addr);

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    debug!("rp service shut down");
    Ok(())
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
