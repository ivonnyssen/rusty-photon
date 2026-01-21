//! PHD2 Guider Client Library
//!
//! This crate provides a Rust client for interacting with Open PHD Guiding 2 (PHD2)
//! via its JSON RPC interface on port 4400.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, Mutex, RwLock};
use tracing::debug;

// ============================================================================
// Error Types
// ============================================================================

/// Errors that can occur when interacting with PHD2
#[derive(Debug, thiserror::Error)]
pub enum Phd2Error {
    #[error("Not connected to PHD2")]
    NotConnected,

    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("PHD2 not running")]
    Phd2NotRunning,

    #[error("Equipment not connected")]
    EquipmentNotConnected,

    #[error("Not calibrated")]
    NotCalibrated,

    #[error("Invalid state for operation: {0}")]
    InvalidState(String),

    #[error("RPC error: {code} - {message}")]
    RpcError { code: i32, message: String },

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Failed to send message: {0}")]
    SendError(String),

    #[error("Failed to receive response")]
    ReceiveError,

    #[error("Failed to start PHD2 process: {0}")]
    ProcessStartFailed(String),

    #[error("PHD2 executable not found: {0}")]
    ExecutableNotFound(String),

    #[error("Process already running")]
    ProcessAlreadyRunning,
}

pub type Result<T> = std::result::Result<T, Phd2Error>;

// ============================================================================
// JSON RPC Types
// ============================================================================

/// JSON RPC 2.0 request
#[derive(Debug, Clone, Serialize)]
pub struct RpcRequest {
    pub jsonrpc: &'static str,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    pub id: u64,
}

impl RpcRequest {
    pub fn new(method: impl Into<String>, params: Option<serde_json::Value>, id: u64) -> Self {
        Self {
            jsonrpc: "2.0",
            method: method.into(),
            params,
            id,
        }
    }
}

/// JSON RPC 2.0 response
#[derive(Debug, Clone, Deserialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<RpcErrorObject>,
    pub id: u64,
}

/// JSON RPC 2.0 error object
#[derive(Debug, Clone, Deserialize)]
pub struct RpcErrorObject {
    pub code: i32,
    pub message: String,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

/// Incoming message from PHD2 - either an event or a response
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Phd2Message {
    Response(RpcResponse),
    Event(Phd2Event),
}

// ============================================================================
// PHD2 Event Types
// ============================================================================

/// PHD2 application state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppState {
    Stopped,
    Selected,
    Calibrating,
    Guiding,
    LostLock,
    Paused,
    Looping,
}

impl std::fmt::Display for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppState::Stopped => write!(f, "Stopped"),
            AppState::Selected => write!(f, "Selected"),
            AppState::Calibrating => write!(f, "Calibrating"),
            AppState::Guiding => write!(f, "Guiding"),
            AppState::LostLock => write!(f, "LostLock"),
            AppState::Paused => write!(f, "Paused"),
            AppState::Looping => write!(f, "Looping"),
        }
    }
}

impl std::str::FromStr for AppState {
    type Err = Phd2Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "Stopped" => Ok(AppState::Stopped),
            "Selected" => Ok(AppState::Selected),
            "Calibrating" => Ok(AppState::Calibrating),
            "Guiding" => Ok(AppState::Guiding),
            "LostLock" => Ok(AppState::LostLock),
            "Paused" => Ok(AppState::Paused),
            "Looping" => Ok(AppState::Looping),
            _ => Err(Phd2Error::InvalidState(format!("Unknown state: {}", s))),
        }
    }
}

