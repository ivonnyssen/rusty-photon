//! MCP server for `rp`.
//!
//! `rp` exposes its action surface as MCP tools over rmcp's
//! streamable-HTTP transport. The handler [`McpHandler`] owns shared
//! state (equipment registry, event bus, session config, image cache,
//! observer site, planner targets, plate-solver client) and exposes 41
//! tools across 9 categories: camera, imaging, filter wheel,
//! cover/calibrator, focuser, mount, auto_focus, plate_solve, planner.
//!
//! ## Layout
//!
//! Each tool category lives in its own file under [`built_in`], holding
//! its parameter structs, its tool method bodies, and any
//! category-specific helpers. The category file declares its own
//! `#[tool_router(router = tool_router_<name>, vis = "pub")]` impl block
//! on `McpHandler`. [`McpHandler::new`] merges the per-category routers
//! via `+` (see [`handler::McpHandler::new`]). A single explicit
//! `#[tool_handler(router = self.tool_router)] impl ServerHandler` block
//! at the bottom of this file glues the merged router into rmcp's
//! transport.
//!
//! Cross-category helper methods on `McpHandler`
//! (`do_capture`, `do_move_focuser_blocking`, `*_via_document` /
//! `*_via_path` dispatch helpers, `persist_capture_artifact`,
//! `resolve_mount`, `read_mount_hints_for_plate_solve`,
//! `do_slew_blocking`, `do_park_blocking`) live in [`internals`]
//! together with their supporting private types (`ResolvedParams`,
//! `BackgroundOutcome`, `DetectStarsOutcome`,
//! `ResolvedMeasureStarsParams`, `PollIdleError`) and free helper
//! functions (`clip_outcome`, `detect_outcome`, `star_to_json`,
//! `poll_slewing_until_idle`).
//!
//! ## Adding a tool category
//!
//! 1. Add `<name>.rs` under [`built_in`] with a
//!    `#[tool_router(router = tool_router_<name>, vis = "pub")] impl
//!    McpHandler { ... #[tool] async fn ... }` block. Param structs and
//!    private helpers go in the same file.
//! 2. Add `pub mod <name>;` to `built_in/mod.rs` and a re-export of the
//!    category's param structs.
//! 3. Add `+ Self::tool_router_<name>()` to the merge chain in
//!    [`handler::McpHandler::new`]. No edits needed in any existing
//!    category file.
//!
//! ## Adding a tool to an existing category
//!
//! Edit only `built_in/<category>.rs`. Add the param struct(s), then
//! add a new `#[tool(description = "...")] async fn ...` inside the
//! existing `#[tool_router]` impl block.
//!
//! ## Macros
//!
//! Three private declarative macros simplify tool bodies:
//!
//! - `tool_success!({...})` — wraps a `serde_json::json!()` payload
//!   into a `CallToolResult::success` text content.
//! - `tool_error!("...", arg)` — returns a `CallToolResult::error`
//!   carrying the formatted message.
//! - `resolve_device!(self, find_X, &id, "kind")` — looks up a device
//!   by id in the equipment registry, early-returning the standard
//!   "kind not found" / "kind not connected" error when missing or
//!   disconnected.
//!
//! They live in this file and are re-exported via `pub(crate) use` so
//! sibling submodules can `use super::{tool_success, tool_error,
//! resolve_device};`.

pub mod built_in;
pub mod handler;
pub mod internals;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests;

pub use handler::McpHandler;

// ---------------------------------------------------------------------------
// Shared private macros, exposed to sibling submodules via `pub(crate) use`.
// ---------------------------------------------------------------------------

/// Build a successful `CallToolResult` from a `serde_json::json!(...)` value.
macro_rules! tool_success {
    ($($json:tt)+) => {
        ::rmcp::model::CallToolResult::success(vec![::rmcp::model::Content::text(
            ::serde_json::json!($($json)+).to_string(),
        )])
    };
}

/// Build an error `CallToolResult` from a format string or literal.
macro_rules! tool_error {
    ($lit:literal) => {
        ::rmcp::model::CallToolResult::error(vec![::rmcp::model::Content::text($lit)])
    };
    ($($arg:tt)+) => {
        ::rmcp::model::CallToolResult::error(vec![::rmcp::model::Content::text(format!($($arg)+))])
    };
}

/// Look up a device by ID and return the entry + connected device, or
/// early-return a `tool_error` `CallToolResult` from the enclosing function.
///
/// Usage: `let (entry, device) = resolve_device!(self, find_camera, &id, "camera");`
/// (the `id` argument is forwarded into `EquipmentRegistry::find_*`,
/// which take `&str` — every real call site passes `&params.camera_id`,
/// `&camera_id`, etc.)
macro_rules! resolve_device {
    ($self:expr, $finder:ident, $id:expr, $kind:literal) => {{
        let entry = match $self.equipment.$finder($id) {
            Some(e) => e,
            None => return Ok(tool_error!(concat!($kind, " not found: {}"), $id)),
        };
        let device = match &entry.device {
            Some(d) => d.clone(),
            None => return Ok(tool_error!(concat!($kind, " not connected: {}"), $id)),
        };
        (entry, device)
    }};
}

pub(crate) use resolve_device;
pub(crate) use tool_error;
pub(crate) use tool_success;

// ---------------------------------------------------------------------------
// ServerHandler glue.
//
// The standalone `#[tool_handler(router = self.tool_router)]` reads the
// merged router off `McpHandler::tool_router` (the field populated in
// `McpHandler::new` by summing per-category routers via `+`). We use
// the standalone form rather than the `#[tool_router(server_handler)]`
// shortcut because pattern (c) — multiple per-category `#[tool_router]`
// blocks merged manually — would otherwise emit conflicting
// `ServerHandler` impls.
// ---------------------------------------------------------------------------

#[rmcp::tool_handler(router = self.tool_router)]
impl rmcp::handler::server::ServerHandler for McpHandler {}
