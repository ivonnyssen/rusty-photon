//! Connection management for PHD2 client
//!
//! This module handles TCP connection establishment, reconnection logic,
//! and message reading from PHD2.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::{broadcast, Mutex, Notify, RwLock};
use tracing::{debug, info, warn};

use crate::config::ReconnectConfig;
use crate::error::Phd2Error;
use crate::events::{AppState, Phd2Event};
#[cfg(test)]
use crate::io::TcpConnectionFactory;
use crate::io::{ConnectionFactory, LineReader, MessageWriter};
use crate::rpc::RpcResponse;

/// Pending RPC request waiting for response
pub(crate) struct PendingRequest {
    pub sender: tokio::sync::oneshot::Sender<std::result::Result<serde_json::Value, Phd2Error>>,
}

/// Internal client connection state
#[derive(Debug, Clone, Default)]
pub(crate) struct ConnectionState {
    pub connected: bool,
    pub phd2_version: Option<String>,
    pub app_state: Option<AppState>,
    pub reconnecting: bool,
}

/// Shared state for connection management
///
/// This struct holds all the Arc-wrapped state that needs to be shared
/// between the client, reader task, and reconnection task.
#[derive(Clone)]
pub(crate) struct SharedConnectionState {
    pub state: Arc<RwLock<ConnectionState>>,
    pub writer: Arc<Mutex<Option<Box<dyn MessageWriter>>>>,
    pub pending_requests: Arc<Mutex<HashMap<u64, PendingRequest>>>,
    pub event_sender: broadcast::Sender<Phd2Event>,
    pub reader_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    pub auto_reconnect_enabled: Arc<AtomicBool>,
    pub stop_reconnect: Arc<Notify>,
    pub connection_factory: Arc<dyn ConnectionFactory>,
}

impl SharedConnectionState {
    /// Create a new shared connection state with a TCP connection factory (test only)
    #[cfg(test)]
    pub fn new(auto_reconnect_enabled: bool) -> Self {
        Self::with_factory(
            auto_reconnect_enabled,
            Arc::new(TcpConnectionFactory::new()),
        )
    }

    /// Create a new shared connection state with a custom connection factory
    pub fn with_factory(
        auto_reconnect_enabled: bool,
        connection_factory: Arc<dyn ConnectionFactory>,
    ) -> Self {
        let (event_sender, _) = broadcast::channel(100);
        Self {
            state: Arc::new(RwLock::new(ConnectionState::default())),
            writer: Arc::new(Mutex::new(None)),
            pending_requests: Arc::new(Mutex::new(HashMap::new())),
            event_sender,
            reader_handle: Arc::new(Mutex::new(None)),
            auto_reconnect_enabled: Arc::new(AtomicBool::new(auto_reconnect_enabled)),
            stop_reconnect: Arc::new(Notify::new()),
            connection_factory,
        }
    }

    /// Check if connected
    pub async fn is_connected(&self) -> bool {
        self.state.read().await.connected
    }

    /// Check if reconnecting
    pub async fn is_reconnecting(&self) -> bool {
        self.state.read().await.reconnecting
    }

    /// Get PHD2 version
    pub async fn get_phd2_version(&self) -> Option<String> {
        self.state.read().await.phd2_version.clone()
    }

    /// Get cached app state
    pub async fn get_cached_app_state(&self) -> Option<AppState> {
        self.state.read().await.app_state
    }

    /// Check if auto-reconnect is enabled
    pub fn is_auto_reconnect_enabled(&self) -> bool {
        self.auto_reconnect_enabled.load(Ordering::SeqCst)
    }

    /// Set auto-reconnect enabled state
    pub fn set_auto_reconnect_enabled(&self, enabled: bool) {
        debug!("Setting auto-reconnect enabled: {}", enabled);
        self.auto_reconnect_enabled.store(enabled, Ordering::SeqCst);
        if !enabled {
            self.stop_reconnect.notify_waiters();
        }
    }

    /// Stop ongoing reconnection attempts
    pub async fn stop_reconnection(&self) {
        debug!("Stopping reconnection attempts");
        self.stop_reconnect.notify_waiters();
        let mut state = self.state.write().await;
        state.reconnecting = false;
    }
}