/// Guide step statistics from PHD2
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GuideStepStats {
    pub frame: u64,
    pub time: f64,
    pub mount: String,
    #[serde(rename = "dx")]
    pub dx: f64,
    #[serde(rename = "dy")]
    pub dy: f64,
    #[serde(rename = "RADistanceRaw")]
    pub ra_distance_raw: Option<f64>,
    #[serde(rename = "DECDistanceRaw")]
    pub dec_distance_raw: Option<f64>,
    #[serde(rename = "RADistanceGuide")]
    pub ra_distance_guide: Option<f64>,
    #[serde(rename = "DECDistanceGuide")]
    pub dec_distance_guide: Option<f64>,
    #[serde(rename = "RADuration")]
    pub ra_duration: Option<i32>,
    #[serde(rename = "RADirection")]
    pub ra_direction: Option<String>,
    #[serde(rename = "DECDuration")]
    pub dec_duration: Option<i32>,
    #[serde(rename = "DECDirection")]
    pub dec_direction: Option<String>,
    #[serde(rename = "StarMass")]
    pub star_mass: Option<f64>,
    #[serde(rename = "SNR")]
    pub snr: Option<f64>,
    #[serde(rename = "HFD")]
    pub hfd: Option<f64>,
    #[serde(rename = "AvgDist")]
    pub avg_dist: Option<f64>,
    #[serde(rename = "RALimited")]
    pub ra_limited: Option<bool>,
    #[serde(rename = "DecLimited")]
    pub dec_limited: Option<bool>,
    #[serde(rename = "ErrorCode")]
    pub error_code: Option<i32>,
}

/// PHD2 event notification
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "Event")]
pub enum Phd2Event {
    /// Sent on connection, contains PHD2 version info
    Version {
        #[serde(rename = "PHDVersion")]
        phd_version: String,
        #[serde(rename = "PHDSubver")]
        phd_subver: Option<String>,
        #[serde(rename = "MsgVersion")]
        msg_version: Option<u32>,
        #[serde(rename = "OverlapSupport")]
        overlap_support: Option<bool>,
    },

    /// Application state changed
    AppState {
        #[serde(rename = "State")]
        state: String,
    },

    /// Guide step with statistics
    GuideStep(GuideStepStats),

    /// Dither operation completed
    GuidingDithered {
        #[serde(rename = "dx")]
        dx: f64,
        #[serde(rename = "dy")]
        dy: f64,
    },

    /// Settling completed
    SettleDone {
        #[serde(rename = "Status")]
        status: i32,
        #[serde(rename = "Error")]
        error: Option<String>,
    },

    /// Star was selected
    StarSelected {
        #[serde(rename = "X")]
        x: f64,
        #[serde(rename = "Y")]
        y: f64,
    },

    /// Star was lost
    StarLost {
        #[serde(rename = "Frame")]
        frame: u64,
        #[serde(rename = "Time")]
        time: f64,
        #[serde(rename = "StarMass")]
        star_mass: f64,
        #[serde(rename = "SNR")]
        snr: f64,
        #[serde(rename = "AvgDist")]
        avg_dist: Option<f64>,
        #[serde(rename = "ErrorCode")]
        error_code: Option<i32>,
        #[serde(rename = "Status")]
        status: String,
    },

    /// Lock position was set
    LockPositionSet {
        #[serde(rename = "X")]
        x: f64,
        #[serde(rename = "Y")]
        y: f64,
    },

    /// Lock shift limit reached
    LockPositionShiftLimitReached,

    /// Calibration in progress
    Calibrating {
        #[serde(rename = "Mount")]
        mount: String,
        #[serde(rename = "dir")]
        dir: String,
        #[serde(rename = "dist")]
        dist: f64,
        #[serde(rename = "dx")]
        dx: f64,
        #[serde(rename = "dy")]
        dy: f64,
        #[serde(rename = "pos")]
        pos: Vec<f64>,
        #[serde(rename = "step")]
        step: u32,
        #[serde(rename = "State")]
        state: String,
    },

    /// Calibration finished
    CalibrationComplete {
        #[serde(rename = "Mount")]
        mount: String,
    },

    /// Calibration failed
    CalibrationFailed {
        #[serde(rename = "Reason")]
        reason: String,
    },

    /// Calibration was flipped
    CalibrationDataFlipped {
        #[serde(rename = "Mount")]
        mount: String,
    },

    /// Looping exposures started
    LoopingExposures {
        #[serde(rename = "Frame")]
        frame: u64,
    },

    /// Looping exposures stopped
    LoopingExposuresStopped,

    /// Guiding was paused
    Paused,

    /// Guiding was resumed
    Resumed,

