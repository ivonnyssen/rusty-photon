#![cfg(feature = "conformu")]

use ascom_alpaca::api::SafetyMonitor;
use ascom_alpaca::test::run_conformu_tests;
use std::process::Stdio;
use tokio::process::Command;
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
            "port": 0,
            "device_number": 0
        }
    });

    std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;
    std::fs::write(&status_file, "SAFE")?;

    // Start filemonitor service, capturing stdout to parse bound port
    let mut child = Command::new("cargo")
        .args(["run", "--", "-c", config_path.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    // Parse the bound port from stdout - the server is ready once this message appears
    // since the socket is already listening after bind()
    let stdout = child.stdout.take().ok_or("Failed to capture stdout")?;
    let (port, stdout_drain) = bdd_infra::parse_bound_port(stdout)
        .await
        .ok_or("Failed to parse bound port from service output")?;

    println!("::group::ConformU Compliance Test Results");
    println!("Running ASCOM Alpaca compliance tests on port {}...", port);

    // Run ConformU tests and capture result
    let result =
        run_conformu_tests::<dyn SafetyMonitor>(&format!("http://localhost:{}", port), 0).await;

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

    // Cleanup - ensure process is properly terminated
    let _ = child.kill().await;
    let _ = child.wait().await;
    stdout_drain.abort();
    std::fs::remove_dir_all(&test_dir).ok();

    result?;
    Ok(())
}