/// Configuration for connection attempts
#[derive(Clone)]
pub(crate) struct ConnectionConfig {
    pub host: String,
    pub port: u16,
    pub connection_timeout_seconds: u64,
    pub reconnect: ReconnectConfig,
}

/// Spawn a reconnection task that attempts to reconnect to PHD2
pub(crate) fn spawn_reconnect_task(
    config: ConnectionConfig,
    shared: SharedConnectionState,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Set reconnecting state
        {
            let mut state_guard = shared.state.write().await;
            state_guard.reconnecting = true;
        }

        let addr = format!("{}:{}", config.host, config.port);
        let interval = std::time::Duration::from_secs(config.reconnect.interval_seconds);
        let timeout_duration = std::time::Duration::from_secs(config.connection_timeout_seconds);
        let max_retries = config.reconnect.max_retries;
        let mut attempt = 0u32;

        loop {
            attempt += 1;

            // Check if we should stop reconnecting
            if !shared.auto_reconnect_enabled.load(Ordering::SeqCst) {
                debug!("Auto-reconnect disabled, stopping reconnection attempts");
                let _ = shared.event_sender.send(Phd2Event::ReconnectFailed {
                    reason: "Auto-reconnect disabled".to_string(),
                });
                break;
            }

            // Check if max retries exceeded
            if let Some(max) = max_retries {
                if attempt > max {
                    warn!("Reconnection failed: max retries ({}) exceeded", max);
                    let _ = shared.event_sender.send(Phd2Event::ReconnectFailed {
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
            let _ = shared.event_sender.send(Phd2Event::Reconnecting {
                attempt,
                max_attempts: max_retries,
            });

            // Wait before attempting connection (unless first attempt)
            if attempt > 1 {
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {}
                    _ = shared.stop_reconnect.notified() => {
                        debug!("Reconnection stopped by user");
                        let _ = shared.event_sender.send(Phd2Event::ReconnectFailed {
                            reason: "Reconnection cancelled".to_string(),
                        });
                        break;
                    }
                }
            }

            // Attempt connection using the connection factory
            debug!("Attempting connection to {}", addr);
            match shared
                .connection_factory
                .connect(&addr, timeout_duration)
                .await
            {
                Ok(connection_pair) => {
                    debug!("Connection established to PHD2");

                    // Store the writer
                    {
                        let mut writer_guard = shared.writer.lock().await;
                        *writer_guard = Some(connection_pair.writer);
                    }

                    // Update connection state
                    {
                        let mut state_guard = shared.state.write().await;
                        state_guard.connected = true;
                        state_guard.reconnecting = false;
                    }

                    // Start a new reader task
                    let new_reader_handle =
                        spawn_reader_task(connection_pair.reader, config.clone(), shared.clone());

                    // Store the new reader handle
                    {
                        let mut handle = shared.reader_handle.lock().await;
                        *handle = Some(new_reader_handle);
                    }

                    // Broadcast reconnected event
                    info!("Successfully reconnected to PHD2");
                    let _ = shared.event_sender.send(Phd2Event::Reconnected);
                    return;
                }
                Err(e) => {
                    debug!("Connection attempt {} failed: {}", attempt, e);
                }
            }
        }

        // Reconnection failed - update state
        {
            let mut state_guard = shared.state.write().await;
            state_guard.reconnecting = false;
        }
    })
}

/// Spawn a reader task that reads messages from PHD2
pub(crate) fn spawn_reader_task(
    mut reader: Box<dyn LineReader>,
    config: ConnectionConfig,
    shared: SharedConnectionState,
) -> tokio::task::JoinHandle<()> {
    let reconnect_handle = Arc::new(Mutex::new(None));

    tokio::spawn(async move {
        let disconnect_reason;

        loop {
            match reader.read_line().await {
                Ok(None) => {
                    debug!("PHD2 connection closed");
                    disconnect_reason = "Connection closed by remote".to_string();
                    break;
                }
                Ok(Some(line)) => {
                    if line.is_empty() {
                        continue;
                    }

                    debug!("Received from PHD2: {}", line);

                    // Try to parse as a response first (has "id" field)
                    if let Ok(response) = serde_json::from_str::<RpcResponse>(&line) {
                        let mut pending = shared.pending_requests.lock().await;
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
                    } else if let Ok(event) = serde_json::from_str::<Phd2Event>(&line) {
                        // Handle specific events to update internal state
                        match &event {
                            Phd2Event::Version { phd_version, .. } => {
                                let mut state_guard = shared.state.write().await;
                                state_guard.phd2_version = Some(phd_version.clone());
                                debug!("PHD2 version: {}", phd_version);
                            }
                            Phd2Event::AppState { state: app_state } => {
                                if let Ok(parsed_state) = app_state.parse::<AppState>() {
                                    let mut state_guard = shared.state.write().await;
                                    state_guard.app_state = Some(parsed_state);
                                    debug!("PHD2 app state: {}", parsed_state);
                                }
                            }
                            _ => {}
                        }

                        // Broadcast event to subscribers
                        let _ = shared.event_sender.send(event);
                    } else {
                        debug!("Failed to parse PHD2 message: {}", line);
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
            let mut state_guard = shared.state.write().await;
            state_guard.connected = false;
        }

        // Broadcast connection lost event
        warn!("PHD2 connection lost: {}", disconnect_reason);
        let _ = shared.event_sender.send(Phd2Event::ConnectionLost {
            reason: disconnect_reason.clone(),
        });

        // Clear pending requests
        {
            let mut pending = shared.pending_requests.lock().await;
            pending.clear();
        }

        // Close the writer
        {
            let mut writer_guard = shared.writer.lock().await;
            if let Some(mut w) = writer_guard.take() {
                let _ = w.shutdown().await;
            }
        }

        // Start reconnection if enabled
        if shared.auto_reconnect_enabled.load(Ordering::SeqCst) {
            debug!("Auto-reconnect enabled, starting reconnection task");
            let reconnect_task = spawn_reconnect_task(config, shared);
            let mut handle = reconnect_handle.lock().await;
            *handle = Some(reconnect_task);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_state_default() {
        let state = ConnectionState::default();
        assert!(!state.connected);
        assert!(state.phd2_version.is_none());
        assert!(state.app_state.is_none());
        assert!(!state.reconnecting);
    }

    #[test]
    fn test_shared_connection_state_auto_reconnect_enabled() {
        let shared = SharedConnectionState::new(true);
        assert!(shared.is_auto_reconnect_enabled());
    }

    #[test]
    fn test_shared_connection_state_auto_reconnect_disabled() {
        let shared = SharedConnectionState::new(false);
        assert!(!shared.is_auto_reconnect_enabled());
    }

    #[test]
    fn test_shared_connection_state_toggle_auto_reconnect() {
        let shared = SharedConnectionState::new(true);
        assert!(shared.is_auto_reconnect_enabled());

        shared.set_auto_reconnect_enabled(false);
        assert!(!shared.is_auto_reconnect_enabled());

        shared.set_auto_reconnect_enabled(true);
        assert!(shared.is_auto_reconnect_enabled());
    }

    #[tokio::test]
    async fn test_shared_connection_state_initial_values() {
        let shared = SharedConnectionState::new(true);
        assert!(!shared.is_connected().await);
        assert!(!shared.is_reconnecting().await);
        assert!(shared.get_phd2_version().await.is_none());
        assert!(shared.get_cached_app_state().await.is_none());
    }

    #[tokio::test]
    async fn test_shared_connection_state_update_connected() {
        let shared = SharedConnectionState::new(true);

        {
            let mut state = shared.state.write().await;
            state.connected = true;
        }

        assert!(shared.is_connected().await);
    }

    #[tokio::test]
    async fn test_shared_connection_state_update_version() {
        let shared = SharedConnectionState::new(true);

        {
            let mut state = shared.state.write().await;
            state.phd2_version = Some("2.6.11".to_string());
        }

        assert_eq!(shared.get_phd2_version().await, Some("2.6.11".to_string()));
    }
}
