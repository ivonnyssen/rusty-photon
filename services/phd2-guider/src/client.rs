//! PHD2 client for communicating with PHD2 via JSON RPC

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, Mutex, RwLock};
use tracing::debug;

use crate::config::{Phd2Config, SettleParams};
use crate::error::{Phd2Error, Result};
use crate::events::{AppState, Phd2Event};
use crate::rpc::{RpcRequest, RpcResponse};
use crate::types::{Profile, Rect};

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
}

impl Default for ClientState {
    fn default() -> Self {
        Self {
            connected: false,
            phd2_version: None,
            app_state: None,
        }
    }
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
}

impl Phd2Client {
    /// Create a new PHD2 client with the given configuration
    pub fn new(config: Phd2Config) -> Self {
        let (event_sender, _) = broadcast::channel(100);
        Self {
            config,
            writer: Arc::new(Mutex::new(None)),
            request_id: AtomicU64::new(1),
            pending_requests: Arc::new(Mutex::new(HashMap::new())),
            event_sender,
            state: Arc::new(RwLock::new(ClientState::default())),
            reader_handle: Arc::new(Mutex::new(None)),
        }
    }

    /// Connect to a running PHD2 instance
    pub async fn connect(&self) -> Result<()> {
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

        tokio::spawn(async move {
            let mut buf_reader = BufReader::new(reader);
            let mut line = String::new();

            loop {
                line.clear();
                match buf_reader.read_line(&mut line).await {
                    Ok(0) => {
                        debug!("PHD2 connection closed");
                        let mut state_guard = state.write().await;
                        state_guard.connected = false;
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
                        let mut state_guard = state.write().await;
                        state_guard.connected = false;
                        break;
                    }
                }
            }
        })
    }

    /// Disconnect from PHD2
    pub async fn disconnect(&self) -> Result<()> {
        debug!("Disconnecting from PHD2");

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
}
