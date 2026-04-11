use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::cover_calibrator::{CalibratorStatus, CoverStatus};
use ascom_alpaca::api::CoverCalibrator;
use serde_json::Value;
use tracing::debug;
use uuid::Uuid;

use crate::equipment::EquipmentRegistry;
use crate::events::EventBus;
use crate::imaging;
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
                    "description": "Capture an image, download image_array, save FITS file",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "camera_id": {"type": "string"},
                            "duration_ms": {"type": "integer", "description": "Exposure time in milliseconds"}
                        },
                        "required": ["camera_id", "duration_ms"]
                    }
                },
                {
                    "name": "get_camera_info",
                    "description": "Read camera capabilities: max_adu, exposure limits, sensor dimensions",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "camera_id": {"type": "string"}
                        },
                        "required": ["camera_id"]
                    }
                },
                {
                    "name": "compute_image_stats",
                    "description": "Read FITS file and compute pixel statistics (median, mean, min, max ADU)",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "image_path": {"type": "string"},
                            "document_id": {"type": "string", "description": "Optional: update exposure document with stats"}
                        },
                        "required": ["image_path"]
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
                },
                {
                    "name": "close_cover",
                    "description": "Close the dust cover (blocks until closed)",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "calibrator_id": {"type": "string"}
                        },
                        "required": ["calibrator_id"]
                    }
                },
                {
                    "name": "open_cover",
                    "description": "Open the dust cover (blocks until open)",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "calibrator_id": {"type": "string"}
                        },
                        "required": ["calibrator_id"]
                    }
                },
                {
                    "name": "calibrator_on",
                    "description": "Turn on flat panel at brightness (default: max). Blocks until ready",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "calibrator_id": {"type": "string"},
                            "brightness": {"type": "integer", "description": "0..max_brightness, default max"}
                        },
                        "required": ["calibrator_id"]
                    }
                },
                {
                    "name": "calibrator_off",
                    "description": "Turn off flat panel. Blocks until off",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "calibrator_id": {"type": "string"}
                        },
                        "required": ["calibrator_id"]
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
            "get_camera_info" => self.tool_get_camera_info(id, arguments).await,
            "compute_image_stats" => self.tool_compute_image_stats(id, arguments).await,
            "set_filter" => self.tool_set_filter(id, arguments).await,
            "get_filter" => self.tool_get_filter(id, arguments).await,
            "close_cover" => self.tool_close_cover(id, arguments).await,
            "open_cover" => self.tool_open_cover(id, arguments).await,
            "calibrator_on" => self.tool_calibrator_on(id, arguments).await,
            "calibrator_off" => self.tool_calibrator_off(id, arguments).await,
            _ => jsonrpc_error(id, &format!("unknown tool: {}", tool_name)),
        }
    }

    // -----------------------------------------------------------------------
    // Camera tools
    // -----------------------------------------------------------------------

    async fn tool_capture(&self, id: Value, args: Value) -> Value {
        let camera_id = match args.get("camera_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return jsonrpc_error(id, "missing camera_id"),
        };

        // Accept duration_ms (preferred) or duration_secs (backward compat)
        let duration = if let Some(ms) = args.get("duration_ms").and_then(|v| v.as_u64()) {
            Duration::from_millis(ms)
        } else if let Some(secs) = args.get("duration_secs").and_then(|v| v.as_f64()) {
            Duration::from_secs_f64(secs)
        } else {
            return jsonrpc_error(id, "missing duration_ms");
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
                "duration_ms": duration.as_millis() as u64,
            }),
        );

        // Start exposure
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

        // Download image_array and save as FITS
        let image_array = match cam.image_array().await {
            Ok(arr) => arr,
            Err(e) => {
                return jsonrpc_error(id, &format!("failed to download image array: {}", e));
            }
        };

        // ImageArray derefs to ArcArray3<i32> (ndarray).
        // dim() returns (x, y, color_planes). For monochrome, color_planes == 1.
        let (dim_x, dim_y, _planes) = image_array.dim();
        let width = dim_x as u32;
        let height = dim_y as u32;

        let pixels: Vec<i32> = image_array.iter().copied().collect();

        if let Err(e) = imaging::write_fits(&image_path, &pixels, width, height).await {
            return jsonrpc_error(id, &format!("failed to write FITS file: {}", e));
        }

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

    async fn tool_get_camera_info(&self, id: Value, args: Value) -> Value {
        let camera_id = match args.get("camera_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return jsonrpc_error(id, "missing camera_id"),
        };

        let cam_entry = match self.equipment.find_camera(camera_id) {
            Some(e) => e,
            None => return jsonrpc_error(id, &format!("camera not found: {}", camera_id)),
        };

        let cam = match &cam_entry.device {
            Some(d) => d.clone(),
            None => return jsonrpc_error(id, &format!("camera not connected: {}", camera_id)),
        };

        let max_adu = match cam.max_adu().await {
            Ok(v) => v,
            Err(e) => return jsonrpc_error(id, &format!("failed to read max_adu: {}", e)),
        };

        let (sensor_x, sensor_y) = match cam.camera_size().await {
            Ok(size) => (size[0], size[1]),
            Err(e) => return jsonrpc_error(id, &format!("failed to read sensor size: {}", e)),
        };

        let (bin_x, bin_y) = match cam.bin().await {
            Ok(bin) => (bin[0] as u32, bin[1] as u32),
            Err(e) => {
                debug!(error = %e, "failed to read binning, using defaults");
                (1u32, 1u32)
            }
        };

        let (exposure_min_ms, exposure_max_ms) = match cam.exposure_range().await {
            Ok(range) => (
                range.start().as_millis() as u64,
                range.end().as_millis() as u64,
            ),
            Err(e) => {
                debug!(error = %e, "failed to read exposure range, using defaults");
                (1u64, 3600000u64) // 1ms to 1 hour
            }
        };

        jsonrpc_success(
            id,
            serde_json::json!({
                "camera_id": camera_id,
                "max_adu": max_adu,
                "sensor_x": sensor_x,
                "sensor_y": sensor_y,
                "bin_x": bin_x,
                "bin_y": bin_y,
                "exposure_min_ms": exposure_min_ms,
                "exposure_max_ms": exposure_max_ms,
            }),
        )
    }

    // -----------------------------------------------------------------------
    // Image stats tool
    // -----------------------------------------------------------------------

    async fn tool_compute_image_stats(&self, id: Value, args: Value) -> Value {
        let image_path = match args.get("image_path").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => return jsonrpc_error(id, "missing image_path"),
        };

        let _document_id = args.get("document_id").and_then(|v| v.as_str());

        // Read FITS and compute stats in a blocking task (file I/O).
        let path_clone = image_path.clone();
        let stats = match tokio::task::spawn_blocking(move || {
            let pixels = imaging::read_fits_pixels(&path_clone)?;
            imaging::compute_stats(&pixels)
                .ok_or_else(|| crate::error::RpError::Imaging("image has no pixels".into()))
        })
        .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return jsonrpc_error(id, &format!("failed to compute stats: {}", e)),
            Err(e) => return jsonrpc_error(id, &format!("task error: {}", e)),
        };

        debug!(
            image_path = %image_path,
            median = stats.median_adu,
            mean = %stats.mean_adu,
            "computed image stats"
        );

        jsonrpc_success(
            id,
            serde_json::json!({
                "median_adu": stats.median_adu,
                "mean_adu": stats.mean_adu,
                "min_adu": stats.min_adu,
                "max_adu": stats.max_adu,
                "pixel_count": stats.pixel_count,
            }),
        )
    }

    // -----------------------------------------------------------------------
    // Filter wheel tools
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // CoverCalibrator tools
    // -----------------------------------------------------------------------

    async fn tool_close_cover(&self, id: Value, args: Value) -> Value {
        let (cc_id, cc, poll_interval) = match self.resolve_calibrator(&id, &args) {
            Ok(v) => v,
            Err(e) => return e,
        };

        debug!(calibrator_id = %cc_id, "closing cover");
        if let Err(e) = cc.close_cover().await {
            return jsonrpc_error(id, &format!("failed to close cover: {}", e));
        }

        // Poll at 3s intervals. CoverCalibrator operations are physical
        // (cover motors, lamp stabilization) so sub-second polling wastes
        // bandwidth. 3s aligns well with typical device timers (2-5s).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            tokio::time::sleep(poll_interval).await;
            match cc.cover_state().await {
                Ok(CoverStatus::Closed) => {
                    debug!(calibrator_id = %cc_id, "cover closed");
                    return jsonrpc_success(id, serde_json::json!({"status": "closed"}));
                }
                Ok(_) if tokio::time::Instant::now() < deadline => continue,
                Ok(_) => break,
                Err(e) => {
                    return jsonrpc_error(id, &format!("error polling cover state: {}", e));
                }
            }
        }

        jsonrpc_error(id, "timeout waiting for cover to close")
    }

    async fn tool_open_cover(&self, id: Value, args: Value) -> Value {
        let (cc_id, cc, poll_interval) = match self.resolve_calibrator(&id, &args) {
            Ok(v) => v,
            Err(e) => return e,
        };

        debug!(calibrator_id = %cc_id, "opening cover");
        if let Err(e) = cc.open_cover().await {
            return jsonrpc_error(id, &format!("failed to open cover: {}", e));
        }

        // Poll until open
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            tokio::time::sleep(poll_interval).await;
            match cc.cover_state().await {
                Ok(CoverStatus::Open) => {
                    debug!(calibrator_id = %cc_id, "cover opened");
                    return jsonrpc_success(id, serde_json::json!({"status": "open"}));
                }
                Ok(_) if tokio::time::Instant::now() < deadline => continue,
                Ok(_) => break,
                Err(e) => {
                    return jsonrpc_error(id, &format!("error polling cover state: {}", e));
                }
            }
        }

        jsonrpc_error(id, "timeout waiting for cover to open")
    }

    async fn tool_calibrator_on(&self, id: Value, args: Value) -> Value {
        let (cc_id, cc, poll_interval) = match self.resolve_calibrator(&id, &args) {
            Ok(v) => v,
            Err(e) => return e,
        };

        // Determine brightness: use provided value or max_brightness
        let brightness = if let Some(b) = args.get("brightness").and_then(|v| v.as_u64()) {
            b as u32
        } else {
            match cc.max_brightness().await {
                Ok(max) => max,
                Err(e) => {
                    return jsonrpc_error(id, &format!("failed to read max_brightness: {}", e))
                }
            }
        };

        debug!(calibrator_id = %cc_id, brightness = brightness, "turning calibrator on");
        if let Err(e) = cc.calibrator_on(brightness).await {
            return jsonrpc_error(id, &format!("failed to turn calibrator on: {}", e));
        }

        // Poll until ready
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            tokio::time::sleep(poll_interval).await;
            match cc.calibrator_state().await {
                Ok(CalibratorStatus::Ready) => {
                    debug!(calibrator_id = %cc_id, "calibrator ready");
                    return jsonrpc_success(
                        id,
                        serde_json::json!({"status": "ready", "brightness": brightness}),
                    );
                }
                Ok(_) if tokio::time::Instant::now() < deadline => continue,
                Ok(_) => break,
                Err(e) => {
                    return jsonrpc_error(id, &format!("error polling calibrator state: {}", e));
                }
            }
        }

        jsonrpc_error(id, "timeout waiting for calibrator to become ready")
    }

    async fn tool_calibrator_off(&self, id: Value, args: Value) -> Value {
        let (cc_id, cc, poll_interval) = match self.resolve_calibrator(&id, &args) {
            Ok(v) => v,
            Err(e) => return e,
        };

        debug!(calibrator_id = %cc_id, "turning calibrator off");
        if let Err(e) = cc.calibrator_off().await {
            return jsonrpc_error(id, &format!("failed to turn calibrator off: {}", e));
        }

        // Poll until off
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            tokio::time::sleep(poll_interval).await;
            match cc.calibrator_state().await {
                Ok(CalibratorStatus::Off) => {
                    debug!(calibrator_id = %cc_id, "calibrator off");
                    return jsonrpc_success(id, serde_json::json!({"status": "off"}));
                }
                Ok(_) if tokio::time::Instant::now() < deadline => continue,
                Ok(_) => break,
                Err(e) => {
                    return jsonrpc_error(id, &format!("error polling calibrator state: {}", e));
                }
            }
        }

        jsonrpc_error(id, "timeout waiting for calibrator to turn off")
    }

    /// Helper: resolve calibrator_id from args, look up device and poll interval.
    fn resolve_calibrator(
        &self,
        id: &Value,
        args: &Value,
    ) -> Result<(String, Arc<dyn CoverCalibrator>, Duration), Value> {
        let cc_id = match args.get("calibrator_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => return Err(jsonrpc_error(id.clone(), "missing calibrator_id")),
        };

        let cc_entry = match self.equipment.find_cover_calibrator(&cc_id) {
            Some(e) => e,
            None => {
                return Err(jsonrpc_error(
                    id.clone(),
                    &format!("calibrator not found: {}", cc_id),
                ))
            }
        };

        let cc = match &cc_entry.device {
            Some(d) => d.clone(),
            None => {
                return Err(jsonrpc_error(
                    id.clone(),
                    &format!("calibrator not connected: {}", cc_id),
                ))
            }
        };

        let poll_interval = Duration::from_secs(cc_entry.config.poll_interval_secs);

        Ok((cc_id, cc, poll_interval))
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
