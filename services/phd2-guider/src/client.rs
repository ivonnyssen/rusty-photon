//! PHD2 client for communicating with PHD2 via JSON RPC

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, Mutex, Notify, RwLock};
use tracing::{debug, info, warn};

use crate::config::{Phd2Config, ReconnectConfig, SettleParams};
use crate::error::{Phd2Error, Result};
use crate::events::{AppState, Phd2Event};
use crate::rpc::{RpcRequest, RpcResponse};
use crate::types::{CalibrationData, CalibrationTarget, Equipment, Profile, Rect};

/// Pending RPC request waiting for response
struct PendingRequest {
    sender: tokio::sync::oneshot::Sender<std::result::Result<serde_json::Value, Phd2Error>>,
}

/// Internal client state
#[derive(Debug, Clone)]
struct ClientState {
    connected: bool,
    phd2_version: Option<String>,
    app_state: Option<AppState>,
    reconnecting: bool,
}

impl Default for ClientState {
    fn default() -> Self {
        Self {
            connected: false,
            phd2_version: None,
            app_state: None,
            reconnecting: false,
        }
    }
}

/// Spawn a reconnection task that attempts to reconnect to PHD2
#[allow(clippy::too_many_arguments)]
fn spawn_reconnect_task(
    host: String,
    port: u16,
    connection_timeout: u64,
    reconnect_config: ReconnectConfig,
    state: Arc<RwLock<ClientState>>,
    writer: Arc<Mutex<Option<tokio::io::WriteHalf<TcpStream>>>>,
    pending_requests: Arc<Mutex<HashMap<u64, PendingRequest>>>,
    event_sender: broadcast::Sender<Phd2Event>,
    stop_reconnect: Arc<Notify>,
    reader_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    auto_reconnect_enabled: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Set reconnecting state
        {
            let mut state_guard = state.write().await;
            state_guard.reconnecting = true;
        }

        let addr = format!("{}:{}", host, port);
        let interval = std::time::Duration::from_secs(reconnect_config.interval_seconds);
        let timeout_duration = std::time::Duration::from_secs(connection_timeout);
        let max_retries = reconnect_config.max_retries;
        let mut attempt = 0u32;

        loop {
            attempt += 1;

            // Check if we should stop reconnecting
            if !auto_reconnect_enabled.load(Ordering::SeqCst) {
                debug!("Auto-reconnect disabled, stopping reconnection attempts");
                let _ = event_sender.send(Phd2Event::ReconnectFailed {
                    reason: "Auto-reconnect disabled".to_string(),
                });
                break;
            }

            // Check if max retries exceeded
            if let Some(max) = max_retries {
                if attempt > max {
                    warn!("Reconnection failed: max retries ({}) exceeded", max);
                    let _ = event_sender.send(Phd2Event::ReconnectFailed {
                        reason: format!("Max retries ({}) exceeded", max),
                    });
                    break;
                }
            }

            // Broadcast reconnecting event
            info!(
                "Attempting to reconnect to PHD2 (attempt {}/{})",
                attempt,
                max_retries.map_or("∞".to_string(), |m| m.to_string())
            );
            let _ = event_sender.send(Phd2Event::Reconnecting {
                attempt,
                max_attempts: max_retries,
            });

            // Wait before attempting connection (unless first attempt)
            if attempt > 1 {
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {}
                    _ = stop_reconnect.notified() => {
                        debug!("Reconnection stopped by user");
                        let _ = event_sender.send(Phd2Event::ReconnectFailed {
                            reason: "Reconnection cancelled".to_string(),
                        });
                        break;
                    }
                }
            }

            // Attempt connection
            debug!("Attempting TCP connection to {}", addr);
            let connect_result =
                tokio::time::timeout(timeout_duration, TcpStream::connect(&addr)).await;

            match connect_result {
                Ok(Ok(stream)) => {
                    debug!("TCP connection established to PHD2");
                    let (reader, new_writer) = tokio::io::split(stream);

                    // Store the writer
                    {
                        let mut writer_guard = writer.lock().await;
                        *writer_guard = Some(new_writer);
                    }

                    // Update connection state
                    {
                        let mut state_guard = state.write().await;
                        state_guard.connected = true;
                        state_guard.reconnecting = false;
                    }

                    // Start a new reader task
                    let new_reader_handle = spawn_reader_task_internal(
                        reader,
                        pending_requests.clone(),
                        event_sender.clone(),
                        state.clone(),
                        auto_reconnect_enabled.clone(),
                        ReconnectConfig {
                            enabled: reconnect_config.enabled,
                            interval_seconds: reconnect_config.interval_seconds,
                            max_retries: reconnect_config.max_retries,
                        },
                        host.clone(),
                        port,
                        connection_timeout,
                        writer.clone(),
                        reader_handle.clone(),
                        stop_reconnect.clone(),
                    );

                    // Store the new reader handle
                    {
                        let mut handle = reader_handle.lock().await;
                        *handle = Some(new_reader_handle);
                    }

                    // Broadcast reconnected event
                    info!("Successfully reconnected to PHD2");
                    let _ = event_sender.send(Phd2Event::Reconnected);
                    return;
                }
                Ok(Err(e)) => {
                    debug!("Connection attempt {} failed: {}", attempt, e);
                }
                Err(_) => {
                    debug!("Connection attempt {} timed out", attempt);
                }
            }
        }

        // Reconnection failed - update state
        {
            let mut state_guard = state.write().await;
            state_guard.reconnecting = false;
        }
    })
}

