//! The rmcp-backed MCP client: `rp`'s tool catalog for layer-2 validation
//! and the engine's [`ToolClient`] seam for execution.
//!
//! Error mapping (pinned in `docs/services/session-runner.md` § Safety
//! Behavior): a call that *returns* with `is_error` is a tool failure —
//! retryable and catchable ([`ToolCallError::Failed`]). Any
//! **request-level failure** — transport loss or a JSON-RPC protocol
//! error — is treated as a terminated MCP session
//! ([`ToolCallError::SessionTerminated`]): `rp` tearing the session down
//! on a safety transition presents exactly this way, `rp` reports tool
//! failures via `is_error` results (so a protocol error means `rp` itself
//! is unhealthy), and the engine's response (best-effort `finally`,
//! persist, exit without completion, await re-invocation) is the safest
//! generic recovery.

use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::ServiceExt;
use serde_json::{Map, Value};
use tracing::debug;

use crate::document::ToolSpec;
use crate::engine::{ToolCallError, ToolClient};
use crate::error::{Result, SessionRunnerError};

/// MCP client backed by rmcp's Streamable HTTP transport.
pub struct McpClient {
    peer: rmcp::Peer<rmcp::RoleClient>,
    // Keep the running service alive so the connection isn't dropped.
    _service: RunningService<rmcp::RoleClient, ()>,
}

impl McpClient {
    /// Connect to `rp`'s MCP server at the given URL.
    pub async fn connect(mcp_url: &str) -> Result<Self> {
        debug!(url = %mcp_url, "connecting MCP client");
        let transport = StreamableHttpClientTransport::from_uri(mcp_url);
        let service = ()
            .serve(transport)
            .await
            .map_err(|e| SessionRunnerError::Mcp(format!("connect to {mcp_url}: {e}")))?;
        let peer = service.peer().clone();
        Ok(Self {
            peer,
            _service: service,
        })
    }

    /// `tools/list` → the catalog for layer-2 validation.
    pub async fn list_tools(&self) -> Result<Vec<ToolSpec>> {
        let tools = self
            .peer
            .list_all_tools()
            .await
            .map_err(|e| SessionRunnerError::Mcp(format!("tools/list: {e}")))?;
        Ok(tools
            .into_iter()
            .map(|tool| ToolSpec {
                name: tool.name.into_owned(),
                input_schema: Value::Object((*tool.input_schema).clone()),
            })
            .collect())
    }
}

impl ToolClient for McpClient {
    async fn call(
        &self,
        tool: &str,
        args: Map<String, Value>,
    ) -> std::result::Result<Value, ToolCallError> {
        let mut params = CallToolRequestParams::new(tool.to_string());
        if !args.is_empty() {
            params.arguments = Some(args);
        }
        let result = match self.peer.call_tool(params).await {
            Ok(result) => result,
            // The request itself failed: the session is unusable.
            Err(e) => return Err(ToolCallError::SessionTerminated(e.to_string())),
        };

        if result.is_error.unwrap_or(false) {
            // Best-effort message extraction on the error path.
            let message = result
                .content
                .first()
                .and_then(|content| content.as_text())
                .map_or_else(|| "unknown error".to_owned(), |text| text.text.clone());
            return Err(ToolCallError::Failed(message));
        }

        // rp returns tool results as one JSON text content block; the
        // parsed value becomes the document's `result` namespace. No
        // content means no result (`null`). Anything else — non-JSON
        // text, a non-text block, multiple blocks — is a loud failure
        // rather than a silently dropped or stringified result.
        match result.content.as_slice() {
            [] => Ok(Value::Null),
            [block] => match block.as_text() {
                Some(text) => serde_json::from_str(&text.text).map_err(|e| {
                    ToolCallError::Failed(format!("tool returned non-JSON content: {e}"))
                }),
                None => Err(ToolCallError::Failed(
                    "tool returned non-text content; expected one JSON text block".to_owned(),
                )),
            },
            blocks => Err(ToolCallError::Failed(format!(
                "tool returned {} content blocks; expected one JSON text block",
                blocks.len()
            ))),
        }
    }
}
