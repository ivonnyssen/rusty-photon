use std::time::Duration;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use super::super::handler::McpHandler;
use super::super::{resolve_device, tool_error, tool_success};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetFilterParams {
    pub filter_wheel_id: String,
    pub filter_name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FilterWheelIdParams {
    pub filter_wheel_id: String,
}

#[tool_router(router = tool_router_filter_wheel, vis = "pub")]
impl McpHandler {
    #[tool(description = "Set the active filter on a filter wheel")]
    pub(crate) async fn set_filter(
        &self,
        Parameters(params): Parameters<SetFilterParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (fw_entry, fw) = resolve_device!(
            self,
            find_filter_wheel,
            &params.filter_wheel_id,
            "filter wheel"
        );

        let position = match fw_entry
            .config
            .filters
            .iter()
            .position(|f| f == &params.filter_name)
        {
            Some(p) => p,
            None => return Ok(tool_error!("filter not found: {}", params.filter_name)),
        };

        if let Err(e) = fw.set_position(position).await {
            return Ok(tool_error!("failed to set filter position: {}", e));
        }

        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            match fw.position().await {
                Ok(Some(p)) if p == position => break,
                Ok(Some(_)) | Ok(None) => continue,
                Err(e) => {
                    return Ok(tool_error!("error waiting for filter wheel: {}", e));
                }
            }
        }

        self.event_bus.emit(
            "filter_switch",
            serde_json::json!({
                "filter_wheel_id": params.filter_wheel_id,
                "filter_name": params.filter_name,
            }),
        );

        Ok(tool_success!({
            "filter_wheel_id": params.filter_wheel_id,
            "filter_name": params.filter_name,
            "position": position,
        }))
    }

    #[tool(description = "Get the current filter on a filter wheel")]
    pub(crate) async fn get_filter(
        &self,
        Parameters(params): Parameters<FilterWheelIdParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (fw_entry, fw) = resolve_device!(
            self,
            find_filter_wheel,
            &params.filter_wheel_id,
            "filter wheel"
        );

        let position = match fw.position().await {
            Ok(Some(p)) => p,
            Ok(None) => return Ok(tool_error!("filter wheel is moving")),
            Err(e) => {
                return Ok(tool_error!("failed to get filter position: {}", e));
            }
        };

        let filter_name = fw_entry
            .config
            .filters
            .get(position)
            .cloned()
            .unwrap_or_else(|| format!("Filter {}", position));

        Ok(tool_success!({
            "filter_wheel_id": params.filter_wheel_id,
            "filter_name": filter_name,
            "position": position,
        }))
    }
}
