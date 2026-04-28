#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use ascom_alpaca::api::{CargoServerInfo, Device, SafetyMonitor};
use ascom_alpaca::{ASCOMError, ASCOMErrorCode, ASCOMResult, Server};
use rp_tls::config::TlsConfig;
use serde::{Deserialize, Serialize};
use tokio::signal;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{interval, Duration};
use tracing::{debug, info};

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
    #[serde(with = "humantime_serde")]
    pub polling_interval: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsingConfig {
    pub rules: Vec<ParsingRule>,
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
    #[serde(default = "default_discovery_port")]
    pub discovery_port: Option<u16>,
    #[serde(default)]
    pub tls: Option<rp_tls::config::TlsConfig>,
    #[serde(default)]
    pub auth: Option<rp_auth::config::AuthConfig>,
}

fn default_discovery_port() -> Option<u16> {
    Some(ascom_alpaca::discovery::DEFAULT_DISCOVERY_PORT)
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

        false
    }

    fn read_file(&self) -> Result<String, std::io::Error> {
        std::fs::read_to_string(&self.config.file.path)
    }

    async fn start_polling(&self) {
        let config = self.config.clone();
        let last_content = Arc::clone(&self.last_content);
        let connected = Arc::clone(&self.connected);

        let handle = tokio::spawn(async move {
            let mut interval = interval(config.file.polling_interval);

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
            return Err(ASCOMError::NOT_CONNECTED);
        }

        let last_content = self.last_content.lock().await;
        match &*last_content {
            Some(content) => Ok(self.evaluate_safety(content)),
            None => Ok(false),
        }
    }
}

pub fn load_config(path: &PathBuf) -> Result<Config, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let config: Config = serde_json::from_str(&content)?;
    Ok(config)
}

/// Builder for the ASCOM Alpaca filemonitor server.
///
/// The returned [`BoundServer`] can be inspected (e.g. `listen_addr()`)
/// before calling `start()`.
pub struct ServerBuilder {
    config: Config,
}

impl ServerBuilder {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub async fn build(self) -> Result<BoundServer, Box<dyn std::error::Error>> {
        let device = FileMonitorDevice::new(self.config.clone());

        let mut server = Server::new(CargoServerInfo!());
        server.listen_addr = SocketAddr::from(([0, 0, 0, 0], self.config.server.port));
        server.discovery_port = self.config.server.discovery_port;
        server.devices.register(device);

        info!(
            "Starting ASCOM Alpaca server on port {}",
            self.config.server.port
        );
        info!(
            "Device: {} ({})",
            self.config.device.name, self.config.device.unique_id
        );
        info!("Monitoring file: {:?}", self.config.file.path);

        let tls = self.config.server.tls.clone();
        let router = axum::Router::new().fallback_service(server.into_service());

        // Layer authentication if configured
        let router = match &self.config.server.auth {
            Some(auth) => {
                if self.config.server.tls.is_none() {
                    tracing::warn!(
                        "Authentication is enabled but TLS is not. \
                         Credentials will be transmitted in cleartext. \
                         Consider enabling TLS (see `rp init-tls`)."
                    );
                }
                rp_auth::layer(router, auth)
            }
            None => router,
        };

        let listener = rp_tls::server::bind_dual_stack_tokio(SocketAddr::from((
            [0, 0, 0, 0],
            self.config.server.port,
        )))
        .await?;
        let local_addr = listener.local_addr()?;

        println!("Bound Alpaca server bound_addr={}", local_addr);

        Ok(BoundServer {
            listener,
            router,
            local_addr,
            tls,
        })
    }
}

/// A fully bound filemonitor server ready to accept connections.
pub struct BoundServer {
    listener: tokio::net::TcpListener,
    router: axum::Router,
    local_addr: SocketAddr,
    tls: Option<TlsConfig>,
}

impl BoundServer {
    pub fn listen_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn start(self) -> Result<(), Box<dyn std::error::Error>> {
        match self.tls {
            Some(ref tls_config) => {
                info!("filemonitor started on {} (TLS)", self.local_addr);
                rp_tls::server::serve_tls(
                    self.listener,
                    self.router,
                    tls_config,
                    shutdown_signal(),
                )
                .await?;
            }
            None => {
                info!("filemonitor started on {}", self.local_addr);
                rp_tls::server::serve_plain(self.listener, self.router, shutdown_signal()).await?;
            }
        }
        debug!("filemonitor shut down");
        Ok(())
    }
}

