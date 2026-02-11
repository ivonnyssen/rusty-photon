use cucumber::World;
use filemonitor::{
    Config, DeviceConfig, FileConfig, FileMonitorDevice, ParsingConfig, ParsingRule, ServerConfig,
};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

#[derive(Debug, Default, World)]
pub struct FilemonitorWorld {
    pub config: Option<Config>,
    pub device: Option<Arc<FileMonitorDevice>>,
    pub temp_dir: Option<TempDir>,
    pub temp_file_path: Option<PathBuf>,
    pub rules: Vec<ParsingRule>,
    pub case_sensitive: bool,
    pub safety_result: Option<bool>,
    pub last_error: Option<String>,
    pub polling_interval: u64,
}

impl FilemonitorWorld {
    pub fn create_temp_file(&mut self, content: &str) -> PathBuf {
        let dir = self
            .temp_dir
            .get_or_insert_with(|| TempDir::new().expect("failed to create temp dir"));
        let path = dir.path().join("monitored.txt");
        std::fs::write(&path, content).expect("failed to write temp file");
        self.temp_file_path = Some(path.clone());
        path
    }

    pub fn build_config(&self, file_path: PathBuf) -> Config {
        Config {
            device: DeviceConfig {
                name: "Test".to_string(),
                unique_id: "test-001".to_string(),
                description: "Test device".to_string(),
            },
            file: FileConfig {
                path: file_path,
                polling_interval_seconds: if self.polling_interval > 0 {
                    self.polling_interval
                } else {
                    60
                },
            },
            parsing: ParsingConfig {
                rules: self.rules.clone(),
                case_sensitive: self.case_sensitive,
            },
            server: ServerConfig {
                port: 0,
                device_number: 0,
            },
        }
    }

    pub fn build_device(&mut self) {
        let path = self
            .temp_file_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("nonexistent.txt"));
        let config = self.build_config(path);
        self.device = Some(Arc::new(FileMonitorDevice::new(config)));
    }
}
