//! MCP client for calling rp's built-in tools via rmcp.

use std::time::Duration;

use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::ServiceExt;
use serde::Deserialize;
use serde_json::Value;
use tracing::debug;

use crate::error::{CalibratorFlatsError, Result};

/// MCP client backed by rmcp's Streamable HTTP transport.
pub struct McpClient {
    peer: rmcp::Peer<rmcp::RoleClient>,
    // Keep the running service alive so the connection isn't dropped.
    _service: RunningService<rmcp::RoleClient, ()>,
}

/// Result from the `capture` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct CaptureResult {
    pub image_path: String,
    pub document_id: String,
}

/// Result from the `get_camera_info` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct CameraInfo {
    pub max_adu: u32,
    #[serde(with = "humantime_serde")]
    pub exposure_min: Duration,
    #[serde(with = "humantime_serde")]
    pub exposure_max: Duration,
}

/// Result from the `compute_image_stats` tool.
#[derive(Debug, Clone, Deserialize)]
pub struct ImageStats {
    pub median_adu: u32,
    pub mean_adu: f64,
}

impl McpClient {
    /// Connect to an MCP server at the given URL.
    pub async fn new(mcp_url: &str) -> Result<Self> {
        debug!(url = %mcp_url, "connecting MCP client");
        let transport = StreamableHttpClientTransport::from_uri(mcp_url);
        let service = ()
            .serve(transport)
            .await
            .map_err(|e| CalibratorFlatsError::ToolCall(format!("MCP connect: {}", e)))?;
        let peer = service.peer().clone();
        Ok(Self {
            peer,
            _service: service,
        })
    }

    pub async fn capture(&self, camera_id: &str, duration: Duration) -> Result<CaptureResult> {
        self.call_tool(
            "capture",
            serde_json::json!({
                "camera_id": camera_id,
                "duration": humantime::format_duration(duration).to_string(),
            }),
        )
        .await
    }

    pub async fn get_camera_info(&self, camera_id: &str) -> Result<CameraInfo> {
        self.call_tool(
            "get_camera_info",
            serde_json::json!({"camera_id": camera_id}),
        )
        .await
    }

    pub async fn compute_image_stats(
        &self,
        image_path: &str,
        document_id: Option<&str>,
    ) -> Result<ImageStats> {
        let mut args = serde_json::json!({"image_path": image_path});
        if let Some(doc_id) = document_id {
            args["document_id"] = serde_json::json!(doc_id);
        }
        self.call_tool("compute_image_stats", args).await
    }

    pub async fn set_filter(&self, filter_wheel_id: &str, filter_name: &str) -> Result<()> {
        let _: Value = self
            .call_tool(
                "set_filter",
                serde_json::json!({"filter_wheel_id": filter_wheel_id, "filter_name": filter_name}),
            )
            .await?;
        Ok(())
    }

    pub async fn close_cover(&self, calibrator_id: &str) -> Result<()> {
        let _: Value = self
            .call_tool(
                "close_cover",
                serde_json::json!({"calibrator_id": calibrator_id}),
            )
            .await?;
        Ok(())
    }

    pub async fn open_cover(&self, calibrator_id: &str) -> Result<()> {
        let _: Value = self
            .call_tool(
                "open_cover",
                serde_json::json!({"calibrator_id": calibrator_id}),
            )
            .await?;
        Ok(())
    }

    pub async fn calibrator_on(&self, calibrator_id: &str, brightness: Option<u32>) -> Result<()> {
        let mut args = serde_json::json!({"calibrator_id": calibrator_id});
        if let Some(b) = brightness {
            args["brightness"] = serde_json::json!(b);
        }
        let _: Value = self.call_tool("calibrator_on", args).await?;
        Ok(())
    }

    pub async fn calibrator_off(&self, calibrator_id: &str) -> Result<()> {
        let _: Value = self
            .call_tool(
                "calibrator_off",
                serde_json::json!({"calibrator_id": calibrator_id}),
            )
            .await?;
        Ok(())
    }

    /// Generic helper: call tool, check for errors, deserialize result.
    async fn call_tool<T: serde::de::DeserializeOwned>(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<T> {
        debug!(tool = %tool_name, "calling MCP tool");

        let mut params = CallToolRequestParams::new(tool_name.to_string());
        if let Some(obj) = arguments.as_object() {
            params.arguments = Some(obj.clone());
        }

        let result = self
            .peer
            .call_tool(params)
            .await
            .map_err(|e| CalibratorFlatsError::ToolCall(format!("{}: {}", tool_name, e)))?;

        if result.is_error.unwrap_or(false) {
            let msg = result
                .content
                .first()
                .and_then(|c| c.as_text())
                .map(|tc| tc.text.clone())
                .unwrap_or_else(|| "unknown error".to_string());
            return Err(CalibratorFlatsError::ToolCall(format!(
                "{}: {}",
                tool_name, msg
            )));
        }

        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|tc| tc.text.clone())
            .ok_or_else(|| {
                CalibratorFlatsError::ToolCall(format!("{}: no content in response", tool_name))
            })?;

        serde_json::from_str(&text).map_err(|e| {
            CalibratorFlatsError::ToolCall(format!("{}: failed to parse result: {}", tool_name, e))
        })
    }
}