    /// Guide parameter changed
    GuideParamChange {
        #[serde(rename = "Name")]
        name: String,
        #[serde(rename = "Value")]
        value: serde_json::Value,
    },

    /// Configuration changed
    ConfigurationChange,

    /// Alert message
    Alert {
        #[serde(rename = "Msg")]
        msg: String,
        #[serde(rename = "Type")]
        alert_type: String,
    },

    /// Start guiding event
    StartGuiding,

    /// Settling in progress
    Settling {
        #[serde(rename = "Distance")]
        distance: f64,
        #[serde(rename = "Time")]
        time: f64,
        #[serde(rename = "SettleTime")]
        settle_time: f64,
        #[serde(rename = "StarLocked")]
        star_locked: bool,
    },

    /// Guiding stopped
    GuidingStopped,
}

// ============================================================================
// Configuration
// ============================================================================

/// PHD2 service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub phd2: Phd2Config,
    #[serde(default)]
    pub settling: SettleParams,
}

/// PHD2 connection settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Phd2Config {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub executable_path: Option<PathBuf>,
    #[serde(default = "default_connection_timeout")]
    pub connection_timeout_seconds: u64,
    #[serde(default = "default_command_timeout")]
    pub command_timeout_seconds: u64,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub auto_connect_equipment: bool,
}

impl Default for Phd2Config {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            executable_path: None,
            connection_timeout_seconds: default_connection_timeout(),
            command_timeout_seconds: default_command_timeout(),
            auto_start: false,
            auto_connect_equipment: false,
        }
    }
}

fn default_host() -> String {
    "localhost".to_string()
}

fn default_port() -> u16 {
    4400
}

fn default_connection_timeout() -> u64 {
    10
}

fn default_command_timeout() -> u64 {
    30
}

/// Settling parameters for guiding operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettleParams {
    #[serde(default = "default_settle_pixels")]
    pub pixels: f64,
    #[serde(default = "default_settle_time")]
    pub time: u32,
    #[serde(default = "default_settle_timeout")]
    pub timeout: u32,
}

impl Default for SettleParams {
    fn default() -> Self {
        Self {
            pixels: default_settle_pixels(),
            time: default_settle_time(),
            timeout: default_settle_timeout(),
        }
    }
}

fn default_settle_pixels() -> f64 {
    0.5
}

fn default_settle_time() -> u32 {
    10
}

fn default_settle_timeout() -> u32 {
    60
}

// ============================================================================
// PHD2 Client
// ============================================================================

/// Pending RPC request waiting for response
struct PendingRequest {
    sender: tokio::sync::oneshot::Sender<std::result::Result<serde_json::Value, Phd2Error>>,
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

    /// Shutdown PHD2 application
    pub async fn shutdown_phd2(&self) -> Result<()> {
        self.send_request("shutdown", None).await?;
        Ok(())
    }
}

/// PHD2 equipment profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: i32,
    pub name: String,
}

// ============================================================================
// PHD2 Process Management
// ============================================================================