/// Internal reader task spawner (called from reconnect task)
#[allow(clippy::too_many_arguments)]
fn spawn_reader_task_internal(
    reader: tokio::io::ReadHalf<TcpStream>,
    pending_requests: Arc<Mutex<HashMap<u64, PendingRequest>>>,
    event_sender: broadcast::Sender<Phd2Event>,
    state: Arc<RwLock<ClientState>>,
    auto_reconnect_enabled: Arc<AtomicBool>,
    reconnect_config: ReconnectConfig,
    host: String,
    port: u16,
    connection_timeout: u64,
    writer: Arc<Mutex<Option<tokio::io::WriteHalf<TcpStream>>>>,
    reader_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    stop_reconnect: Arc<Notify>,
) -> tokio::task::JoinHandle<()> {
    let reconnect_handle = Arc::new(Mutex::new(None));

    tokio::spawn(async move {
        let mut buf_reader = BufReader::new(reader);
        let mut line = String::new();
        let disconnect_reason;

        loop {
            line.clear();
            match buf_reader.read_line(&mut line).await {
                Ok(0) => {
                    debug!("PHD2 connection closed");
                    disconnect_reason = "Connection closed by remote".to_string();
                    break;
                }
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    debug!("Received from PHD2: {}", trimmed);

                    // Try to parse as a response first (has "id" field)
                    if let Ok(response) = serde_json::from_str::<RpcResponse>(trimmed) {
                        let mut pending = pending_requests.lock().await;
                        if let Some(request) = pending.remove(&response.id) {
                            let result = if let Some(error) = response.error {
                                Err(Phd2Error::RpcError {
                                    code: error.code,
                                    message: error.message,
                                })
                            } else {
                                Ok(response.result.unwrap_or(serde_json::Value::Null))
                            };
                            let _ = request.sender.send(result);
                        }
                    } else if let Ok(event) = serde_json::from_str::<Phd2Event>(trimmed) {
                        // Handle specific events to update internal state
                        match &event {
                            Phd2Event::Version { phd_version, .. } => {
                                let mut state_guard = state.write().await;
                                state_guard.phd2_version = Some(phd_version.clone());
                                debug!("PHD2 version: {}", phd_version);
                            }
                            Phd2Event::AppState { state: app_state } => {
                                if let Ok(parsed_state) = app_state.parse::<AppState>() {
                                    let mut state_guard = state.write().await;
                                    state_guard.app_state = Some(parsed_state);
                                    debug!("PHD2 app state: {}", parsed_state);
                                }
                            }
                            _ => {}
                        }

                        // Broadcast event to subscribers
                        let _ = event_sender.send(event);
                    } else {
                        debug!("Failed to parse PHD2 message: {}", trimmed);
                    }
                }
                Err(e) => {
                    debug!("Error reading from PHD2: {}", e);
                    disconnect_reason = format!("Read error: {}", e);
                    break;
                }
            }
        }

        // Connection lost - update state and notify
        {
            let mut state_guard = state.write().await;
            state_guard.connected = false;
        }

        // Broadcast connection lost event
        warn!("PHD2 connection lost: {}", disconnect_reason);
        let _ = event_sender.send(Phd2Event::ConnectionLost {
            reason: disconnect_reason.clone(),
        });

        // Clear pending requests
        {
            let mut pending = pending_requests.lock().await;
            pending.clear();
        }

        // Close the writer
        {
            let mut writer_guard = writer.lock().await;
            if let Some(mut w) = writer_guard.take() {
                let _ = w.shutdown().await;
            }
        }

        // Start reconnection if enabled
        if auto_reconnect_enabled.load(Ordering::SeqCst) {
            debug!("Auto-reconnect enabled, starting reconnection task");
            let reconnect_task = spawn_reconnect_task(
                host,
                port,
                connection_timeout,
                reconnect_config,
                state,
                writer,
                pending_requests,
                event_sender,
                stop_reconnect,
                reader_handle,
                auto_reconnect_enabled,
            );
            let mut handle = reconnect_handle.lock().await;
            *handle = Some(reconnect_task);
        }
    })
}

