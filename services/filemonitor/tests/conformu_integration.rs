#![cfg(feature = "conformu")]

use ascom_alpaca::api::SafetyMonitor;
use ascom_alpaca::test::run_conformu_tests;
use bdd_infra::ServiceHandle;
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::test]
#[ignore] // Run with --ignored flag since it requires ConformU installation
async fn conformu_compliance_tests() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing to capture ConformU detailed output
    let _ = fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("ascom_alpaca::conformu=trace,info")),
        )
        .with_test_writer()
        .try_init();

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
            "polling_interval": "1s"
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
            "port": 0,
            "device_number": 0
        }
    });

    std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;
    std::fs::write(&status_file, "SAFE")?;

    let mut handle = ServiceHandle::start("filemonitor", config_path.to_str().unwrap()).await;

    println!("::group::ConformU Compliance Test Results");
    println!(
        "Running ASCOM Alpaca compliance tests on port {}...",
        handle.port
    );

    let result = run_conformu_tests::<dyn SafetyMonitor>(&handle.base_url, 0).await;

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

    handle.stop().await;
    std::fs::remove_dir_all(&test_dir).ok();

    result?;
    Ok(())
}
