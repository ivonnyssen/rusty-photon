use ascom_alpaca::api::SafetyMonitor;
use ascom_alpaca::test::run_conformu_tests;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::sleep;

#[tokio::test]
#[ignore] // Run with --ignored flag since it requires ConformU installation
async fn conformu_compliance_tests() -> Result<(), Box<dyn std::error::Error>> {
    // Create test config
    let test_dir = std::env::temp_dir().join("conformu_test");
    std::fs::create_dir_all(&test_dir)?;

    let config_path = test_dir.join("config.json");
    let status_file = test_dir.join("status.txt");

    let config = serde_json::json!({
        "device": {
            "name": "ConformU Test Monitor",
            "unique_id": "conformu-test-001",
            "description": "Test SafetyMonitor for ConformU compliance"
        },
        "file": {
            "path": status_file,
            "polling_interval_seconds": 1
        },
        "parsing": {
            "rules": [
                {
                    "type": "contains",
                    "pattern": "SAFE",
                    "safe": true
                }
            ],
            "default_safe": false,
            "case_sensitive": false
        },
        "server": {
            "port": 11112,
            "device_number": 0
        }
    });

    std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;
    std::fs::write(&status_file, "SAFE")?;

    // Start filemonitor service
    let mut child = Command::new("cargo")
        .args(&["run", "--", "-c", config_path.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    // Wait for service to start
    sleep(Duration::from_secs(3)).await;

    // Run ConformU tests
    let result = run_conformu_tests::<dyn SafetyMonitor>("http://localhost:11112", 0).await;

    // Cleanup
    child.kill().await.ok();
    std::fs::remove_dir_all(&test_dir).ok();

    result?;
    Ok(())
}
