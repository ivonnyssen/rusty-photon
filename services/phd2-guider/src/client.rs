//! PHD2 client for communicating with PHD2 via JSON RPC

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::broadcast;
use tracing::debug;

use crate::config::{Phd2Config, SettleParams};
use crate::connection::{
    spawn_reader_task, ConnectionConfig, ConnectionState, PendingRequest, SharedConnectionState,
};
use crate::error::{Phd2Error, Result};
use crate::events::{AppState, Phd2Event};
use crate::io::{ConnectionFactory, TcpConnectionFactory};
use crate::rpc::RpcRequest;
use crate::types::{
    CalibrationData, CalibrationTarget, CoolerStatus, Equipment, GuideAxis, Profile, Rect,
    StarImage,
};

/// PHD2 client for communicating with PHD2 via JSON RPC
pub struct Phd2Client {
    config: Phd2Config,
    request_id: AtomicU64,
    shared: SharedConnectionState,
    reconnect_handle: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    connection_factory: Arc<dyn ConnectionFactory>,
}

impl Phd2Client {
    /// Create a new PHD2 client with the given configuration
    ///
    /// Uses the default TCP connection factory for production use.
    pub fn new(config: Phd2Config) -> Self {
        Self::with_connection_factory(config, Arc::new(TcpConnectionFactory::new()))
    }

    /// Create a new PHD2 client with a custom connection factory
    ///
    /// This is useful for testing with mock connections.
    pub fn with_connection_factory(
        config: Phd2Config,
        connection_factory: Arc<dyn ConnectionFactory>,
    ) -> Self {
        let auto_reconnect_enabled = config.reconnect.enabled;
        Self {
            config,
            request_id: AtomicU64::new(1),
            shared: SharedConnectionState::with_factory(
                auto_reconnect_enabled,
                connection_factory.clone(),
            ),
            reconnect_handle: tokio::sync::Mutex::new(None),
            connection_factory,
        }
    }

    /// Get the connection config for reconnection
    fn get_connection_config(&self) -> ConnectionConfig {
        ConnectionConfig {
            host: self.config.host.clone(),
            port: self.config.port,
            connection_timeout_seconds: self.config.connection_timeout_seconds,
            reconnect: self.config.reconnect.clone(),
        }
    }

    /// Connect to a running PHD2 instance
    pub async fn connect(&self) -> Result<()> {
        // Stop any ongoing reconnection attempt
        self.shared.stop_reconnect.notify_waiters();
        {
            let mut handle = self.reconnect_handle.lock().await;
            if let Some(h) = handle.take() {
                h.abort();
            }
        }

        let addr = format!("{}:{}", self.config.host, self.config.port);
        debug!("Connecting to PHD2 at {}", addr);

        let timeout_duration =
            std::time::Duration::from_secs(self.config.connection_timeout_seconds);

        let connection_pair = self
            .connection_factory
            .connect(&addr, timeout_duration)
            .await?;

        debug!("Connection established to PHD2");

        // Store the writer
        {
            let mut writer_guard = self.shared.writer.lock().await;
            *writer_guard = Some(connection_pair.writer);
        }

        // Update connection state
        {
            let mut state = self.shared.state.write().await;
            state.connected = true;
            state.reconnecting = false;
        }

        // Start the reader task
        let reader_handle = spawn_reader_task(
            connection_pair.reader,
            self.get_connection_config(),
            self.shared.clone(),
        );
        {
            let mut handle_guard = self.shared.reader_handle.lock().await;
            *handle_guard = Some(reader_handle);
        }

        debug!("PHD2 client connected and reader task started");
        Ok(())
    }

