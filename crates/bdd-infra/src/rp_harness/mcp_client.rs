//! Persistent MCP test client using rmcp.
//!
//! [`McpTestClient`] wraps a single rmcp session. It is created once per
//! scenario (typically in an "MCP client connected to rp" Given step) and
//! reused for all tool calls within that scenario — mirroring how a real
//! MCP client (e.g. calibrator-flats) works.

use std::time::Duration;

use rmcp::model::CallToolRequestParams;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::ServiceExt;
use serde_json::Value;

/// Upper bound on a single MCP request (`call_tool` / `list_tools`).
///
/// rmcp's `Peer::call_tool` has no built-in client timeout: if an rp tool
/// handler never returns, the `await` here hangs *forever*. Two centering-BDD
/// hangs traced to that: `do_capture` looping on a failed `sky-survey-camera`
/// exposure (fixed in `services/rp/src/mcp/internals.rs::do_capture`), and —
/// issue #319 — a per-iteration mount read stalling because rp's Alpaca client
/// had no per-request timeout (fixed in rp's `equipment::alpaca`). This timeout
/// is a defense-in-depth backstop: any *future* handler hang fails the scenario
/// fast and legibly here, rather than hanging the BDD binary until the CI
/// job's `timeout-minutes` kills it.
///
/// 360 s sits comfortably above rp's worst-case single blocking operation
/// (the 300 s slew/park deadlines), so a genuinely slow-but-progressing call
/// still completes and a *server-produced* error still propagates with its
/// real message; only a true hang trips this bound.
const MCP_CALL_TIMEOUT: Duration = Duration::from_secs(360);

/// A persistent MCP client backed by a single rmcp session.
pub struct McpTestClient {
    // Keep the running service alive so the session isn't dropped.
    _service: rmcp::service::RunningService<rmcp::RoleClient, ()>,
    peer: rmcp::Peer<rmcp::RoleClient>,
}

impl McpTestClient {
    /// Connect to an MCP server and perform the initialize handshake.
    pub async fn connect(mcp_url: &str) -> Result<Self, String> {
        let transport = StreamableHttpClientTransport::from_uri(mcp_url);
        let service = ().serve(transport).await.map_err(|e| format!("MCP connect: {}", e))?;
        let peer = service.peer().clone();
        Ok(Self {
            _service: service,
            peer,
        })
    }

    /// Call an MCP tool and return the parsed JSON result or error message.
    pub async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<Value, String> {
        let mut params = CallToolRequestParams::new(tool_name.to_string());
        if let Some(obj) = arguments.as_object() {
            params.arguments = Some(obj.clone());
        }

        let result = tokio::time::timeout(MCP_CALL_TIMEOUT, self.peer.call_tool(params))
            .await
            .map_err(|_| {
                format!(
                    "{}: MCP call timed out after {}s with no response — the rp MCP \
                     transport was almost certainly torn down mid-request (see MCP_CALL_TIMEOUT)",
                    tool_name,
                    MCP_CALL_TIMEOUT.as_secs()
                )
            })?
            .map_err(|e| format!("{}: {}", tool_name, e))?;

        if result.is_error.unwrap_or(false) {
            let msg = result
                .content
                .first()
                .and_then(|c| c.as_text())
                .map(|tc| tc.text.clone())
                .unwrap_or_else(|| "unknown error".to_string());
            return Err(msg);
        }

        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|tc| tc.text.clone())
            .unwrap_or_else(|| "{}".to_string());

        serde_json::from_str(&text).map_err(|e| format!("failed to parse tool result: {}", e))
    }

    /// List all available MCP tools and return their names.
    pub async fn list_tools(&self) -> Result<Vec<String>, String> {
        let tools = tokio::time::timeout(MCP_CALL_TIMEOUT, self.peer.list_all_tools())
            .await
            .map_err(|_| {
                format!(
                    "list_tools: MCP call timed out after {}s with no response (see \
                     MCP_CALL_TIMEOUT)",
                    MCP_CALL_TIMEOUT.as_secs()
                )
            })?
            .map_err(|e| format!("list_tools: {}", e))?;

        Ok(tools.into_iter().map(|t| t.name.to_string()).collect())
    }
}
