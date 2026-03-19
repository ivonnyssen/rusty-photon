//! QHY FilterWheel device implementation
//!
//! Implements the ASCOM Alpaca Device and FilterWheel traits for QHYCCD filter wheels.

use ascom_alpaca::api::{Device, FilterWheel};
use ascom_alpaca::{ASCOMError, ASCOMResult};
use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::{debug, error};

use crate::config::FilterWheelConfig;
use crate::io::FilterWheelHandle;

/// Guard macro that returns NOT_CONNECTED if the device is not connected.
macro_rules! ensure_connected {
    ($self:ident) => {
        if !$self.connected().await.is_ok_and(|connected| connected) {
            debug!("FilterWheel device not connected");
            return Err(ASCOMError::NOT_CONNECTED);
        }
    };
}

/// QHY FilterWheel device for ASCOM Alpaca
pub struct QhyccdFilterWheel {
    config: FilterWheelConfig,
    device: Box<dyn FilterWheelHandle>,
    number_of_filters: RwLock<Option<u32>>,
    target_position: RwLock<Option<u32>>,
}

impl std::fmt::Debug for QhyccdFilterWheel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QhyccdFilterWheel")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl QhyccdFilterWheel {
    /// Create a new QHY FilterWheel device
    pub fn new(config: FilterWheelConfig, device: Box<dyn FilterWheelHandle>) -> Self {
        Self {
            config,
            device,
            number_of_filters: RwLock::new(None),
            target_position: RwLock::new(None),
        }
    }
}

#[async_trait]
impl Device for QhyccdFilterWheel {
    fn static_name(&self) -> &str {
        &self.config.name
    }

    fn unique_id(&self) -> &str {
        &self.config.unique_id
    }

    async fn connected(&self) -> ASCOMResult<bool> {
        self.device.is_open().map_err(|e| {
            error!("is_open failed: {}", e);
            ASCOMError::NOT_CONNECTED
        })
    }

    async fn set_connected(&self, connected: bool) -> ASCOMResult<()> {
        if self.connected().await? == connected {
            return Ok(());
        }
        match connected {
            true => {
                self.device.open().map_err(|e| {
                    error!("open failed: {}", e);
                    ASCOMError::NOT_CONNECTED
                })?;
                let num = self.device.get_number_of_filters().map_err(|e| {
                    error!("get_number_of_filters failed: {}", e);
                    ASCOMError::NOT_CONNECTED
                })?;
                *self.number_of_filters.write().await = Some(num);
                let pos = self.device.get_fw_position().map_err(|e| {
                    error!("get_fw_position failed: {}", e);
                    ASCOMError::NOT_CONNECTED
                })?;
                *self.target_position.write().await = Some(pos);
                debug!("FilterWheel connected: {} filters, position {}", num, pos);
                Ok(())
            }
            false => {
                self.device.close().map_err(|e| {
                    error!("close failed: {}", e);
                    ASCOMError::NOT_CONNECTED
                })?;
                *self.number_of_filters.write().await = None;
                *self.target_position.write().await = None;
                debug!("FilterWheel disconnected");
                Ok(())
            }
        }
    }

    async fn description(&self) -> ASCOMResult<String> {
        Ok(self.config.description.clone())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("QHY FilterWheel Driver - ASCOM Alpaca interface for QHYCCD filter wheels".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }
}

#[async_trait]
impl FilterWheel for QhyccdFilterWheel {
    async fn focus_offsets(&self) -> ASCOMResult<Vec<i32>> {
        ensure_connected!(self);
        let Some(num) = *self.number_of_filters.read().await else {
            return Err(ASCOMError::NOT_CONNECTED);
        };
        Ok(vec![0; num as usize])
    }

    async fn names(&self) -> ASCOMResult<Vec<String>> {
        ensure_connected!(self);
        let Some(num) = *self.number_of_filters.read().await else {
            return Err(ASCOMError::NOT_CONNECTED);
        };
        if !self.config.filter_names.is_empty() {
            let mut names = self.config.filter_names.clone();
            names.resize(num as usize, String::new());
            // Fill any missing names with defaults
            for (i, name) in names.iter_mut().enumerate() {
                if name.is_empty() {
                    *name = format!("Filter{}", i);
                }
            }
            Ok(names.into_iter().take(num as usize).collect())
        } else {
            Ok((0..num).map(|i| format!("Filter{}", i)).collect())
        }
    }

    async fn position(&self) -> ASCOMResult<Option<usize>> {
        ensure_connected!(self);
        let Some(target) = *self.target_position.read().await else {
            return Err(ASCOMError::NOT_CONNECTED);
        };
        let actual = self.device.get_fw_position().map_err(|e| {
            error!("get_fw_position failed: {}", e);
            ASCOMError::INVALID_OPERATION
        })?;
        if actual == target {
            Ok(Some(actual as usize))
        } else {
            debug!("filter wheel moving: target={}, actual={}", target, actual);
            Ok(None)
        }
    }

    async fn set_position(&self, position: usize) -> ASCOMResult<()> {
        ensure_connected!(self);
        let Some(num) = *self.number_of_filters.read().await else {
            return Err(ASCOMError::NOT_CONNECTED);
        };
        if !(0..num).contains(&(position as u32)) {
            return Err(ASCOMError::INVALID_VALUE);
        }
        let mut lock = self.target_position.write().await;
        if lock.is_some_and(|target| target == position as u32) {
            return Ok(());
        }
        self.device.set_fw_position(position as u32).map_err(|e| {
            error!("set_fw_position failed: {}", e);
            ASCOMError::INVALID_OPERATION
        })?;
        *lock = Some(position as u32);
        Ok(())
    }
}
