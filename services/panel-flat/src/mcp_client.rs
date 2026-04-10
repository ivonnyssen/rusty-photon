//! MCP client for calling rp's built-in tools via JSON-RPC over HTTP.

use serde_json::Value;
use tracing::debug;

use crate::error::{PanelFlatError, Result};

/// Thin JSON-RPC client that POSTs to rp's `/mcp` endpoint.
pub struct McpClient {
    http: reqwest::Client,
    mcp_url: String,
}

/// Result from the `capture` tool.
#[derive(Debug, Clone)]
pub struct CaptureResult {
    pub image_path: String,
    pub document_id: String,
}

/// Result from the `get_camera_info` tool.
#[derive(Debug, Clone)]
pub struct CameraInfo {
    pub max_adu: u32,
    pub exposure_min_ms: u64,
    pub exposure_max_ms: u64,
}

/// Result from the `compute_image_stats` tool.
#[derive(Debug, Clone)]
pub struct ImageStats {
    pub median_adu: u32,
    pub mean_adu: f64,
}

impl McpClient {
    pub fn new(mcp_url: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            mcp_url: mcp_url.to_string(),
        }
    }

    pub async fn capture(&self, camera_id: &str, duration_ms: u32) -> Result<CaptureResult> {
        let result = self
            .call_tool(
                "capture",
                serde_json::json!({
                    "camera_id": camera_id,
                    "duration_ms": duration_ms,
                }),
            )
            .await?;

        Ok(CaptureResult {
            image_path: result["image_path"]
                .as_str()
                .ok_or_else(|| {
                    PanelFlatError::ToolCall("missing image_path in capture result".into())
                })?
                .to_string(),
            document_id: result["document_id"]
                .as_str()
                .ok_or_else(|| {
                    PanelFlatError::ToolCall("missing document_id in capture result".into())
                })?
                .to_string(),
        })
    }

    pub async fn get_camera_info(&self, camera_id: &str) -> Result<CameraInfo> {
        let result = self
            .call_tool(
                "get_camera_info",
                serde_json::json!({ "camera_id": camera_id }),
            )
            .await?;

        Ok(CameraInfo {
            max_adu: result["max_adu"]
                .as_u64()
                .ok_or_else(|| PanelFlatError::ToolCall("missing max_adu".into()))?
                as u32,
            exposure_min_ms: result["exposure_min_ms"]
                .as_u64()
                .ok_or_else(|| PanelFlatError::ToolCall("missing exposure_min_ms".into()))?,
            exposure_max_ms: result["exposure_max_ms"]
                .as_u64()
                .ok_or_else(|| PanelFlatError::ToolCall("missing exposure_max_ms".into()))?,
        })
    }

    pub async fn compute_image_stats(
        &self,
        image_path: &str,
        document_id: Option<&str>,
    ) -> Result<ImageStats> {
        let mut args = serde_json::json!({ "image_path": image_path });
        if let Some(doc_id) = document_id {
            args["document_id"] = serde_json::json!(doc_id);
        }

        let result = self.call_tool("compute_image_stats", args).await?;

        Ok(ImageStats {
            median_adu: result["median_adu"]
                .as_u64()
                .ok_or_else(|| PanelFlatError::ToolCall("missing median_adu".into()))?
                as u32,
            mean_adu: result["mean_adu"]
                .as_f64()
                .ok_or_else(|| PanelFlatError::ToolCall("missing mean_adu".into()))?,
        })
    }

    pub async fn set_filter(&self, filter_wheel_id: &str, filter_name: &str) -> Result<()> {
        self.call_tool(
            "set_filter",
            serde_json::json!({
                "filter_wheel_id": filter_wheel_id,
                "filter_name": filter_name,
            }),
        )
        .await?;
        Ok(())
    }

    pub async fn close_cover(&self, calibrator_id: &str) -> Result<()> {
        self.call_tool(
            "close_cover",
            serde_json::json!({ "calibrator_id": calibrator_id }),
        )
        .await?;
        Ok(())
    }

    pub async fn open_cover(&self, calibrator_id: &str) -> Result<()> {
        self.call_tool(
            "open_cover",
            serde_json::json!({ "calibrator_id": calibrator_id }),
        )
        .await?;
        Ok(())
    }

    pub async fn calibrator_on(&self, calibrator_id: &str, brightness: Option<u32>) -> Result<()> {
        let mut args = serde_json::json!({ "calibrator_id": calibrator_id });
        if let Some(b) = brightness {
            args["brightness"] = serde_json::json!(b);
        }
        self.call_tool("calibrator_on", args).await?;
        Ok(())
    }

    pub async fn calibrator_off(&self, calibrator_id: &str) -> Result<()> {
        self.call_tool(
            "calibrator_off",
            serde_json::json!({ "calibrator_id": calibrator_id }),
        )
        .await?;
        Ok(())
    }

    /// Send a JSON-RPC tools/call request and return the result.
    async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<Value> {
        debug!(tool = %tool_name, url = %self.mcp_url, "calling MCP tool");

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": tool_name,
                "arguments": arguments,
            }
        });

        let resp = self
            .http
            .post(&self.mcp_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| PanelFlatError::ToolCall(format!("{}: {}", tool_name, e)))?;

        let json: Value = resp.json().await.map_err(|e| {
            PanelFlatError::ToolCall(format!("{}: failed to parse response: {}", tool_name, e))
        })?;

        if let Some(err) = json.get("error") {
            let msg = err["message"].as_str().unwrap_or("unknown error");
            return Err(PanelFlatError::ToolCall(format!("{}: {}", tool_name, msg)));
        }

        json.get("result").cloned().ok_or_else(|| {
            PanelFlatError::ToolCall(format!("{}: no result in response", tool_name))
        })
    }
}
