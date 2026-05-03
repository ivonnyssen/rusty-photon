//! rp-plate-solver — rp-managed service wrapping the ASTAP CLI.
//!
//! See `docs/services/rp-plate-solver.md` for the design contract and
//! `docs/plans/rp-plate-solver.md` for sequencing.

pub mod api;
pub mod config;
pub mod error;
pub mod runner;
pub mod supervision;

pub use api::AppState;
pub use config::{load_config, Config, ConfigError};
pub use error::{AppError, ErrorCode, ErrorResponse};
pub use runner::astap::AstapCliRunner;
pub use runner::{AstapRunner, RunnerError, SolveOutcome, SolveRequest};

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

/// Two-phase server builder. Use `build()` to bind the TCP listener
/// (so the bound port is known up-front), then `start()` to serve.
/// Mirrors `services/rp::ServerBuilder` and avoids the port-TOCTOU
/// race noted in `docs/skills/development-workflow.md` §"Phase 4
/// Stabilization".
pub struct ServerBuilder {
    config: Option<Config>,
    runner: Option<Arc<dyn AstapRunner>>,
}

impl ServerBuilder {
    pub fn new() -> Self {
        Self {
            config: None,
            runner: None,
        }
    }

    pub fn with_config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    /// Override the runner (tests inject mocks; production uses
    /// `AstapCliRunner` constructed from config).
    pub fn with_runner(mut self, runner: Arc<dyn AstapRunner>) -> Self {
        self.runner = Some(runner);
        self
    }

    pub async fn build(self) -> Result<BoundServer, std::io::Error> {
        let config = self.config.expect("ServerBuilder: config is required");
        config
            .validate()
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        let runner = self.runner.unwrap_or_else(|| {
            let mut runner = AstapCliRunner::new(
                config.astap_binary_path.clone(),
                config.astap_db_directory.clone(),
            );
            for (k, v) in &config.astap_extra_env {
                runner = runner.with_env(k, v);
            }
            Arc::new(runner)
        });

        let semaphore = Arc::new(Semaphore::new(config.max_concurrency));

        let state = AppState {
            runner,
            semaphore,
            default_solve_timeout: config.default_solve_timeout,
            max_solve_timeout: config.max_solve_timeout,
            astap_binary_path: config.astap_binary_path.clone(),
            astap_db_directory: config.astap_db_directory.clone(),
        };

        let router = api::build_router(state);

        let bind_addr = SocketAddr::new(config.bind_address, config.port);
        let listener = TcpListener::bind(bind_addr).await?;
        let local_addr = listener.local_addr()?;

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

/// A fully bound server ready to accept connections.
pub struct BoundServer {
    listener: TcpListener,
    router: axum::Router,
    local_addr: SocketAddr,
}

impl BoundServer {
    pub fn listen_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Run the server until shutdown. Listens for SIGTERM (Unix) or
    /// CTRL_BREAK_EVENT (Windows) and shuts down gracefully.
    pub async fn start(self) -> Result<(), std::io::Error> {
        axum::serve(self.listener, self.router)
            .with_graceful_shutdown(shutdown_signal())
            .await
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        let mut sigint = signal(SignalKind::interrupt()).expect("install SIGINT handler");
        tokio::select! {
            _ = sigterm.recv() => tracing::debug!("SIGTERM received, shutting down"),
            _ = sigint.recv() => tracing::debug!("SIGINT received, shutting down"),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        tracing::debug!("CTRL_C received, shutting down");
    }
}
