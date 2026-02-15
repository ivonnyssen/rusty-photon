//! Monitor trait and state types

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Instant;

/// The state of a monitored device
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorState {
    Safe,
    Unsafe,
    Unknown,
}

impl fmt::Display for MonitorState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MonitorState::Safe => write!(f, "Safe"),
            MonitorState::Unsafe => write!(f, "Unsafe"),
            MonitorState::Unknown => write!(f, "Unknown"),
        }
    }
}

/// A change in monitor state
#[derive(Debug, Clone)]
pub struct StateChange {
    pub monitor_name: String,
    pub previous: MonitorState,
    pub current: MonitorState,
    pub timestamp: Instant,
}

/// Trait for polling a device's state
#[async_trait]
pub trait Monitor: Send + Sync + std::fmt::Debug {
    /// Get the monitor name
    fn name(&self) -> &str;

    /// Poll the current state of the device
    async fn poll(&self) -> MonitorState;

    /// Connect to the device
    async fn connect(&self) -> crate::Result<()>;

    /// Disconnect from the device
    async fn disconnect(&self) -> crate::Result<()>;
}