/// Get the default PHD2 executable path for the current platform
///
/// This function checks for PHD2 in the following order:
/// 1. Local build in the external/phd2/tmp directory (relative to workspace root)
/// 2. System-installed PHD2 in standard locations
/// 3. PHD2 in PATH (Linux only)
pub fn get_default_phd2_path() -> Option<PathBuf> {
    // First, try to find the local build relative to the cargo manifest directory
    // The workspace structure is: workspace_root/services/phd2-guider/
    // The local build is at: workspace_root/external/phd2/tmp/phd2.bin
    if let Some(local_path) = get_local_phd2_build_path() {
        if local_path.exists() {
            debug!("Found local PHD2 build at: {}", local_path.display());
            return Some(local_path);
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Check common Linux locations
        let paths = ["/usr/bin/phd2", "/usr/local/bin/phd2"];
        for path in paths {
            let p = PathBuf::from(path);
            if p.exists() {
                return Some(p);
            }
        }
        // Try to find in PATH
        if let Ok(output) = std::process::Command::new("which").arg("phd2").output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return Some(PathBuf::from(path));
                }
            }
        }
        None
    }

    #[cfg(target_os = "macos")]
    {
        let path = PathBuf::from("/Applications/PHD2.app/Contents/MacOS/PHD2");
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }

    #[cfg(target_os = "windows")]
    {
        let paths = [
            r"C:\Program Files (x86)\PHDGuiding2\phd2.exe",
            r"C:\Program Files\PHDGuiding2\phd2.exe",
        ];
        for path in paths {
            let p = PathBuf::from(path);
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

/// Get the path to the local PHD2 build in the external directory
///
/// Returns the path to external/phd2/tmp/phd2.bin relative to the workspace root.
/// This works by looking for the Cargo.toml workspace file and navigating from there.
fn get_local_phd2_build_path() -> Option<PathBuf> {
    // Try using CARGO_MANIFEST_DIR if available (set during cargo build/test)
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        // manifest_dir is services/phd2-guider, go up two levels to workspace root
        let workspace_root = PathBuf::from(manifest_dir)
            .parent()? // services/
            .parent()? // workspace root
            .to_path_buf();
        let local_build = workspace_root.join("external/phd2/tmp/phd2.bin");
        if local_build.exists() {
            return Some(local_build);
        }
    }

    // Try finding workspace root from current directory
    if let Ok(cwd) = std::env::current_dir() {
        let mut dir = cwd.as_path();
        // Walk up the directory tree looking for Cargo.toml with [workspace]
        while let Some(parent) = dir.parent() {
            let cargo_toml = dir.join("Cargo.toml");
            if cargo_toml.exists() {
                if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                    if content.contains("[workspace]") {
                        let local_build = dir.join("external/phd2/tmp/phd2.bin");
                        if local_build.exists() {
                            return Some(local_build);
                        }
                    }
                }
            }
            dir = parent;
        }
    }

    None
}

/// PHD2 process manager for starting and stopping PHD2
pub struct Phd2ProcessManager {
    config: Phd2Config,
    process: Arc<Mutex<Option<Child>>>,
}

impl Phd2ProcessManager {
    /// Create a new process manager with the given configuration
    pub fn new(config: Phd2Config) -> Self {
        Self {
            config,
            process: Arc::new(Mutex::new(None)),
        }
    }

    /// Check if PHD2 is already running (by attempting to connect)
    pub async fn is_phd2_running(&self) -> bool {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        TcpStream::connect(&addr).await.is_ok()
    }

    /// Get the PHD2 executable path (from config or default)
    fn get_executable_path(&self) -> Result<PathBuf> {
        if let Some(ref path) = self.config.executable_path {
            if path.exists() {
                return Ok(path.clone());
            }
            return Err(Phd2Error::ExecutableNotFound(path.display().to_string()));
        }

        get_default_phd2_path().ok_or_else(|| {
            Phd2Error::ExecutableNotFound(
                "PHD2 executable not found in default locations".to_string(),
            )
        })
    }