/// Legacy wrapper for backward compat with existing callers.
pub async fn start_server(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    ServerBuilder::new(config).build().await?.start().await
}

pub async fn run_server_loop(
    config_path: &Path,
    mut stop: impl FnMut() -> Pin<Box<dyn Future<Output = ()>>>,
    mut reload: impl FnMut() -> Pin<Box<dyn Future<Output = ()>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        let config = load_config(&config_path.to_path_buf())?;
        info!("Starting filemonitor server on port {}", config.server.port);
        tokio::select! {
            result = start_server(config) => return result,
            _ = stop() => { info!("Received stop signal"); break; }
            _ = reload() => { info!("Reloading configuration"); continue; }
        }
    }
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => debug!("received Ctrl+C"),
        () = terminate => debug!("received SIGTERM"),
    }
}

#[cfg(test)]
#[cfg(not(miri))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod property_tests {
    use super::*;
    use proptest::prelude::*;
    use std::path::PathBuf;

    fn create_case_insensitive_config() -> Config {
        Config {
            device: DeviceConfig {
                name: "Test Device".to_string(),
                unique_id: "test-123".to_string(),
                description: "Test Description".to_string(),
            },
            file: FileConfig {
                path: PathBuf::from("/tmp/test.txt"),
                polling_interval: Duration::from_secs(1),
            },
            parsing: ParsingConfig {
                rules: vec![
                    ParsingRule {
                        rule_type: RuleType::Contains,
                        pattern: "SAFE".to_string(),
                        safe: true,
                    },
                    ParsingRule {
                        rule_type: RuleType::Contains,
                        pattern: "DANGER".to_string(),
                        safe: false,
                    },
                ],
                case_sensitive: false,
            },
            server: ServerConfig {
                port: 8080,
                device_number: 0,
                discovery_port: None,
                tls: None,
                auth: None,
            },
        }
    }

    fn create_test_config() -> Config {
        Config {
            device: DeviceConfig {
                name: "Test Device".to_string(),
                unique_id: "test-123".to_string(),
                description: "Test Description".to_string(),
            },
            file: FileConfig {
                path: PathBuf::from("/tmp/test.txt"),
                polling_interval: Duration::from_secs(1),
            },
            parsing: ParsingConfig {
                rules: vec![
                    ParsingRule {
                        rule_type: RuleType::Contains,
                        pattern: "SAFE".to_string(),
                        safe: true,
                    },
                    ParsingRule {
                        rule_type: RuleType::Contains,
                        pattern: "DANGER".to_string(),
                        safe: false,
                    },
                ],
                case_sensitive: true,
            },
            server: ServerConfig {
                port: 8080,
                device_number: 0,
                discovery_port: None,
                tls: None,
                auth: None,
            },
        }
    }

    proptest! {
        #[test]
        fn test_safety_evaluation_consistency(content in ".*") {
            let config = create_test_config();
            let device = FileMonitorDevice::new(config);

            // Safety evaluation should be deterministic
            let result1 = device.evaluate_safety(&content);
            let result2 = device.evaluate_safety(&content);
            prop_assert_eq!(result1, result2);
        }

        #[test]
        fn test_safe_content_always_safe(safe_suffix in ".*") {
            let content = format!("SAFE {}", safe_suffix);
            let config = create_test_config();
            let device = FileMonitorDevice::new(config);

            prop_assert!(device.evaluate_safety(&content));
        }

        #[test]
        fn test_danger_content_always_unsafe(danger_suffix in ".*") {
            let content = format!("DANGER {}", danger_suffix);
            let config = create_test_config();
            let device = FileMonitorDevice::new(config);

            prop_assert!(!device.evaluate_safety(&content));
        }

        #[test]
        fn test_regex_pattern_consistency(
            pattern in "[a-zA-Z0-9]+",
            content in ".*"
        ) {
            let config = Config {
                device: DeviceConfig {
                    name: "Test".to_string(),
                    unique_id: "test".to_string(),
                    description: "Test".to_string(),
                },
                file: FileConfig {
                    path: PathBuf::from("/tmp/test.txt"),
                    polling_interval: Duration::from_secs(1),
                },
                parsing: ParsingConfig {
                    rules: vec![ParsingRule {
                        rule_type: RuleType::Regex,
                        pattern: pattern.clone(),
                        safe: true,
                    }],
                    case_sensitive: true,
                },
                server: ServerConfig {
                    port: 8080,
                    device_number: 0,
                    discovery_port: None,
                    tls: None,
                    auth: None,
                },
            };

            let device = FileMonitorDevice::new(config);

            // Should not panic on any input
            let _result = device.evaluate_safety(&content);
        }

        #[test]
        fn test_case_insensitive_contains_matches_any_case(
            prefix in "[a-zA-Z ]{0,10}",
            suffix in "[a-zA-Z ]{0,10}"
        ) {
            let config = create_case_insensitive_config();
            let device = FileMonitorDevice::new(config);

            // "safe" in any case should match
            let content = format!("{}safe{}", prefix, suffix);
            prop_assert!(device.evaluate_safety(&content));

            let content_upper = format!("{}SAFE{}", prefix, suffix);
            prop_assert!(device.evaluate_safety(&content_upper));

            let content_mixed = format!("{}SaFe{}", prefix, suffix);
            prop_assert!(device.evaluate_safety(&content_mixed));
        }

        #[test]
        fn test_case_insensitive_regex_consistency(
            pattern in "[a-zA-Z0-9]+",
            content in ".*"
        ) {
            let config = Config {
                device: DeviceConfig {
                    name: "Test".to_string(),
                    unique_id: "test".to_string(),
                    description: "Test".to_string(),
                },
                file: FileConfig {
                    path: PathBuf::from("/tmp/test.txt"),
                    polling_interval: Duration::from_secs(1),
                },
                parsing: ParsingConfig {
                    rules: vec![ParsingRule {
                        rule_type: RuleType::Regex,
                        pattern: pattern.clone(),
                        safe: true,
                    }],
                    case_sensitive: false,
                },
                server: ServerConfig {
                    port: 8080,
                    device_number: 0,
                    discovery_port: None,
                    tls: None,
                    auth: None,
                },
            };

            let device = FileMonitorDevice::new(config);

            // Should not panic and should be deterministic
            let result1 = device.evaluate_safety(&content);
            let result2 = device.evaluate_safety(&content);
            prop_assert_eq!(result1, result2);
        }

        #[test]
        fn test_config_round_trip_serialization(
            name in "[a-zA-Z ]{1,20}",
            unique_id in "[a-z0-9-]{1,20}",
            port in 1024u16..65535u16,
            polling in 1u64..3600u64,
        ) {
            let config = Config {
                device: DeviceConfig {
                    name,
                    unique_id,
                    description: "Test Description".to_string(),
                },
                file: FileConfig {
                    path: PathBuf::from("/tmp/test.txt"),
                    polling_interval: Duration::from_secs(polling),
                },
                parsing: ParsingConfig {
                    rules: vec![ParsingRule {
                        rule_type: RuleType::Contains,
                        pattern: "TEST".to_string(),
                        safe: true,
                    }],
                    case_sensitive: false,
                },
                server: ServerConfig {
                    port,
                    device_number: 0,
                    discovery_port: None,
                    tls: None,
                    auth: None,
                },
            };

            let json = serde_json::to_string(&config).unwrap();
            let deserialized: Config = serde_json::from_str(&json).unwrap();

            prop_assert_eq!(config.device.name, deserialized.device.name);
            prop_assert_eq!(config.device.unique_id, deserialized.device.unique_id);
            prop_assert_eq!(config.server.port, deserialized.server.port);
            prop_assert_eq!(config.file.polling_interval, deserialized.file.polling_interval);
            prop_assert_eq!(config.parsing.rules.len(), deserialized.parsing.rules.len());
        }

        #[test]
        fn test_invalid_regex_never_panics(
            pattern in ".*",
            content in ".*"
        ) {
            let config = Config {
                device: DeviceConfig {
                    name: "Test".to_string(),
                    unique_id: "test".to_string(),
                    description: "Test".to_string(),
                },
                file: FileConfig {
                    path: PathBuf::from("/tmp/test.txt"),
                    polling_interval: Duration::from_secs(1),
                },
                parsing: ParsingConfig {
                    rules: vec![ParsingRule {
                        rule_type: RuleType::Regex,
                        pattern,
                        safe: true,
                    }],
                    case_sensitive: false,
                },
                server: ServerConfig {
                    port: 8080,
                    device_number: 0,
                    discovery_port: None,
                    tls: None,
                    auth: None,
                },
            };

            let device = FileMonitorDevice::new(config);
            // Should never panic, even with arbitrary regex patterns
            let _result = device.evaluate_safety(&content);
        }
    }
}
