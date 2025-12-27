use ascom_alpaca::api::SafetyMonitor;
use ascom_alpaca::test::run_conformu_tests;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::{sleep, timeout};

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
            "port": 11113,
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

    // Wait for service to be ready with health check
    let client = reqwest::Client::new();
    let mut ready = false;

    for _ in 0..30 {
        sleep(Duration::from_secs(1)).await;

        if let Ok(response) = timeout(
            Duration::from_secs(2),
            client.get("http://localhost:11113/management/v1/description").send(),
        )
        .await
        {
            if let Ok(resp) = response {
                if resp.status().is_success() {
                    ready = true;
                    break;
                }
            }
        }
    }

    if !ready {
        child.kill().await.ok();
        std::fs::remove_dir_all(&test_dir).ok();
        return Err("Service failed to start within 30 seconds".into());
    }

    println!("::group::ConformU Compliance Test Results");
    
    // Run ConformU tests and capture result
    let result = run_conformu_tests::<dyn SafetyMonitor>("http://localhost:11113", 0).await;
    
    match &result {
        Ok(_) => {
            println!("✅ ConformU compliance tests PASSED");
            println!("All ASCOM Alpaca compliance requirements met");
        }
        Err(e) => {
            println!("❌ ConformU compliance tests FAILED");
            println!("Error: {}", e);
        }
    }
    
    println!("::endgroup::");

    // Cleanup
    child.kill().await.ok();
    std::fs::remove_dir_all(&test_dir).ok();

    result?;
    Ok(())
}