    /// Start PHD2 process
    pub async fn start_phd2(&self) -> Result<()> {
        // Check if already running
        if self.is_phd2_running().await {
            debug!("PHD2 is already running");
            return Ok(());
        }

        // Check if we already have a managed process
        {
            let process = self.process.lock().await;
            if process.is_some() {
                return Err(Phd2Error::ProcessAlreadyRunning);
            }
        }

        let executable = self.get_executable_path()?;
        debug!("Starting PHD2 from: {}", executable.display());

        let child = Command::new(&executable)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                Phd2Error::ProcessStartFailed(format!(
                    "Failed to start {}: {}",
                    executable.display(),
                    e
                ))
            })?;

        debug!("PHD2 process started with PID: {:?}", child.id());

        // Store the child process
        {
            let mut process = self.process.lock().await;
            *process = Some(child);
        }

        // Wait for PHD2 to be ready (TCP port available)
        self.wait_for_ready().await?;

        debug!("PHD2 is ready and accepting connections");
        Ok(())
    }

    /// Wait for PHD2 to be ready (TCP port accepting connections)
    async fn wait_for_ready(&self) -> Result<()> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let timeout = std::time::Duration::from_secs(self.config.connection_timeout_seconds);
        let start = std::time::Instant::now();
        let poll_interval = std::time::Duration::from_millis(500);

        debug!("Waiting for PHD2 to be ready at {}...", addr);

        while start.elapsed() < timeout {
            if TcpStream::connect(&addr).await.is_ok() {
                return Ok(());
            }

            // Check if process is still running
            {
                let mut process = self.process.lock().await;
                if let Some(ref mut child) = *process {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            return Err(Phd2Error::ProcessStartFailed(format!(
                                "PHD2 process exited prematurely with status: {}",
                                status
                            )));
                        }
                        Ok(None) => {
                            // Still running, continue waiting
                        }
                        Err(e) => {
                            return Err(Phd2Error::ProcessStartFailed(format!(
                                "Failed to check process status: {}",
                                e
                            )));
                        }
                    }
                }
            }

            tokio::time::sleep(poll_interval).await;
        }

        Err(Phd2Error::Timeout(format!(
            "PHD2 did not become ready within {} seconds",
            self.config.connection_timeout_seconds
        )))
    }

    /// Stop PHD2 process gracefully
    ///
    /// If a client is provided, it will first try to send the shutdown RPC command.
    /// If that fails or no client is provided, it will kill the process directly.
    pub async fn stop_phd2(&self, client: Option<&Phd2Client>) -> Result<()> {
        // Try graceful shutdown via RPC first
        if let Some(client) = client {
            if client.is_connected().await {
                debug!("Attempting graceful shutdown via RPC...");
                match client.shutdown_phd2().await {
                    Ok(()) => {
                        debug!("Shutdown command sent, waiting for process to exit...");
                        // Wait for process to exit
                        if self.wait_for_exit().await.is_ok() {
                            return Ok(());
                        }
                    }
                    Err(e) => {
                        debug!("Graceful shutdown failed: {}, will force kill", e);
                    }
                }
            }
        }

        // Force kill the process
        self.kill_process().await
    }

    /// Wait for the PHD2 process to exit
    async fn wait_for_exit(&self) -> Result<()> {
        let timeout = std::time::Duration::from_secs(10);
        let start = std::time::Instant::now();
        let poll_interval = std::time::Duration::from_millis(500);

        while start.elapsed() < timeout {
            {
                let mut process = self.process.lock().await;
                if let Some(ref mut child) = *process {
                    match child.try_wait() {
                        Ok(Some(_)) => {
                            *process = None;
                            return Ok(());
                        }
                        Ok(None) => {
                            // Still running
                        }
                        Err(e) => {
                            debug!("Error checking process status: {}", e);
                        }
                    }
                } else {
                    // No process being managed
                    return Ok(());
                }
            }
            tokio::time::sleep(poll_interval).await;
        }

        Err(Phd2Error::Timeout(
            "Process did not exit within timeout".to_string(),
        ))
    }

    /// Kill the PHD2 process forcefully
    async fn kill_process(&self) -> Result<()> {
        let mut process = self.process.lock().await;
        if let Some(mut child) = process.take() {
            debug!("Killing PHD2 process...");
            if let Err(e) = child.kill().await {
                debug!("Error killing process: {}", e);
            }
            // Wait for the process to be reaped
            let _ = child.wait().await;
        }
        Ok(())
    }

    /// Check if we are managing a PHD2 process
    pub async fn has_managed_process(&self) -> bool {
        let process = self.process.lock().await;
        process.is_some()
    }
}

// ============================================================================
// Configuration Loading
// ============================================================================

