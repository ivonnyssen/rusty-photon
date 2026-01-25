//! ConformU compliance tests for PPBA Switch driver
//!
//! These tests verify ASCOM Alpaca compliance by running the ConformU test suite
//! against the driver running in mock mode.

use ascom_alpaca::api::Switch;
use ascom_alpaca::test::run_conformu_tests;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::{sleep, timeout};
use tracing_subscriber::{fmt, EnvFilter};

fn get_random_port() -> u16 {
    use std::net::TcpListener;

    // Bind to port 0 to get a random available port
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind to random port");
    let port = listener
        .local_addr()
        .expect("Failed to get local addr")
        .port();
    drop(listener); // Release the port
    port
}

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
    let test_dir = std::env::temp_dir().join("conformu_ppba_test");
    std::fs::create_dir_all(&test_dir)?;

    let config_path = test_dir.join("config.json");
    let port = get_random_port();

    let config = serde_json::json!({
        "device": {
            "name": "ConformU Test PPBA",
            "unique_id": "conformu-ppba-001",
            "description": "Test PPBA Switch for ConformU compliance"
        },
        "serial": {
            "port": "/dev/mock",
            "baud_rate": 9600,
            "polling_interval_seconds": 60,
            "timeout_seconds": 2
        },
        "server": {
            "port": port,
            "device_number": 0
        }
    });

    std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;

    // Start ppba-switch service with mock feature
    let mut child = Command::new("cargo")
        .args([
            "run",
            "-p",
            "ppba-switch",
            "--features",
            "mock",
            "--",
            "-c",
            config_path.to_str().unwrap(),
        ])
        .spawn()?;

    // Wait for service to be ready with health check
    let client = reqwest::Client::new();
    let mut ready = false;

    for _ in 0..30 {
        sleep(Duration::from_secs(1)).await;

        if let Ok(Ok(resp)) = timeout(
            Duration::from_secs(2),
            client
                .get(format!(
                    "http://localhost:{}/management/v1/description",
                    port
                ))
                .send(),
        )
        .await
        {
            if resp.status().is_success() {
                ready = true;
                break;
            }
        }
    }

    if !ready {
        let _ = child.kill().await;
        let _ = child.wait().await;
        std::fs::remove_dir_all(&test_dir).ok();
        return Err("Service failed to start within 30 seconds".into());
    }

    println!("::group::ConformU Compliance Test Results");
    println!("Running ASCOM Alpaca Switch compliance tests...");

    // Run ConformU tests and capture result
    let result = run_conformu_tests::<dyn Switch>(&format!("http://localhost:{}", port), 0).await;

    match &result {
        Ok(_) => {
            println!("ConformU compliance tests PASSED");
            println!("All ASCOM Alpaca Switch compliance requirements met");
        }
        Err(e) => {
            println!("ConformU compliance tests FAILED");
            println!("Error: {}", e);
        }
    }

    println!("::endgroup::");

    // Cleanup - ensure process is properly terminated
    let _ = child.kill().await;
    let _ = child.wait().await;
    std::fs::remove_dir_all(&test_dir).ok();

    result?;
    Ok(())
}
