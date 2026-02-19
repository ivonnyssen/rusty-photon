//! Notifier trait for sending alerts

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A notification to be sent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub title: String,
    pub message: String,
    pub priority: i8,
    pub sound: Option<String>,
}

/// Record of a sent notification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationRecord {
    pub monitor_name: String,
    pub notifier_type: String,
    pub message: String,
    pub success: bool,
    pub error: Option<String>,
    pub timestamp_epoch_ms: u64,
}

/// Trait for sending notifications
#[async_trait]
pub trait Notifier: Send + Sync + std::fmt::Debug {
    /// Get the notifier type name (e.g. "pushover")
    fn type_name(&self) -> &str;

    /// Send a notification
    async fn notify(&self, notification: &Notification) -> crate::Result<()>;
}
