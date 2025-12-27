use ascom_alpaca::api::{Device, SafetyMonitor};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{interval, Duration};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub device: DeviceConfig,
    pub file: FileConfig,
    pub parsing: ParsingConfig,
    pub server: ServerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub name: String,
    pub unique_id: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileConfig {
    pub path: PathBuf,
    pub polling_interval_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsingConfig {
    pub rules: Vec<ParsingRule>,
    pub default_safe: bool,
    pub case_sensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsingRule {
    #[serde(rename = "type")]
    pub rule_type: RuleType,
    pub pattern: String,
    pub safe: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleType {
    Contains,
    Regex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
    pub device_number: u32,
}

#[derive(Debug)]
pub struct FileMonitorDevice {
    config: Config,
    connected: Arc<RwLock<bool>>,
    last_content: Arc<Mutex<Option<String>>>,
    polling_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl FileMonitorDevice {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            connected: Arc::new(RwLock::new(false)),
            last_content: Arc::new(Mutex::new(None)),
            polling_handle: Arc::new(Mutex::new(None)),
        }
    }

    pub fn evaluate_safety(&self, content: &str) -> bool {
        for rule in &self.config.parsing.rules {
            let matches = match rule.rule_type {
                RuleType::Contains => {
                    if self.config.parsing.case_sensitive {
                        content.contains(&rule.pattern)
                    } else {
                        content
                            .to_lowercase()
                            .contains(&rule.pattern.to_lowercase())
                    }
                }
                RuleType::Regex => {
                    let pattern = if self.config.parsing.case_sensitive {
                        rule.pattern.clone()
                    } else {
                        format!("(?i){}", rule.pattern)
                    };

                    match regex::Regex::new(&pattern) {
                        Ok(re) => re.is_match(content),
                        Err(_) => false, // Invalid regex patterns don't match
                    }
                }
            };

            if matches {
                return rule.safe;
            }
        }

        self.config.parsing.default_safe
    }

    fn read_file(&self) -> Result<String, std::io::Error> {
        std::fs::read_to_string(&self.config.file.path)
    }

    async fn start_polling(&self) {
        let config = self.config.clone();
        let last_content = Arc::clone(&self.last_content);
        let connected = Arc::clone(&self.connected);

        let handle = tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(config.file.polling_interval_seconds));

            loop {
                interval.tick().await;

                // Check if still connected
                if !*connected.read().await {
                    break;
                }

                // Read and store file content
                if let Ok(content) = std::fs::read_to_string(&config.file.path) {
                    let mut last = last_content.lock().await;
                    *last = Some(content);
                }
            }
        });

        let mut polling_handle = self.polling_handle.lock().await;
        *polling_handle = Some(handle);
    }

    async fn stop_polling(&self) {
        let mut handle = self.polling_handle.lock().await;
        if let Some(h) = handle.take() {
            h.abort();
        }
    }
}

#[async_trait::async_trait]
impl Device for FileMonitorDevice {
    fn static_name(&self) -> &str {
        &self.config.device.name
    }

    fn unique_id(&self) -> &str {
        &self.config.device.unique_id
    }

    async fn description(&self) -> ASCOMResult<String> {
        Ok(self.config.device.description.clone())
    }

    async fn connected(&self) -> ASCOMResult<bool> {
        Ok(*self.connected.read().await)
    }

    async fn set_connected(&self, connected: bool) -> Result<(), ASCOMError> {
        if connected {
            // Reload the file
            match self.read_file() {
                Ok(content) => {
                    let mut last_content = self.last_content.lock().await;
                    *last_content = Some(content);
                }
                Err(e) => {
                    return Err(ASCOMError::new(
                        ASCOMErrorCode::NOT_CONNECTED,
                        format!("Failed to read file: {}", e),
                    ));
                }
            }

            // Set connected state
            let mut conn_state = self.connected.write().await;
            *conn_state = true;

            // Start polling
            self.start_polling().await;
        } else {
            // Set disconnected state
            let mut conn_state = self.connected.write().await;
            *conn_state = false;

            // Stop polling
            self.stop_polling().await;
        }

        Ok(())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok(self.config.device.description.clone())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok("0.1.0".to_string())
    }
}

#[async_trait::async_trait]
impl SafetyMonitor for FileMonitorDevice {
    async fn is_safe(&self) -> ASCOMResult<bool> {
        if !*self.connected.read().await {
            return Err(ASCOMError::new(
                ASCOMErrorCode::NOT_CONNECTED,
                "Device is not connected".to_string(),
            ));
        }

        let last_content = self.last_content.lock().await;
        match &*last_content {
            Some(content) => Ok(self.evaluate_safety(content)),
            None => Ok(self.config.parsing.default_safe),
        }
    }
}

pub fn load_config(path: &PathBuf) -> Result<Config, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let config: Config = serde_json::from_str(&content)?;
    Ok(config)
}

pub async fn start_server(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    use ascom_alpaca::api::CargoServerInfo;
    use ascom_alpaca::Server;
    use std::net::SocketAddr;

    let device = FileMonitorDevice::new(config.clone());

    let mut server = Server::new(CargoServerInfo!());
    server.listen_addr = SocketAddr::from(([0, 0, 0, 0], config.server.port));
    server.devices.register(device);

    tracing::info!(
        "Starting ASCOM Alpaca server on port {}",
        config.server.port
    );
    tracing::info!(
        "Device: {} ({})",
        config.device.name,
        config.device.unique_id
    );
    tracing::info!("Monitoring file: {:?}", config.file.path);

    server.start().await?;

    Ok(())
}

#[cfg(test)]
mod async_concurrency_tests;

#[cfg(test)]
mod property_tests;
