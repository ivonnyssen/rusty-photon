//! PHD2 Guider Client Library
//!
//! This crate provides a Rust client for interacting with Open PHD Guiding 2 (PHD2)
//! via its JSON RPC interface on port 4400.
//!
//! ## Module Structure
//!
//! - [`client`] - PHD2 client for RPC communication
//! - [`config`] - Configuration types and loading
//! - [`error`] - Error types and Result alias
//! - [`events`] - PHD2 event types and application state
//! - [`process`] - PHD2 process management
//! - [`rpc`] - JSON RPC 2.0 types
//! - [`types`] - Common types (Rect, Profile, Equipment)
//!
//! ## Example
//!
//! ```ignore
//! use phd2_guider::{Phd2Client, Phd2Config, SettleParams};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = Phd2Config::default();
//!     let client = Phd2Client::new(config);
//!
//!     client.connect().await?;
//!
//!     let state = client.get_app_state().await?;
//!     println!("PHD2 state: {}", state);
//!
//!     client.disconnect().await?;
//!     Ok(())
//! }
//! ```

pub mod client;
pub mod config;
pub mod error;
pub mod events;
pub mod process;
pub mod rpc;
pub mod types;

// Re-export commonly used types at the crate root for convenience
pub use client::Phd2Client;
pub use config::{load_config, Config, Phd2Config, ReconnectConfig, SettleParams};
pub use error::{Phd2Error, Result};
pub use events::{AppState, GuideStepStats, Phd2Event};
pub use process::{get_default_phd2_path, Phd2ProcessManager};
pub use rpc::{RpcErrorObject, RpcRequest, RpcResponse};
pub use types::{Equipment, EquipmentDevice, Profile, Rect};
