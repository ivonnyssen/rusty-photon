//! PPBA ObservingConditions device implementation
//!
//! This module implements the ASCOM Alpaca ObservingConditions trait
//! for the Pegasus Astro Pocket Powerbox Advance Gen2 environmental sensors.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::{Device, ObservingConditions};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::debug;

use crate::config::ObservingConditionsConfig;
use crate::error::PpbaError;
use crate::serial_manager::SerialManager;

/// Guard macro that returns NOT_CONNECTED if the device is not connected.
macro_rules! ensure_connected {
    ($self:ident) => {
        if !$self.connected().await.is_ok_and(|connected| connected) {
            debug!("ObservingConditions device not connected");
            return Err(ASCOMError::NOT_CONNECTED);
        }
    };
}

/// PPBA ObservingConditions device for ASCOM Alpaca
pub struct PpbaObservingConditionsDevice {
    config: ObservingConditionsConfig,
    requested_connection: Arc<RwLock<bool>>,
    serial_manager: Arc<SerialManager>,
}

impl fmt::Debug for PpbaObservingConditionsDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PpbaObservingConditionsDevice")
            .field("config", &self.config)
            .field("requested_connection", &self.requested_connection)
            .finish_non_exhaustive()
    }
}

impl PpbaObservingConditionsDevice {
    /// Create a new PPBA ObservingConditions device
    pub fn new(config: ObservingConditionsConfig, serial_manager: Arc<SerialManager>) -> Self {
        Self {
            config,
            requested_connection: Arc::new(RwLock::new(false)),
            serial_manager,
        }
    }

    /// Convert internal error to ASCOM error
    fn to_ascom_error(err: PpbaError) -> ASCOMError {
        match err {
            PpbaError::NotConnected => {
                ASCOMError::new(ASCOMErrorCode::NOT_CONNECTED, err.to_string())
            }
            PpbaError::InvalidValue(_) => {
                ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, err.to_string())
            }
            _ => ASCOMError::invalid_operation(err.to_string()),
        }
    }
}

#[async_trait]
impl Device for PpbaObservingConditionsDevice {
    fn static_name(&self) -> &str {
        &self.config.name
    }

    fn unique_id(&self) -> &str {
        &self.config.unique_id
    }

    async fn description(&self) -> ASCOMResult<String> {
        Ok(self.config.description.clone())
    }

    async fn connected(&self) -> ASCOMResult<bool> {
        let requested = *self.requested_connection.read().await;
        let serial_ok = self.serial_manager.is_available();
        Ok(requested && serial_ok)
    }

    async fn set_connected(&self, connected: bool) -> ASCOMResult<()> {
        if self.connected().await? == connected {
            return Ok(());
        }
        match connected {
            true => {
                self.serial_manager
                    .connect()
                    .await
                    .map_err(Self::to_ascom_error)?;
                *self.requested_connection.write().await = true;
                debug!("ObservingConditions device connected");
            }
            false => {
                *self.requested_connection.write().await = false;
                self.serial_manager.disconnect().await;
                debug!("ObservingConditions device disconnected");
            }
        }
        Ok(())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("PPBA Driver - ObservingConditions interface for Pegasus Astro PPBA Gen2 environmental sensors".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }
}

#[async_trait]
impl ObservingConditions for PpbaObservingConditionsDevice {
    async fn average_period(&self) -> ASCOMResult<f64> {
        ensure_connected!(self);
        let cached = self.serial_manager.get_cached_state().await;
        let window = cached.temp_mean.window();

        // If window is 10 seconds, we're in instantaneous mode - return 0.0
        if window == Duration::from_secs(10) {
            return Ok(0.0);
        }

        // ASCOM spec requires hours, not seconds
        Ok(window.as_secs_f64() / 3600.0)
    }

    async fn set_average_period(&self, period: f64) -> ASCOMResult<()> {
        ensure_connected!(self);
        // ASCOM spec requires hours. Must accept 0.0 for instantaneous readings.
        // Per spec: "All drivers must accept 0.0 to specify that an instantaneous value is available"

        // Reject negative values
        if period < 0.0 {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!("Average period cannot be negative, got {}", period),
            ));
        }

