use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tracing::debug;
use uuid::Uuid;

use crate::equipment::EquipmentRegistry;
use crate::events::EventBus;
use crate::session::SessionConfig;

pub struct McpHandler {
    pub equipment: Arc<EquipmentRegistry>,
    pub event_bus: Arc<EventBus>,
    pub session_config: SessionConfig,
}

impl McpHandler {
    pub async fn handle_request(&self, body: Value) -> Value {
        let id = body.get("id").cloned().unwrap_or(Value::Null);
        let method = body.get("method").and_then(|v| v.as_str()).unwrap_or("");

        debug!(method = %method, "handling MCP request");

        match method {
            "tools/list" => self.handle_tools_list(id),
            "tools/call" => {
                let params = body.get("params").cloned().unwrap_or(Value::Null);
                self.handle_tools_call(id, params).await
            }
            _ => jsonrpc_error(id, &format!("unknown method: {}", method)),
        }
    }

    fn handle_tools_list(&self, id: Value) -> Value {
        let tools = serde_json::json!({
            "tools": [
                {
                    "name": "capture",
                    "description": "Capture an image with a camera",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "camera_id": {"type": "string"},
                            "duration_secs": {"type": "number"}
                        },
                        "required": ["camera_id", "duration_secs"]
                    }
                },
                {
                    "name": "set_filter",
                    "description": "Set the active filter on a filter wheel",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "filter_wheel_id": {"type": "string"},
                            "filter_name": {"type": "string"}
                        },
                        "required": ["filter_wheel_id", "filter_name"]
                    }
                },
                {
                    "name": "get_filter",
                    "description": "Get the current filter on a filter wheel",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "filter_wheel_id": {"type": "string"}
                        },
                        "required": ["filter_wheel_id"]
                    }
                }
            ]
        });

        jsonrpc_success(id, tools)
    }

    async fn handle_tools_call(&self, id: Value, params: Value) -> Value {
        let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);

        debug!(tool = %tool_name, "calling tool");

        match tool_name {
            "capture" => self.tool_capture(id, arguments).await,
            "set_filter" => self.tool_set_filter(id, arguments).await,
            "get_filter" => self.tool_get_filter(id, arguments).await,
            _ => jsonrpc_error(id, &format!("unknown tool: {}", tool_name)),
        }
    }

    async fn tool_capture(&self, id: Value, args: Value) -> Value {
        let camera_id = match args.get("camera_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return jsonrpc_error(id, "missing camera_id"),
        };
        let duration_secs = match args.get("duration_secs").and_then(|v| v.as_f64()) {
            Some(d) => d,
            None => return jsonrpc_error(id, "missing duration_secs"),
        };

        let cam_entry = match self.equipment.find_camera(camera_id) {
            Some(e) => e,
            None => return jsonrpc_error(id, &format!("camera not found: {}", camera_id)),
        };

        let cam = match &cam_entry.device {
            Some(d) => d.clone(),
            None => return jsonrpc_error(id, &format!("camera not connected: {}", camera_id)),
        };

        let document_id = Uuid::new_v4().to_string();
        let image_path = format!(
            "{}/capture_{}.fits",
            self.session_config.data_directory, document_id
        );

        // Emit exposure_started
        self.event_bus.emit(
            "exposure_started",
            serde_json::json!({
                "camera_id": camera_id,
                "duration_secs": duration_secs,
            }),
        );

        // Start exposure
        let duration = Duration::from_secs_f64(duration_secs);
        if let Err(e) = cam.start_exposure(duration, true).await {
            return jsonrpc_error(id, &format!("failed to start exposure: {}", e));
        }

        // Poll for image ready
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            match cam.image_ready().await {
                Ok(true) => break,
                Ok(false) => continue,
                Err(e) => {
                    return jsonrpc_error(id, &format!("error checking image ready: {}", e));
                }
            }
        }

        // Create placeholder file
        let _ = std::fs::create_dir_all(&self.session_config.data_directory);
        let _ = std::fs::write(&image_path, b"");

        // Emit exposure_complete
        self.event_bus.emit(
            "exposure_complete",
            serde_json::json!({
                "document_id": document_id,
                "file_path": image_path,
            }),
        );

        jsonrpc_success(
            id,
            serde_json::json!({
                "image_path": image_path,
                "document_id": document_id,
            }),
        )
    }

    async fn tool_set_filter(&self, id: Value, args: Value) -> Value {
        let fw_id = match args.get("filter_wheel_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return jsonrpc_error(id, "missing filter_wheel_id"),
        };
        let filter_name = match args.get("filter_name").and_then(|v| v.as_str()) {
            Some(n) => n,
            None => return jsonrpc_error(id, "missing filter_name"),
        };

        let fw_entry = match self.equipment.find_filter_wheel(fw_id) {
            Some(e) => e,
            None => return jsonrpc_error(id, &format!("filter wheel not found: {}", fw_id)),
        };

        let position = match fw_entry
            .config
            .filters
            .iter()
            .position(|f| f == filter_name)
        {
            Some(p) => p,
            None => return jsonrpc_error(id, &format!("filter not found: {}", filter_name)),
        };

        let fw = match &fw_entry.device {
            Some(d) => d.clone(),
            None => return jsonrpc_error(id, &format!("filter wheel not connected: {}", fw_id)),
        };

        if let Err(e) = fw.set_position(position).await {
            return jsonrpc_error(id, &format!("failed to set filter position: {}", e));
        }

        // Wait for the filter wheel to reach the target position
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            match fw.position().await {
                Ok(Some(p)) if p == position => break,
                Ok(Some(_)) | Ok(None) => continue,
                Err(e) => {
                    return jsonrpc_error(id, &format!("error waiting for filter wheel: {}", e));
                }
            }
        }

        // Emit filter_switch event
        self.event_bus.emit(
            "filter_switch",
            serde_json::json!({
                "filter_wheel_id": fw_id,
                "filter_name": filter_name,
            }),
        );

        jsonrpc_success(
            id,
            serde_json::json!({
                "filter_wheel_id": fw_id,
                "filter_name": filter_name,
                "position": position,
            }),
        )
    }

    async fn tool_get_filter(&self, id: Value, args: Value) -> Value {
        let fw_id = match args.get("filter_wheel_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return jsonrpc_error(id, "missing filter_wheel_id"),
        };

        let fw_entry = match self.equipment.find_filter_wheel(fw_id) {
            Some(e) => e,
            None => return jsonrpc_error(id, &format!("filter wheel not found: {}", fw_id)),
        };

        let fw = match &fw_entry.device {
            Some(d) => d.clone(),
            None => return jsonrpc_error(id, &format!("filter wheel not connected: {}", fw_id)),
        };

        let position = match fw.position().await {
            Ok(Some(p)) => p,
            Ok(None) => {
                return jsonrpc_error(id, "filter wheel is moving");
            }
            Err(e) => {
                return jsonrpc_error(id, &format!("failed to get filter position: {}", e));
            }
        };

        let filter_name = fw_entry
            .config
            .filters
            .get(position)
            .cloned()
            .unwrap_or_else(|| format!("Filter {}", position));

        jsonrpc_success(
            id,
            serde_json::json!({
                "filter_wheel_id": fw_id,
                "filter_name": filter_name,
                "position": position,
            }),
        )
    }
}

fn jsonrpc_success(id: Value, result: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn jsonrpc_error(id: Value, message: &str) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "message": message,
        },
    })
}
