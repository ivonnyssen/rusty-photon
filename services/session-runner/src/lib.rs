#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! `session-runner` — generic imaging-workflow orchestrator (an `rp`
//! orchestrator plugin).
//!
//! Design: `docs/services/session-runner.md`; delivery plan:
//! `docs/plans/workflow-dsl.md`. This crate ships the expression layer
//! ([`expr`]), the document layer ([`document`]: model, validation layers
//! 1–2, parameter binding), the engine ([`engine`] + [`blackboard`],
//! triggers included), the SSE event client ([`events`]), and the service
//! wiring ([`mcp_client`], [`routes`], [`config`]) behind the two-phase
//! [`ServerBuilder`]. Still ahead in the plan: the resume BDD proof
//! (Phase D) and the deep-sky document (Phase E).

pub mod blackboard;
pub mod config;
pub mod document;
pub mod engine;
pub mod error;
pub mod events;
pub mod expr;
pub mod mcp_client;
pub mod routes;

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use tracing::{debug, info};

use crate::config::Config;
use crate::error::{Result, SessionRunnerError};

/// Builder for the session-runner server (two-phase: build → start).
pub struct ServerBuilder {
    config: Option<Config>,
    bind_address: String,
}

impl ServerBuilder {
    pub fn new() -> Self {
        Self {
            config: None,
            bind_address: "127.0.0.1".to_owned(),
        }
    }

    pub fn with_config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    pub fn with_bind_address(mut self, addr: String) -> Self {
        self.bind_address = addr;
        self
    }

    pub async fn build(self) -> Result<BoundServer> {
        let config = self.config.ok_or_else(|| {
            SessionRunnerError::Config(
                "ServerBuilder::build: configuration is required — call .with_config(...) first"
                    .to_owned(),
            )
        })?;
        let bind_addr = format!("{}:{}", self.bind_address, config.port);
        let router = routes::build_router(Arc::new(config));

        let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
        let local_addr = listener.local_addr()?;

        // This println is parsed by BDD tests to discover the bound port.
        println!("Bound session-runner server bound_addr={local_addr}");
        info!("session-runner service bound on {local_addr}");

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

/// A fully bound session-runner server ready to accept connections.
pub struct BoundServer {
    listener: tokio::net::TcpListener,
    router: axum::Router,
    local_addr: SocketAddr,
}

impl BoundServer {
    pub fn listen_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn start(self, shutdown: impl Future<Output = ()> + Send + 'static) -> Result<()> {
        info!("session-runner service started on {}", self.local_addr);

        axum::serve(self.listener, self.router)
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(|e| SessionRunnerError::Server(e.to_string()))?;

        debug!("session-runner service shut down");
        Ok(())
    }
}