    /// Disconnect from PHD2
    ///
    /// This will stop any ongoing reconnection attempts and cleanly disconnect
    /// from PHD2. After calling this, auto-reconnect will not trigger unless
    /// you manually call `connect()` again.
    pub async fn disconnect(&self) -> Result<()> {
        debug!("Disconnecting from PHD2");

        // Stop any ongoing reconnection attempt
        self.shared.stop_reconnect.notify_waiters();
        {
            let mut handle = self.reconnect_handle.lock().await;
            if let Some(h) = handle.take() {
                h.abort();
            }
        }

        // Abort the reader task
        {
            let mut handle = self.shared.reader_handle.lock().await;
            if let Some(h) = handle.take() {
                h.abort();
            }
        }

        // Close the writer
        {
            let mut writer = self.shared.writer.lock().await;
            if let Some(mut w) = writer.take() {
                let _ = w.shutdown().await;
            }
        }

        // Update state
        {
            let mut state = self.shared.state.write().await;
            *state = ConnectionState::default();
        }

        // Clear pending requests
        {
            let mut pending = self.shared.pending_requests.lock().await;
            pending.clear();
        }

        debug!("Disconnected from PHD2");
        Ok(())
    }

    /// Check if connected to PHD2
    pub async fn is_connected(&self) -> bool {
        self.shared.is_connected().await
    }

    /// Get the PHD2 version (available after connection)
    pub async fn get_phd2_version(&self) -> Option<String> {
        self.shared.get_phd2_version().await
    }

    /// Subscribe to PHD2 events
    pub fn subscribe(&self) -> broadcast::Receiver<Phd2Event> {
        self.shared.event_sender.subscribe()
    }

    // ========================================================================
    // Auto-reconnect Control Methods
    // ========================================================================

    /// Check if auto-reconnect is currently enabled
    pub fn is_auto_reconnect_enabled(&self) -> bool {
        self.shared.is_auto_reconnect_enabled()
    }

    /// Enable or disable auto-reconnect
    ///
    /// When disabled during an active reconnection attempt, the attempt will
    /// be stopped after the current connection try completes.
    pub fn set_auto_reconnect_enabled(&self, enabled: bool) {
        self.shared.set_auto_reconnect_enabled(enabled);
    }

    /// Check if the client is currently attempting to reconnect
    pub async fn is_reconnecting(&self) -> bool {
        self.shared.is_reconnecting().await
    }

    /// Stop any ongoing reconnection attempts
    ///
    /// This stops the current reconnection task without disabling auto-reconnect.
    /// If the connection is lost again in the future, reconnection will be attempted.
    pub async fn stop_reconnection(&self) {
        self.shared.stop_reconnection().await;
        {
            let mut handle = self.reconnect_handle.lock().await;
            if let Some(h) = handle.take() {
                h.abort();
            }
        }
    }

    /// Send an RPC request and wait for response
    async fn send_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        if !self.is_connected().await {
            return Err(Phd2Error::NotConnected);
        }

        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let request = RpcRequest::new(method, params, id);
        let request_json = serde_json::to_string(&request)?;

        debug!("Sending RPC request: {}", request_json);

        // Create a oneshot channel for the response
        let (sender, receiver) = tokio::sync::oneshot::channel();

        // Register the pending request
        {
            let mut pending = self.shared.pending_requests.lock().await;
            pending.insert(id, PendingRequest { sender });
        }

        // Send the request using the MessageWriter trait
        {
            let mut writer_guard = self.shared.writer.lock().await;
            if let Some(writer) = writer_guard.as_mut() {
                writer.write_message(&request_json).await?;
            } else {
                return Err(Phd2Error::NotConnected);
            }
        }

