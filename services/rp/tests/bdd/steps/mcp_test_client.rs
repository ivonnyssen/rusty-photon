//! Shared MCP test client helpers using rmcp.
//!
//! Provides `call_tool` and `list_tools` functions that create an rmcp
//! client, execute a single operation, and return the result. Each call
//! establishes a fresh connection (stateless mode, json_response).

use rmcp::model::CallToolRequestParams;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::ServiceExt;
use serde_json::Value;

/// Call an MCP tool and return the parsed JSON result or error message.
pub async fn call_tool(mcp_url: &str, tool_name: &str, arguments: Value) -> Result<Value, String> {
    let transport = StreamableHttpClientTransport::from_uri(mcp_url);
    let client = ().serve(transport).await.map_err(|e| format!("MCP connect: {}", e))?;
    let peer = client.peer();

    let mut params = CallToolRequestParams::new(tool_name.to_string());
    if let Some(obj) = arguments.as_object() {
        params.arguments = Some(obj.clone());
    }

    let result = peer
        .call_tool(params)
        .await
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
pub async fn list_tools(mcp_url: &str) -> Result<Vec<String>, String> {
    let transport = StreamableHttpClientTransport::from_uri(mcp_url);
    let client = ().serve(transport).await.map_err(|e| format!("MCP connect: {}", e))?;
    let peer = client.peer();

    let tools = peer
        .list_all_tools()
        .await
        .map_err(|e| format!("list_tools: {}", e))?;

    Ok(tools.into_iter().map(|t| t.name.to_string()).collect())
}
