use filemonitor::{Config, DeviceConfig, FileConfig, ParsingConfig, ServerConfig};
use std::path::PathBuf;

#[tokio::test]
#[cfg(not(miri))]
async fn test_start_server_creation() {
    use filemonitor::start_server;
    use std::time::Duration;
    use tokio::time::timeout;

    let config = Config {
        device: DeviceConfig {
            name: "Test Server".to_string(),
            unique_id: "test-server-001".to_string(),
            description: "Test server device".to_string(),
        },
        file: FileConfig {
            path: PathBuf::from("test_server_file.txt"),
            polling_interval_seconds: 1,
        },
        parsing: ParsingConfig {
            rules: vec![],
            case_sensitive: false,
        },
        server: ServerConfig {
            port: 0,
            device_number: 0,
        },
    };

    std::fs::write(&config.file.path, "test").unwrap();

    let server_future = start_server(config.clone());
    let result = timeout(Duration::from_millis(100), server_future).await;

    std::fs::remove_file(&config.file.path).unwrap();

    // We expect timeout since server.start() would block indefinitely
    assert!(result.is_err());
}
