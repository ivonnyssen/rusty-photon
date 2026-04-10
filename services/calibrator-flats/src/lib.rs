#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
pub mod config;
pub mod error;
pub mod mcp_client;
pub mod routes;
pub mod workflow;

use std::net::SocketAddr;

use tokio::signal;
use tracing::{debug, info};

use crate::config::FlatPlan;
use crate::error::Result;

/// Builder for the calibrator-flats server.
pub struct ServerBuilder {
    plan: Option<FlatPlan>,
    port: u16,
    bind_address: String,
}

impl ServerBuilder {
    pub fn new() -> Self {
        Self {
            plan: None,
            port: 11170,
            bind_address: "127.0.0.1".to_string(),
        }
    }

    pub fn with_plan(mut self, plan: FlatPlan) -> Self {
        self.plan = Some(plan);
        self
    }

    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    pub fn with_bind_address(mut self, addr: String) -> Self {
        self.bind_address = addr;
        self
    }

    pub async fn build(self) -> Result<BoundServer> {
        let plan = self.plan.expect("flat plan is required");
        let bind_addr = format!("{}:{}", self.bind_address, self.port);

        let router = routes::build_router(plan);

        let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
        let local_addr = listener.local_addr()?;

        // This println is parsed by BDD tests to discover the bound port.
        println!("Bound calibrator-flats server bound_addr={}", local_addr);
        info!("calibrator-flats service bound on {}", local_addr);

        Ok(BoundServer {
            listener,
            router,
            local_addr,
        })
    }
}

impl Default for ServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// A fully bound calibrator-flats server ready to accept connections.
pub struct BoundServer {
    listener: tokio::net::TcpListener,
    router: axum::Router,
    local_addr: SocketAddr,
}

impl BoundServer {
    pub fn listen_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn start(self) -> Result<()> {
        info!("calibrator-flats service started on {}", self.local_addr);

        axum::serve(self.listener, self.router)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(|e| crate::error::CalibratorFlatsError::Server(e.to_string()))?;

        debug!("calibrator-flats service shut down");
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
