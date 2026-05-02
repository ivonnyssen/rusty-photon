//! rp-plate-solver — rp-managed service wrapping the ASTAP CLI.
//!
//! Phase 2 scaffold: library surface only. The HTTP server (`api`) and
//! `main.rs` arrive in Phase 4 per `docs/plans/rp-plate-solver.md`.

pub mod config;
pub mod error;
pub mod runner;
pub mod supervision;

pub use config::{load_config, Config, ConfigError};
pub use error::{AppError, ErrorCode, ErrorResponse};
pub use runner::astap::AstapCliRunner;
pub use runner::{AstapRunner, RunnerError, SolveOutcome, SolveRequest};
