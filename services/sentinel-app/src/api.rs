//! Client-side API fetch helpers
//!
//! These types mirror the server-side JSON response structures
//! and are shared between SSR and client-side hydration.

use serde::{Deserialize, Serialize};

/// Monitor status as returned by /api/status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorStatusResponse {
    pub name: String,
    pub state: String,
    pub last_poll_epoch_ms: u64,
    pub last_change_epoch_ms: Option<u64>,
    pub consecutive_errors: u32,
}

/// Notification record as returned by /api/history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationHistoryResponse {
    pub monitor_name: String,
    pub notifier_type: String,
    pub message: String,
    pub success: bool,
    pub error: Option<String>,
    pub timestamp_epoch_ms: u64,
}