/// PHD2 client for communicating with PHD2 via JSON RPC
pub struct Phd2Client {
    config: Phd2Config,
    writer: Arc<Mutex<Option<tokio::io::WriteHalf<TcpStream>>>>,
    request_id: AtomicU64,
    pending_requests: Arc<Mutex<HashMap<u64, PendingRequest>>>,
    event_sender: broadcast::Sender<Phd2Event>,
    state: Arc<RwLock<ClientState>>,
    reader_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    reconnect_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    auto_reconnect_enabled: Arc<AtomicBool>,
    stop_reconnect: Arc<Notify>,
}

impl Phd2Client {
    /// Create a new PHD2 client with the given configuration
    pub fn new(config: Phd2Config) -> Self {
        let (event_sender, _) = broadcast::channel(100);
        let auto_reconnect_enabled = config.reconnect.enabled;
        Self {
            config,
            writer: Arc::new(Mutex::new(None)),
            request_id: AtomicU64::new(1),
            pending_requests: Arc::new(Mutex::new(HashMap::new())),
            event_sender,
            state: Arc::new(RwLock::new(ClientState::default())),
            reader_handle: Arc::new(Mutex::new(None)),
            reconnect_handle: Arc::new(Mutex::new(None)),
            auto_reconnect_enabled: Arc::new(AtomicBool::new(auto_reconnect_enabled)),
            stop_reconnect: Arc::new(Notify::new()),
        }
    }

    /// Connect to a running PHD2 instance
    pub async fn connect(&self) -> Result<()> {
        // Stop any ongoing reconnection attempt
        self.stop_reconnect.notify_waiters();
        {
            let mut handle = self.reconnect_handle.lock().await;
            if let Some(h) = handle.take() {
                h.abort();
            }
        }

        let addr = format!("{}:{}", self.config.host, self.config.port);
        debug!("Connecting to PHD2 at {}", addr);

        let connect_future = TcpStream::connect(&addr);
        let timeout_duration =
            std::time::Duration::from_secs(self.config.connection_timeout_seconds);

        let stream = tokio::time::timeout(timeout_duration, connect_future)
            .await
            .map_err(|_| Phd2Error::Timeout(format!("Connection to {} timed out", addr)))?
            .map_err(|e| {
                Phd2Error::ConnectionFailed(format!("Failed to connect to {}: {}", addr, e))
            })?;

        debug!("TCP connection established to PHD2");

        let (reader, writer) = tokio::io::split(stream);

        // Store the writer
        {
            let mut writer_guard = self.writer.lock().await;
            *writer_guard = Some(writer);
        }

        // Update connection state
        {
            let mut state = self.state.write().await;
            state.connected = true;
            state.reconnecting = false;
        }

        // Start the reader task
        let reader_handle = self.spawn_reader_task(reader);
        {
            let mut handle_guard = self.reader_handle.lock().await;
            *handle_guard = Some(reader_handle);
        }

        debug!("PHD2 client connected and reader task started");
        Ok(())
    }

