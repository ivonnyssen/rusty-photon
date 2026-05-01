//! Test harness for services that interact with the rp service.
//!
//! This module is gated behind the `rp-harness` cargo feature. It provides
//! everything a plugin or workflow service needs to run BDD tests against a
//! real rp process:
//!
//! - [`OmniSimHandle`] — singleton ASCOM simulator process (camera, filter
//!   wheel, cover calibrator).
//! - [`RpConfigBuilder`] + [`CameraConfig`], [`FilterWheelConfig`],
//!   [`CoverCalibratorConfig`] — accumulate equipment/plugin entries and emit
//!   a JSON config for rp.
//! - [`start_rp`] / [`wait_for_rp_healthy`] — spawn rp with a config and
//!   wait for its `/health` endpoint.
//! - [`WebhookReceiver`] + [`ReceivedEvent`] — in-process HTTP server that
//!   acts as an event plugin so tests can assert on emitted events.
//! - [`TestOrchestrator`] + [`OrchestratorBehavior`],
//!   [`OrchestratorInvocation`] — in-process orchestrator plugin with
//!   configurable behavior.
//! - [`McpTestClient`] — persistent rmcp session for calling rp's MCP tools.
//!
//! All types emit and consume `serde_json::Value`. Nothing here depends on
//! rp's own types, which keeps the dependency direction one-way (rp's tests
//! and plugin tests depend on bdd-infra; bdd-infra does not depend on rp).

mod config;
mod launcher;
mod mcp_client;
mod omnisim;
mod orchestrator;
mod webhook;

pub use config::{
    build_calibrator_flats_config, CameraConfig, CoverCalibratorConfig, FilterWheelConfig,
    FocuserConfig, RpConfigBuilder,
};
pub use launcher::{start_rp, wait_for_rp_healthy, write_temp_config_file};
pub use mcp_client::McpTestClient;
pub use omnisim::OmniSimHandle;
pub use orchestrator::{OrchestratorBehavior, OrchestratorInvocation, TestOrchestrator};
pub use webhook::{ReceivedEvent, WebhookReceiver};
