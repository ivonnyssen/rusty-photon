use ascom_alpaca::api::Device;
use filemonitor::{
    Config, DeviceConfig, FileConfig, FileMonitorDevice, ParsingConfig, ParsingRule, RuleType,
    ServerConfig,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

fn create_test_config() -> Config {
    Config {
        device: DeviceConfig {
            name: "Test Device".to_string(),
            unique_id: "test-123".to_string(),
            description: "Test Description".to_string(),
        },
        file: FileConfig {
            path: PathBuf::from("/tmp/test.txt"),
            polling_interval_seconds: 1,
        },
        parsing: ParsingConfig {
            rules: vec![ParsingRule {
                rule_type: RuleType::Contains,
                pattern: "SAFE".to_string(),
                safe: true,
            }],
            default_safe: false,
            case_sensitive: true,
        },
        server: ServerConfig {
            port: 8080,
            device_number: 0,
        },
    }
}

#[tokio::test]
#[cfg(not(miri))]
async fn test_concurrent_connection_state_changes() {
    let config = create_test_config();
    let device = Arc::new(FileMonitorDevice::new(config));

    let device1 = Arc::clone(&device);
    let device2 = Arc::clone(&device);

    // Simulate rapid connection state changes
    let t1 = tokio::spawn(async move {
        for i in 0..10 {
            let connected = i % 2 == 0;
            let _ = device1.set_connected(connected).await;
            sleep(Duration::from_millis(1)).await;
        }
    });

    let t2 = tokio::spawn(async move {
        for _ in 0..10 {
            let _ = device2.connected().await;
            sleep(Duration::from_millis(1)).await;
        }
    });

    let _ = tokio::join!(t1, t2);
}

#[tokio::test]
async fn test_concurrent_safety_checks() {
    let config = create_test_config();
    let device = Arc::new(FileMonitorDevice::new(config));

    // Don't connect - just test safety evaluation which doesn't require connection
    let handles: Vec<_> = (0..5)
        .map(|i| {
            let device = Arc::clone(&device);
            tokio::spawn(async move {
                let content = if i % 2 == 0 {
                    "SAFE operation"
                } else {
                    "unsafe operation"
                };
                for _ in 0..10 {
                    let result = device.evaluate_safety(content);
                    if content.contains("SAFE") {
                        assert!(result);
                    } else {
                        assert!(!result);
                    }
                    sleep(Duration::from_millis(1)).await;
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.unwrap();
    }
}

#[tokio::test]
#[cfg(not(miri))]
async fn test_stress_concurrent_operations() {
    let config = create_test_config();
    let device = Arc::new(FileMonitorDevice::new(config));

    let handles: Vec<_> = (0..20)
        .map(|i| {
            let device = Arc::clone(&device);
            tokio::spawn(async move {
                match i % 3 {
                    0 => {
                        // Connection operations
                        let _ = device.set_connected(true).await;
                        let _ = device.connected().await;
                    }
                    1 => {
                        // Safety evaluations
                        let _ = device.evaluate_safety("test content");
                    }
                    _ => {
                        // Device info operations
                        let _ = device.description().await;
                        let _ = device.driver_version().await;
                    }
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.unwrap();
    }
}