        // Set a reasonable upper limit (24 hours)
        if period > 24.0 {
            return Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!("Average period cannot exceed 24 hours, got {}", period),
            ));
        }

        // Convert hours to Duration
        // Special case: 0.0 means instantaneous (use small but reasonable averaging window)
        let duration = if period == 0.0 {
            // Use 10 seconds for instantaneous - enough to avoid aging out samples
            // immediately while still being effectively instantaneous for astronomy
            Duration::from_secs(10)
        } else {
            Duration::from_secs_f64(period * 3600.0) // Convert hours to seconds
        };

        // Update mean calculators in SerialManager
        self.serial_manager.set_averaging_period(duration).await;

        debug!("Average period set to {} hours", period);
        Ok(())
    }

    async fn temperature(&self) -> ASCOMResult<f64> {
        ensure_connected!(self);

        let state = self.serial_manager.get_cached_state().await;
        state.temp_mean.get_mean().ok_or_else(|| {
            ASCOMError::new(
                ASCOMErrorCode::VALUE_NOT_SET,
                "No temperature data available yet",
            )
        })
    }

    async fn humidity(&self) -> ASCOMResult<f64> {
        ensure_connected!(self);

        let state = self.serial_manager.get_cached_state().await;
        state.humidity_mean.get_mean().ok_or_else(|| {
            ASCOMError::new(
                ASCOMErrorCode::VALUE_NOT_SET,
                "No humidity data available yet",
            )
        })
    }

    async fn dew_point(&self) -> ASCOMResult<f64> {
        ensure_connected!(self);

        let state = self.serial_manager.get_cached_state().await;
        state.dewpoint_mean.get_mean().ok_or_else(|| {
            ASCOMError::new(
                ASCOMErrorCode::VALUE_NOT_SET,
                "No dewpoint data available yet",
            )
        })
    }

    async fn time_since_last_update(&self, sensor_name: String) -> ASCOMResult<f64> {
        ensure_connected!(self);

        let state = self.serial_manager.get_cached_state().await;

        let duration = match sensor_name.to_lowercase().as_str() {
            // Empty string means "latest update time" across all sensors
            "" => {
                // Return the most recent update time among all implemented sensors
                let times = [
                    state.temp_mean.time_since_last_update(),
                    state.humidity_mean.time_since_last_update(),
                    state.dewpoint_mean.time_since_last_update(),
                ];
                // Get the minimum time (most recent update)
                times
                    .iter()
                    .filter_map(|&t| t)
                    .min()
                    .or(Some(Duration::ZERO))
            }
            "temperature" => state.temp_mean.time_since_last_update(),
            "humidity" => state.humidity_mean.time_since_last_update(),
            "dewpoint" => state.dewpoint_mean.time_since_last_update(),
            // Unimplemented sensors - return NOT_IMPLEMENTED error
            "cloudcover" | "pressure" | "rainrate" | "skybrightness" | "skyquality"
            | "starfwhm" | "skytemperature" | "winddirection" | "windgust" | "windspeed" => {
                return Err(ASCOMError::NOT_IMPLEMENTED);
            }
            // Truly unknown sensor name
            _ => {
                return Err(ASCOMError::new(
                    ASCOMErrorCode::INVALID_VALUE,
                    format!("Unknown sensor name: {}", sensor_name),
                ))
            }
        };

        Ok(duration.map(|d| d.as_secs_f64()).unwrap_or(f64::MAX))
    }

    async fn sensor_description(&self, sensor_name: String) -> ASCOMResult<String> {
        ensure_connected!(self);
        match sensor_name.to_lowercase().as_str() {
            "temperature" => Ok("PPBA internal temperature sensor".to_string()),
            "humidity" => Ok("PPBA internal humidity sensor".to_string()),
            "dewpoint" => Ok("Dewpoint calculated from temperature and humidity".to_string()),
            // Empty string is an invalid sensor name
            "" => Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                "Sensor name cannot be empty".to_string(),
            )),
            // Unimplemented sensors - return NOT_IMPLEMENTED error
            "cloudcover" | "pressure" | "rainrate" | "skybrightness" | "skyquality"
            | "starfwhm" | "skytemperature" | "winddirection" | "windgust" | "windspeed" => {
                Err(ASCOMError::NOT_IMPLEMENTED)
            }
            // Truly unknown sensor name
            _ => Err(ASCOMError::new(
                ASCOMErrorCode::INVALID_VALUE,
                format!("Unknown sensor name: {}", sensor_name),
            )),
        }
    }

    async fn refresh(&self) -> ASCOMResult<()> {
        ensure_connected!(self);

        // Trigger immediate refresh via SerialManager
        self.serial_manager
            .refresh_status()
            .await
            .map_err(Self::to_ascom_error)?;
        debug!("ObservingConditions sensors refreshed");
        Ok(())
    }

    // All other sensors are not implemented and use default trait implementations
    // which return NOT_IMPLEMENTED errors
}