        // Wait for response with timeout
        let timeout_duration = std::time::Duration::from_secs(self.config.command_timeout_seconds);
        tokio::time::timeout(timeout_duration, receiver)
            .await
            .map_err(|_| Phd2Error::Timeout(format!("Request '{}' timed out", method)))?
            .map_err(|_| Phd2Error::ReceiveError)?
    }

    // ========================================================================
    // State and Status Methods
    // ========================================================================

    /// Get the current PHD2 application state
    pub async fn get_app_state(&self) -> Result<AppState> {
        let result = self.send_request("get_app_state", None).await?;
        let state_str = result
            .as_str()
            .ok_or_else(|| Phd2Error::InvalidState("Expected string for app state".to_string()))?;
        state_str.parse()
    }

    /// Get cached application state (from events, no RPC call)
    pub async fn get_cached_app_state(&self) -> Option<AppState> {
        self.shared.get_cached_app_state().await
    }

    // ========================================================================
    // Equipment and Profile Methods
    // ========================================================================

    /// Check if equipment is connected
    pub async fn is_equipment_connected(&self) -> Result<bool> {
        let result = self.send_request("get_connected", None).await?;
        result.as_bool().ok_or_else(|| {
            Phd2Error::InvalidState("Expected boolean for connected state".to_string())
        })
    }

    /// Connect all equipment in current profile
    pub async fn connect_equipment(&self) -> Result<()> {
        self.send_request("set_connected", Some(serde_json::json!(true)))
            .await?;
        Ok(())
    }

    /// Disconnect all equipment
    pub async fn disconnect_equipment(&self) -> Result<()> {
        self.send_request("set_connected", Some(serde_json::json!(false)))
            .await?;
        Ok(())
    }

    /// Get list of available equipment profiles
    pub async fn get_profiles(&self) -> Result<Vec<Profile>> {
        let result = self.send_request("get_profiles", None).await?;
        let profiles: Vec<Profile> = serde_json::from_value(result)?;
        Ok(profiles)
    }

    /// Get current active profile
    pub async fn get_current_profile(&self) -> Result<Profile> {
        let result = self.send_request("get_profile", None).await?;
        let profile: Profile = serde_json::from_value(result)?;
        Ok(profile)
    }

    /// Set active profile (equipment must be disconnected)
    pub async fn set_profile(&self, profile_id: i32) -> Result<()> {
        self.send_request("set_profile", Some(serde_json::json!({"id": profile_id})))
            .await?;
        Ok(())
    }

    /// Get current equipment configuration
    ///
    /// Returns information about all equipment devices in the current profile,
    /// including camera, mount, auxiliary mount, adaptive optics, and rotator.
    pub async fn get_current_equipment(&self) -> Result<Equipment> {
        debug!("Getting current equipment configuration");
        let result = self.send_request("get_current_equipment", None).await?;
        let equipment: Equipment = serde_json::from_value(result)?;
        Ok(equipment)
    }

    // ========================================================================
    // Application Control Methods
    // ========================================================================

    /// Shutdown PHD2 application
    pub async fn shutdown_phd2(&self) -> Result<()> {
        self.send_request("shutdown", None).await?;
        Ok(())
    }

    // ========================================================================
    // Guiding Control Methods
    // ========================================================================

    /// Start guiding with settling parameters
    ///
    /// # Arguments
    /// * `settle` - Settling parameters (pixels threshold, time, timeout)
    /// * `recalibrate` - If true, recalibrate before guiding
    /// * `roi` - Optional region of interest for star selection
    pub async fn start_guiding(
        &self,
        settle: &SettleParams,
        recalibrate: bool,
        roi: Option<Rect>,
    ) -> Result<()> {
        debug!(
            "Starting guiding with settle: pixels={}, time={}, timeout={}, recalibrate={}",
            settle.pixels, settle.time, settle.timeout, recalibrate
        );

        let settle_obj = serde_json::json!({
            "pixels": settle.pixels,
            "time": settle.time,
            "timeout": settle.timeout
        });

        let mut params = serde_json::json!({
            "settle": settle_obj,
            "recalibrate": recalibrate
        });

        if let Some(rect) = roi {
            params["roi"] = serde_json::json!([rect.x, rect.y, rect.width, rect.height]);
        }

        self.send_request("guide", Some(params)).await?;
        Ok(())
    }

    /// Stop guiding but continue looping exposures
    ///
    /// This is equivalent to calling `loop` - it stops guiding but keeps
    /// the camera capturing frames.
    pub async fn stop_guiding(&self) -> Result<()> {
        debug!("Stopping guiding (continuing loop)");
        self.send_request("loop", None).await?;
        Ok(())
    }

    /// Stop all capture and guiding operations
    pub async fn stop_capture(&self) -> Result<()> {
        debug!("Stopping capture");
        self.send_request("stop_capture", None).await?;
        Ok(())
    }

    /// Start looping exposures without guiding
    ///
    /// If currently guiding, this will stop guiding but continue capturing frames.
    /// If stopped, this will start capturing frames.
    pub async fn start_loop(&self) -> Result<()> {
        debug!("Starting loop");
        self.send_request("loop", None).await?;
        Ok(())
    }

    /// Check if guiding is currently paused
    pub async fn is_paused(&self) -> Result<bool> {
        let result = self.send_request("get_paused", None).await?;
        result
            .as_bool()
            .ok_or_else(|| Phd2Error::InvalidState("Expected boolean for paused state".to_string()))
    }

    /// Pause guiding
    ///
    /// # Arguments
    /// * `full` - If true, pause looping entirely (no exposures). If false, continue
    ///   looping but don't send guide corrections.
    pub async fn pause(&self, full: bool) -> Result<()> {
        debug!("Pausing guiding (full={})", full);
        let params = if full {
            serde_json::json!({"paused": true, "full": "full"})
        } else {
            serde_json::json!({"paused": true})
        };
        self.send_request("set_paused", Some(params)).await?;
        Ok(())
    }

    /// Resume guiding after pause
    pub async fn resume(&self) -> Result<()> {
        debug!("Resuming guiding");
        self.send_request("set_paused", Some(serde_json::json!({"paused": false})))
            .await?;
        Ok(())
    }

    /// Dither the guide position
    ///
    /// Shifts the lock position by a random amount up to `amount` pixels.
    /// Used between exposures to reduce fixed pattern noise.
    ///
    /// # Arguments
    /// * `amount` - Maximum dither distance in pixels
    /// * `ra_only` - If true, only dither in RA axis
    /// * `settle` - Settling parameters to wait for after dither
    pub async fn dither(&self, amount: f64, ra_only: bool, settle: &SettleParams) -> Result<()> {
        debug!(
            "Dithering: amount={}, ra_only={}, settle: pixels={}, time={}, timeout={}",
            amount, ra_only, settle.pixels, settle.time, settle.timeout
        );

        let settle_obj = serde_json::json!({
            "pixels": settle.pixels,
            "time": settle.time,
            "timeout": settle.timeout
        });

        let params = serde_json::json!({
            "amount": amount,
            "raOnly": ra_only,
            "settle": settle_obj
        });

        self.send_request("dither", Some(params)).await?;
        Ok(())
    }

    // ========================================================================
    // Star Selection Methods
    // ========================================================================

    /// Auto-select a guide star
    ///
    /// PHD2 will search for a suitable guide star. If a region of interest (ROI)
    /// is provided, the search will be limited to that region.
    ///
    /// # Arguments
    /// * `roi` - Optional region of interest for star selection
    pub async fn find_star(&self, roi: Option<Rect>) -> Result<()> {
        debug!(
            "Finding star{}",
            roi.map_or(String::new(), |r| format!(
                " in ROI [{},{},{},{}]",
                r.x, r.y, r.width, r.height
            ))
        );

        let params = roi.map(|r| serde_json::json!([r.x, r.y, r.width, r.height]));

        self.send_request("find_star", params).await?;
        Ok(())
    }

    /// Get the current lock position (guide star coordinates)
    ///
    /// Returns the x,y coordinates of the current lock position.
    /// Returns an error if no star is currently selected.
    pub async fn get_lock_position(&self) -> Result<(f64, f64)> {
        debug!("Getting lock position");
        let result = self.send_request("get_lock_position", None).await?;

        // PHD2 returns null when no star is selected
        if result.is_null() {
            return Err(Phd2Error::InvalidState(
                "No star selected - lock position not available".to_string(),
            ));
        }

        // PHD2 returns [x, y]
        let arr = result.as_array().ok_or_else(|| {
            Phd2Error::InvalidState("Expected array for lock position".to_string())
        })?;

        if arr.len() != 2 {
            return Err(Phd2Error::InvalidState(format!(
                "Expected 2 elements for lock position, got {}",
                arr.len()
            )));
        }

        let x = arr[0].as_f64().ok_or_else(|| {
            Phd2Error::InvalidState("Expected number for x coordinate".to_string())
        })?;
        let y = arr[1].as_f64().ok_or_else(|| {
            Phd2Error::InvalidState("Expected number for y coordinate".to_string())
        })?;

        Ok((x, y))
    }

    /// Set the lock position (guide star coordinates)
    ///
    /// # Arguments
    /// * `x` - X coordinate for the lock position
    /// * `y` - Y coordinate for the lock position
    /// * `exact` - If true, use the exact position. If false, PHD2 will search
    ///   for a nearby star to use as the guide star.
    pub async fn set_lock_position(&self, x: f64, y: f64, exact: bool) -> Result<()> {
        debug!("Setting lock position to ({}, {}), exact={}", x, y, exact);

        let params = serde_json::json!({
            "X": x,
            "Y": y,
            "EXACT": exact
        });

        self.send_request("set_lock_position", Some(params)).await?;
        Ok(())
    }

    // ========================================================================
    // Calibration Methods
    // ========================================================================

    /// Check if the mount is calibrated
    pub async fn is_calibrated(&self) -> Result<bool> {
        debug!("Checking calibration status");
        let result = self.send_request("get_calibrated", None).await?;
        result.as_bool().ok_or_else(|| {
            Phd2Error::InvalidState("Expected boolean for calibrated state".to_string())
        })
    }

    /// Get calibration data for the specified target
    ///
    /// # Arguments
    /// * `which` - Which calibration data to retrieve (Mount or AO)
    ///
    /// Note: `CalibrationTarget::Both` is not valid for this method and will
    /// default to returning Mount calibration data.
    pub async fn get_calibration_data(&self, which: CalibrationTarget) -> Result<CalibrationData> {
        debug!("Getting calibration data for {}", which);

        let params = serde_json::json!({
            "which": which.to_get_api_string()
        });

        let result = self
            .send_request("get_calibration_data", Some(params))
            .await?;
        let data: CalibrationData = serde_json::from_value(result)?;
        Ok(data)
    }

    /// Clear calibration data
    ///
    /// # Arguments
    /// * `which` - Which calibration to clear (Mount, AO, or Both)
    pub async fn clear_calibration(&self, which: CalibrationTarget) -> Result<()> {
        debug!("Clearing calibration for {}", which);

        let params = serde_json::json!({
            "which": which.to_clear_api_string()
        });

        self.send_request("clear_calibration", Some(params)).await?;
        Ok(())
    }

    /// Flip calibration for meridian flip
    ///
    /// Inverts the existing calibration data without requiring a full recalibration.
    /// Should be called after performing a meridian flip on the mount.
    pub async fn flip_calibration(&self) -> Result<()> {
        debug!("Flipping calibration");
        self.send_request("flip_calibration", None).await?;
        Ok(())
    }

    // ========================================================================
    // Camera Exposure Methods
    // ========================================================================

    /// Get the current exposure duration in milliseconds
    pub async fn get_exposure(&self) -> Result<u32> {
        debug!("Getting exposure duration");
        let result = self.send_request("get_exposure", None).await?;
        result.as_u64().map(|v| v as u32).ok_or_else(|| {
            Phd2Error::InvalidState("Expected integer for exposure duration".to_string())
        })
    }

    /// Set the exposure duration in milliseconds
    ///
    /// # Arguments
    /// * `exposure_ms` - Exposure duration in milliseconds
    pub async fn set_exposure(&self, exposure_ms: u32) -> Result<()> {
        debug!("Setting exposure duration to {} ms", exposure_ms);
        self.send_request("set_exposure", Some(serde_json::json!(exposure_ms)))
            .await?;
        Ok(())
    }

    /// Get the list of valid exposure durations
    ///
    /// Returns a list of exposure durations in milliseconds that the camera supports.
    pub async fn get_exposure_durations(&self) -> Result<Vec<u32>> {
        debug!("Getting exposure durations");
        let result = self.send_request("get_exposure_durations", None).await?;
        let durations: Vec<u32> = serde_json::from_value(result)?;
        Ok(durations)
    }

    /// Get the camera frame size (image dimensions)
    ///
    /// Returns the width and height of the camera sensor in pixels.
    pub async fn get_camera_frame_size(&self) -> Result<(u32, u32)> {
        debug!("Getting camera frame size");
        let result = self.send_request("get_camera_frame_size", None).await?;

        // PHD2 returns [width, height]
        let arr = result.as_array().ok_or_else(|| {
            Phd2Error::InvalidState("Expected array for camera frame size".to_string())
        })?;

        if arr.len() != 2 {
            return Err(Phd2Error::InvalidState(format!(
                "Expected 2 elements for frame size, got {}",
                arr.len()
            )));
        }

        let width = arr[0]
            .as_u64()
            .map(|v| v as u32)
            .ok_or_else(|| Phd2Error::InvalidState("Expected integer for width".to_string()))?;
        let height = arr[1]
            .as_u64()
            .map(|v| v as u32)
            .ok_or_else(|| Phd2Error::InvalidState("Expected integer for height".to_string()))?;

        Ok((width, height))
    }

    /// Check if subframe mode is enabled
    ///
    /// When subframing is enabled, PHD2 only reads a portion of the camera
    /// sensor around the guide star, which can improve frame rate.
    pub async fn get_use_subframes(&self) -> Result<bool> {
        debug!("Getting use subframes setting");
        let result = self.send_request("get_use_subframes", None).await?;
        result.as_bool().ok_or_else(|| {
            Phd2Error::InvalidState("Expected boolean for use subframes".to_string())
        })
    }

    /// Capture a single frame
    ///
    /// Acquires one frame from the camera. This can be used to preview the
    /// field of view or check focus without starting guiding.
    ///
    /// # Arguments
    /// * `exposure_ms` - Optional exposure duration in milliseconds. If not specified,
    ///   uses the current exposure setting.
    /// * `subframe` - Optional region of interest to capture. If not specified,
    ///   captures the full frame.
    pub async fn capture_single_frame(
        &self,
        exposure_ms: Option<u32>,
        subframe: Option<Rect>,
    ) -> Result<()> {
        debug!(
            "Capturing single frame{}{}",
            exposure_ms.map_or(String::new(), |e| format!(", exposure={}ms", e)),
            subframe.map_or(String::new(), |r| format!(
                ", subframe=[{},{},{},{}]",
                r.x, r.y, r.width, r.height
            ))
        );

        let mut params = serde_json::Map::new();

        if let Some(exp) = exposure_ms {
            params.insert("exposure".to_string(), serde_json::json!(exp));
        }

        if let Some(rect) = subframe {
            params.insert(
                "subframe".to_string(),
                serde_json::json!([rect.x, rect.y, rect.width, rect.height]),
            );
        }

        let params_value = if params.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(params))
        };

        self.send_request("capture_single_frame", params_value)
            .await?;
        Ok(())
    }

    // ========================================================================
    // Guide Algorithm Parameter Methods
    // ========================================================================

    /// Get the list of algorithm parameter names for the specified axis
    ///
    /// Returns a list of parameter names that can be used with `get_algo_param`
    /// and `set_algo_param`.
    ///
    /// # Arguments
    /// * `axis` - The guide axis (RA or Dec)
    pub async fn get_algo_param_names(&self, axis: GuideAxis) -> Result<Vec<String>> {
        debug!("Getting algorithm parameter names for {} axis", axis);

        let params = serde_json::json!({
            "axis": axis.to_api_string()
        });

        let result = self
            .send_request("get_algo_param_names", Some(params))
            .await?;
        let names: Vec<String> = serde_json::from_value(result)?;
        Ok(names)
    }

    /// Get the value of a guide algorithm parameter
    ///
    /// # Arguments
    /// * `axis` - The guide axis (RA or Dec)
    /// * `name` - The parameter name (from `get_algo_param_names`)
    pub async fn get_algo_param(&self, axis: GuideAxis, name: &str) -> Result<f64> {
        debug!("Getting algorithm parameter '{}' for {} axis", name, axis);

        let params = serde_json::json!({
            "axis": axis.to_api_string(),
            "name": name
        });

        let result = self.send_request("get_algo_param", Some(params)).await?;
        result.as_f64().ok_or_else(|| {
            Phd2Error::InvalidState(format!(
                "Expected number for algorithm parameter '{}'",
                name
            ))
        })
    }

    /// Set the value of a guide algorithm parameter
    ///
    /// # Arguments
    /// * `axis` - The guide axis (RA or Dec)
    /// * `name` - The parameter name (from `get_algo_param_names`)
    /// * `value` - The new value for the parameter
    pub async fn set_algo_param(&self, axis: GuideAxis, name: &str, value: f64) -> Result<()> {
        debug!(
            "Setting algorithm parameter '{}' for {} axis to {}",
            name, axis, value
        );

        let params = serde_json::json!({
            "axis": axis.to_api_string(),
            "name": name,
            "value": value
        });

        self.send_request("set_algo_param", Some(params)).await?;
        Ok(())
    }

    // ========================================================================
    // Camera Cooling Methods
    // ========================================================================

    /// Get the current CCD sensor temperature
    ///
    /// Returns the temperature in degrees Celsius.
    /// Returns an error if the camera does not support temperature reading.
    pub async fn get_ccd_temperature(&self) -> Result<f64> {
        debug!("Getting CCD temperature");
        let result = self.send_request("get_ccd_temperature", None).await?;
        result.as_f64().ok_or_else(|| {
            Phd2Error::InvalidState("Expected number for CCD temperature".to_string())
        })
    }

    /// Get the cooler status including temperature and power
    ///
    /// Returns detailed cooler information including current temperature,
    /// whether the cooler is enabled, setpoint temperature, and power level.
    pub async fn get_cooler_status(&self) -> Result<CoolerStatus> {
        debug!("Getting cooler status");
        let result = self.send_request("get_cooler_status", None).await?;
        let status: CoolerStatus = serde_json::from_value(result)?;
        Ok(status)
    }

    /// Set the cooler state
    ///
    /// Enables or disables the camera cooler and optionally sets the target temperature.
    ///
    /// # Arguments
    /// * `enabled` - Whether to enable the cooler
    /// * `temperature` - Target temperature in degrees Celsius (required when enabling)
    pub async fn set_cooler_state(&self, enabled: bool, temperature: Option<f64>) -> Result<()> {
        debug!(
            "Setting cooler state: enabled={}, temperature={:?}",
            enabled, temperature
        );

        let mut params = serde_json::json!({
            "enabled": enabled
        });

        if let Some(temp) = temperature {
            params["temperature"] = serde_json::json!(temp);
        }

        self.send_request("set_cooler_state", Some(params)).await?;
        Ok(())
    }

    // ========================================================================
    // Image Operations Methods
    // ========================================================================

    /// Get the current guide star image
    ///
    /// Returns the guide star image data including the image pixels as base64-encoded data.
    /// The image is a subframe around the guide star.
    ///
    /// # Arguments
    /// * `size` - Size of the image in pixels (width and height will be 2*size+1)
    pub async fn get_star_image(&self, size: u32) -> Result<StarImage> {
        debug!("Getting star image with size {}", size);

        let params = serde_json::json!({
            "size": size
        });

        let result = self.send_request("get_star_image", Some(params)).await?;
        let image: StarImage = serde_json::from_value(result)?;
        Ok(image)
    }

    /// Save the current camera frame to a file
    ///
    /// Saves the current frame to a FITS file in PHD2's default image directory.
    /// Returns the path to the saved file.
    pub async fn save_image(&self) -> Result<String> {
        debug!("Saving current image");
        let result = self.send_request("save_image", None).await?;
        let filename = result.as_str().ok_or_else(|| {
            Phd2Error::InvalidState("Expected string for saved image filename".to_string())
        })?;
        Ok(filename.to_string())
    }
}