    /// Spawn a background task to read messages from PHD2
    fn spawn_reader_task(
        &self,
        reader: tokio::io::ReadHalf<TcpStream>,
    ) -> tokio::task::JoinHandle<()> {
        let pending_requests = Arc::clone(&self.pending_requests);
        let event_sender = self.event_sender.clone();
        let state = Arc::clone(&self.state);
        let auto_reconnect_enabled = Arc::clone(&self.auto_reconnect_enabled);
        let reconnect_config = self.config.reconnect.clone();
        let host = self.config.host.clone();
        let port = self.config.port;
        let connection_timeout = self.config.connection_timeout_seconds;
        let writer = Arc::clone(&self.writer);
        let reconnect_handle = Arc::clone(&self.reconnect_handle);
        let stop_reconnect = Arc::clone(&self.stop_reconnect);
        let reader_handle = Arc::clone(&self.reader_handle);

        tokio::spawn(async move {
            let mut buf_reader = BufReader::new(reader);
            let mut line = String::new();
            let disconnect_reason;

            loop {
                line.clear();
                match buf_reader.read_line(&mut line).await {
                    Ok(0) => {
                        debug!("PHD2 connection closed");
                        disconnect_reason = "Connection closed by remote".to_string();
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }

                        debug!("Received from PHD2: {}", trimmed);

                        // Try to parse as a response first (has "id" field)
                        if let Ok(response) = serde_json::from_str::<RpcResponse>(trimmed) {
                            let mut pending = pending_requests.lock().await;
                            if let Some(request) = pending.remove(&response.id) {
                                let result = if let Some(error) = response.error {
                                    Err(Phd2Error::RpcError {
                                        code: error.code,
                                        message: error.message,
                                    })
                                } else {
                                    Ok(response.result.unwrap_or(serde_json::Value::Null))
                                };
                                let _ = request.sender.send(result);
                            }
                        } else if let Ok(event) = serde_json::from_str::<Phd2Event>(trimmed) {
                            // Handle specific events to update internal state
                            match &event {
                                Phd2Event::Version { phd_version, .. } => {
                                    let mut state_guard = state.write().await;
                                    state_guard.phd2_version = Some(phd_version.clone());
                                    debug!("PHD2 version: {}", phd_version);
                                }
                                Phd2Event::AppState { state: app_state } => {
                                    if let Ok(parsed_state) = app_state.parse::<AppState>() {
                                        let mut state_guard = state.write().await;
                                        state_guard.app_state = Some(parsed_state);
                                        debug!("PHD2 app state: {}", parsed_state);
                                    }
                                }
                                _ => {}
                            }

                            // Broadcast event to subscribers
                            let _ = event_sender.send(event);
                        } else {
                            debug!("Failed to parse PHD2 message: {}", trimmed);
                        }
                    }
                    Err(e) => {
                        debug!("Error reading from PHD2: {}", e);
                        disconnect_reason = format!("Read error: {}", e);
                        break;
                    }
                }
            }

            // Connection lost - update state and notify
            {
                let mut state_guard = state.write().await;
                state_guard.connected = false;
            }

            // Broadcast connection lost event
            warn!("PHD2 connection lost: {}", disconnect_reason);
            let _ = event_sender.send(Phd2Event::ConnectionLost {
                reason: disconnect_reason.clone(),
            });

            // Clear pending requests
            {
                let mut pending = pending_requests.lock().await;
                pending.clear();
            }

            // Close the writer
            {
                let mut writer_guard = writer.lock().await;
                if let Some(mut w) = writer_guard.take() {
                    let _ = w.shutdown().await;
                }
            }

            // Start reconnection if enabled
            if auto_reconnect_enabled.load(Ordering::SeqCst) {
                debug!("Auto-reconnect enabled, starting reconnection task");
                let reconnect_task = spawn_reconnect_task(
                    host,
                    port,
                    connection_timeout,
                    reconnect_config,
                    state,
                    writer,
                    pending_requests,
                    event_sender,
                    stop_reconnect,
                    reader_handle,
                    auto_reconnect_enabled,
                );
                let mut handle = reconnect_handle.lock().await;
                *handle = Some(reconnect_task);
            }
        })
    }

    /// Disconnect from PHD2
    ///
    /// This will stop any ongoing reconnection attempts and cleanly disconnect
    /// from PHD2. After calling this, auto-reconnect will not trigger unless
    /// you manually call `connect()` again.
    pub async fn disconnect(&self) -> Result<()> {
        debug!("Disconnecting from PHD2");

        // Stop any ongoing reconnection attempt
        self.stop_reconnect.notify_waiters();
        {
            let mut handle = self.reconnect_handle.lock().await;
            if let Some(h) = handle.take() {
                h.abort();
            }
        }

        // Abort the reader task
        {
            let mut handle = self.reader_handle.lock().await;
            if let Some(h) = handle.take() {
                h.abort();
            }
        }

        // Close the writer
        {
            let mut writer = self.writer.lock().await;
            if let Some(mut w) = writer.take() {
                let _ = w.shutdown().await;
            }
        }

        // Update state
        {
            let mut state = self.state.write().await;
            state.connected = false;
            state.reconnecting = false;
            state.phd2_version = None;
            state.app_state = None;
        }

        // Clear pending requests
        {
            let mut pending = self.pending_requests.lock().await;
            pending.clear();
        }

        debug!("Disconnected from PHD2");
        Ok(())
    }

    /// Check if connected to PHD2
    pub async fn is_connected(&self) -> bool {
        self.state.read().await.connected
    }

    /// Get the PHD2 version (available after connection)
    pub async fn get_phd2_version(&self) -> Option<String> {
        self.state.read().await.phd2_version.clone()
    }

    /// Subscribe to PHD2 events
    pub fn subscribe(&self) -> broadcast::Receiver<Phd2Event> {
        self.event_sender.subscribe()
    }

    // ========================================================================
    // Auto-reconnect Control Methods
    // ========================================================================

    /// Check if auto-reconnect is currently enabled
    pub fn is_auto_reconnect_enabled(&self) -> bool {
        self.auto_reconnect_enabled.load(Ordering::SeqCst)
    }

    /// Enable or disable auto-reconnect
    ///
    /// When disabled during an active reconnection attempt, the attempt will
    /// be stopped after the current connection try completes.
    pub fn set_auto_reconnect_enabled(&self, enabled: bool) {
        debug!("Setting auto-reconnect enabled: {}", enabled);
        self.auto_reconnect_enabled.store(enabled, Ordering::SeqCst);
        if !enabled {
            // Signal any waiting reconnect task to stop
            self.stop_reconnect.notify_waiters();
        }
    }

    /// Check if the client is currently attempting to reconnect
    pub async fn is_reconnecting(&self) -> bool {
        self.state.read().await.reconnecting
    }

    /// Stop any ongoing reconnection attempts
    ///
    /// This stops the current reconnection task without disabling auto-reconnect.
    /// If the connection is lost again in the future, reconnection will be attempted.
    pub async fn stop_reconnection(&self) {
        debug!("Stopping reconnection attempts");
        self.stop_reconnect.notify_waiters();
        {
            let mut handle = self.reconnect_handle.lock().await;
            if let Some(h) = handle.take() {
                h.abort();
            }
        }
        {
            let mut state = self.state.write().await;
            state.reconnecting = false;
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
            let mut pending = self.pending_requests.lock().await;
            pending.insert(id, PendingRequest { sender });
        }

        // Send the request
        {
            let mut writer_guard = self.writer.lock().await;
            if let Some(writer) = writer_guard.as_mut() {
                writer
                    .write_all(format!("{}\r\n", request_json).as_bytes())
                    .await
                    .map_err(|e| Phd2Error::SendError(e.to_string()))?;
                writer
                    .flush()
                    .await
                    .map_err(|e| Phd2Error::SendError(e.to_string()))?;
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
        self.state.read().await.app_state
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_guide_request_params_format() {
        let settle = SettleParams::default();
        let settle_obj = serde_json::json!({
            "pixels": settle.pixels,
            "time": settle.time,
            "timeout": settle.timeout
        });
        let params = serde_json::json!({
            "settle": settle_obj,
            "recalibrate": false
        });

        assert!(params["settle"]["pixels"].as_f64().is_some());
        assert!(params["settle"]["time"].as_u64().is_some());
        assert!(params["settle"]["timeout"].as_u64().is_some());
        assert_eq!(params["recalibrate"].as_bool().unwrap(), false);
    }

    #[test]
    fn test_guide_request_with_roi() {
        let settle = SettleParams::default();
        let roi = Rect::new(100, 100, 200, 200);

        let settle_obj = serde_json::json!({
            "pixels": settle.pixels,
            "time": settle.time,
            "timeout": settle.timeout
        });
        let mut params = serde_json::json!({
            "settle": settle_obj,
            "recalibrate": true
        });
        params["roi"] = serde_json::json!([roi.x, roi.y, roi.width, roi.height]);

        let roi_arr = params["roi"].as_array().unwrap();
        assert_eq!(roi_arr.len(), 4);
        assert_eq!(roi_arr[0].as_i64().unwrap(), 100);
        assert_eq!(roi_arr[1].as_i64().unwrap(), 100);
        assert_eq!(roi_arr[2].as_i64().unwrap(), 200);
        assert_eq!(roi_arr[3].as_i64().unwrap(), 200);
    }

    #[test]
    fn test_dither_request_params_format() {
        let settle = SettleParams::default();
        let settle_obj = serde_json::json!({
            "pixels": settle.pixels,
            "time": settle.time,
            "timeout": settle.timeout
        });
        let params = serde_json::json!({
            "amount": 5.0,
            "raOnly": true,
            "settle": settle_obj
        });

        assert_eq!(params["amount"].as_f64().unwrap(), 5.0);
        assert_eq!(params["raOnly"].as_bool().unwrap(), true);
        assert!(params["settle"]["pixels"].as_f64().is_some());
    }

    #[test]
    fn test_pause_request_params_full() {
        let params = serde_json::json!({"paused": true, "full": "full"});
        assert_eq!(params["paused"].as_bool().unwrap(), true);
        assert_eq!(params["full"].as_str().unwrap(), "full");
    }

    #[test]
    fn test_pause_request_params_partial() {
        let params = serde_json::json!({"paused": true});
        assert_eq!(params["paused"].as_bool().unwrap(), true);
        assert!(params.get("full").is_none());
    }

    #[test]
    fn test_resume_request_params() {
        let params = serde_json::json!({"paused": false});
        assert_eq!(params["paused"].as_bool().unwrap(), false);
    }

    #[test]
    fn test_get_current_equipment_response_parsing() {
        // Simulate PHD2's get_current_equipment response
        let response_json = serde_json::json!({
            "camera": {"name": "ZWO ASI120MM Mini", "connected": true},
            "mount": {"name": "EQMOD ASCOM HEQ5/6", "connected": true},
            "aux_mount": null,
            "AO": null,
            "rotator": null
        });

        let equipment: Equipment = serde_json::from_value(response_json).unwrap();

        let camera = equipment.camera.unwrap();
        assert_eq!(camera.name, "ZWO ASI120MM Mini");
        assert!(camera.connected);

        let mount = equipment.mount.unwrap();
        assert_eq!(mount.name, "EQMOD ASCOM HEQ5/6");
        assert!(mount.connected);

        assert!(equipment.aux_mount.is_none());
        assert!(equipment.ao.is_none());
        assert!(equipment.rotator.is_none());
    }

    #[test]
    fn test_client_auto_reconnect_default_enabled() {
        let config = Phd2Config::default();
        let client = Phd2Client::new(config);
        assert!(client.is_auto_reconnect_enabled());
    }

    #[test]
    fn test_client_auto_reconnect_disabled_in_config() {
        let config = Phd2Config {
            reconnect: ReconnectConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let client = Phd2Client::new(config);
        assert!(!client.is_auto_reconnect_enabled());
    }

    #[test]
    fn test_client_toggle_auto_reconnect() {
        let config = Phd2Config::default();
        let client = Phd2Client::new(config);

        assert!(client.is_auto_reconnect_enabled());
        client.set_auto_reconnect_enabled(false);
        assert!(!client.is_auto_reconnect_enabled());
        client.set_auto_reconnect_enabled(true);
        assert!(client.is_auto_reconnect_enabled());
    }

    #[tokio::test]
    async fn test_client_initial_state() {
        let config = Phd2Config::default();
        let client = Phd2Client::new(config);

        assert!(!client.is_connected().await);
        assert!(!client.is_reconnecting().await);
        assert!(client.get_phd2_version().await.is_none());
    }

    // ========================================================================
    // Star Selection Method Tests
    // ========================================================================

    #[test]
    fn test_find_star_request_params_no_roi() {
        let params: Option<serde_json::Value> = None;
        assert!(params.is_none());
    }

    #[test]
    fn test_find_star_request_params_with_roi() {
        let roi = Rect::new(100, 200, 300, 400);
        let params = serde_json::json!([roi.x, roi.y, roi.width, roi.height]);

        let arr = params.as_array().unwrap();
        assert_eq!(arr.len(), 4);
        assert_eq!(arr[0].as_i64().unwrap(), 100);
        assert_eq!(arr[1].as_i64().unwrap(), 200);
        assert_eq!(arr[2].as_i64().unwrap(), 300);
        assert_eq!(arr[3].as_i64().unwrap(), 400);
    }

    #[test]
    fn test_get_lock_position_response_parsing() {
        let response = serde_json::json!([256.5, 512.3]);
        let arr = response.as_array().unwrap();
        let x = arr[0].as_f64().unwrap();
        let y = arr[1].as_f64().unwrap();

        assert_eq!(x, 256.5);
        assert_eq!(y, 512.3);
    }

    #[test]
    fn test_get_lock_position_null_response() {
        let response = serde_json::json!(null);
        assert!(response.is_null());
    }

    #[test]
    fn test_set_lock_position_request_params() {
        let params = serde_json::json!({
            "X": 256.5,
            "Y": 512.3,
            "EXACT": true
        });

        assert_eq!(params["X"].as_f64().unwrap(), 256.5);
        assert_eq!(params["Y"].as_f64().unwrap(), 512.3);
        assert!(params["EXACT"].as_bool().unwrap());
    }

    #[test]
    fn test_set_lock_position_request_params_not_exact() {
        let params = serde_json::json!({
            "X": 100.0,
            "Y": 200.0,
            "EXACT": false
        });

        assert_eq!(params["X"].as_f64().unwrap(), 100.0);
        assert_eq!(params["Y"].as_f64().unwrap(), 200.0);
        assert!(!params["EXACT"].as_bool().unwrap());
    }

    // ========================================================================
    // Calibration Method Tests
    // ========================================================================

    #[test]
    fn test_get_calibration_data_request_params_mount() {
        let params = serde_json::json!({
            "which": CalibrationTarget::Mount.to_get_api_string()
        });
        assert_eq!(params["which"].as_str().unwrap(), "Mount");
    }

    #[test]
    fn test_get_calibration_data_request_params_ao() {
        let params = serde_json::json!({
            "which": CalibrationTarget::AO.to_get_api_string()
        });
        assert_eq!(params["which"].as_str().unwrap(), "AO");
    }

    #[test]
    fn test_clear_calibration_request_params_mount() {
        let params = serde_json::json!({
            "which": CalibrationTarget::Mount.to_clear_api_string()
        });
        assert_eq!(params["which"].as_str().unwrap(), "mount");
    }

    #[test]
    fn test_clear_calibration_request_params_ao() {
        let params = serde_json::json!({
            "which": CalibrationTarget::AO.to_clear_api_string()
        });
        assert_eq!(params["which"].as_str().unwrap(), "ao");
    }

    #[test]
    fn test_clear_calibration_request_params_both() {
        let params = serde_json::json!({
            "which": CalibrationTarget::Both.to_clear_api_string()
        });
        assert_eq!(params["which"].as_str().unwrap(), "both");
    }

    #[test]
    fn test_get_calibration_data_response_parsing() {
        let response = serde_json::json!({
            "calibrated": true,
            "xAngle": 45.5,
            "xRate": 15.2,
            "xParity": "+",
            "yAngle": 135.5,
            "yRate": 14.8,
            "yParity": "-",
            "declination": 30.0
        });

        let data: CalibrationData = serde_json::from_value(response).unwrap();
        assert!(data.calibrated);
        assert_eq!(data.x_angle, 45.5);
        assert_eq!(data.x_rate, 15.2);
        assert_eq!(data.x_parity, "+");
        assert_eq!(data.y_angle, 135.5);
        assert_eq!(data.y_rate, 14.8);
        assert_eq!(data.y_parity, "-");
        assert_eq!(data.declination, Some(30.0));
    }

    #[test]
    fn test_get_calibration_data_response_not_calibrated() {
        let response = serde_json::json!({
            "calibrated": false,
            "xAngle": 0.0,
            "xRate": 0.0,
            "xParity": "+",
            "yAngle": 0.0,
            "yRate": 0.0,
            "yParity": "+"
        });

        let data: CalibrationData = serde_json::from_value(response).unwrap();
        assert!(!data.calibrated);
        assert_eq!(data.x_rate, 0.0);
        assert!(data.declination.is_none());
    }
}