/// Load configuration from a JSON file
pub fn load_config(path: &PathBuf) -> std::result::Result<Config, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let config: Config = serde_json::from_str(&content)?;
    Ok(config)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpc_request_serialization() {
        let request = RpcRequest::new("get_app_state", None, 1);
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"get_app_state\""));
        assert!(json.contains("\"id\":1"));
        assert!(!json.contains("params"));
    }

    #[test]
    fn test_rpc_request_with_params() {
        let request = RpcRequest::new("set_connected", Some(serde_json::json!(true)), 2);
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"params\":true"));
    }

    #[test]
    fn test_rpc_response_parsing() {
        let json = r#"{"jsonrpc":"2.0","result":"Guiding","id":1}"#;
        let response: RpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, 1);
        assert_eq!(response.result.unwrap().as_str().unwrap(), "Guiding");
        assert!(response.error.is_none());
    }

    #[test]
    fn test_rpc_error_response_parsing() {
        let json =
            r#"{"jsonrpc":"2.0","error":{"code":-32600,"message":"Invalid request"},"id":1}"#;
        let response: RpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, 1);
        assert!(response.result.is_none());
        let error = response.error.unwrap();
        assert_eq!(error.code, -32600);
        assert_eq!(error.message, "Invalid request");
    }

    #[test]
    fn test_version_event_parsing() {
        let json = r#"{"Event":"Version","PHDVersion":"2.6.11","PHDSubver":"","MsgVersion":1}"#;
        let event: Phd2Event = serde_json::from_str(json).unwrap();
        match event {
            Phd2Event::Version { phd_version, .. } => {
                assert_eq!(phd_version, "2.6.11");
            }
            _ => panic!("Expected Version event"),
        }
    }

    #[test]
    fn test_app_state_event_parsing() {
        let json = r#"{"Event":"AppState","State":"Guiding"}"#;
        let event: Phd2Event = serde_json::from_str(json).unwrap();
        match event {
            Phd2Event::AppState { state } => {
                assert_eq!(state, "Guiding");
            }
            _ => panic!("Expected AppState event"),
        }
    }

    #[test]
    fn test_app_state_from_str() {
        assert_eq!("Stopped".parse::<AppState>().unwrap(), AppState::Stopped);
        assert_eq!("Guiding".parse::<AppState>().unwrap(), AppState::Guiding);
        assert_eq!(
            "Calibrating".parse::<AppState>().unwrap(),
            AppState::Calibrating
        );
        assert!("Unknown".parse::<AppState>().is_err());
    }

    #[test]
    fn test_settle_params_default() {
        let params = SettleParams::default();
        assert_eq!(params.pixels, 0.5);
        assert_eq!(params.time, 10);
        assert_eq!(params.timeout, 60);
    }

    #[test]
    fn test_phd2_config_default() {
        let config = Phd2Config::default();
        assert_eq!(config.host, "localhost");
        assert_eq!(config.port, 4400);
        assert_eq!(config.connection_timeout_seconds, 10);
        assert_eq!(config.command_timeout_seconds, 30);
        assert!(!config.auto_start);
        assert!(!config.auto_connect_equipment);
    }

    #[test]
    fn test_guide_step_parsing() {
        let json = r#"{"Event":"GuideStep","Frame":1,"Time":1.5,"Mount":"Mount","dx":0.5,"dy":-0.3,"RADistanceRaw":0.4,"DECDistanceRaw":-0.2}"#;
        let event: Phd2Event = serde_json::from_str(json).unwrap();
        match event {
            Phd2Event::GuideStep(stats) => {
                assert_eq!(stats.frame, 1);
                assert_eq!(stats.dx, 0.5);
                assert_eq!(stats.dy, -0.3);
            }
            _ => panic!("Expected GuideStep event"),
        }
    }

    #[test]
    fn test_star_lost_event_parsing() {
        let json = r#"{"Event":"StarLost","Frame":10,"Time":5.0,"StarMass":1000.0,"SNR":15.5,"Status":"Lost"}"#;
        let event: Phd2Event = serde_json::from_str(json).unwrap();
        match event {
            Phd2Event::StarLost { frame, snr, .. } => {
                assert_eq!(frame, 10);
                assert_eq!(snr, 15.5);
            }
            _ => panic!("Expected StarLost event"),
        }
    }

    #[test]
    fn test_profile_parsing() {
        let json = r#"{"id":1,"name":"Default Equipment"}"#;
        let profile: Profile = serde_json::from_str(json).unwrap();
        assert_eq!(profile.id, 1);
        assert_eq!(profile.name, "Default Equipment");
    }
}
