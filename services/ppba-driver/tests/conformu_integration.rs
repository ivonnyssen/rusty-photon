//! ConformU compliance tests for PPBA driver
//!
//! These tests verify ASCOM Alpaca compliance by running the ConformU test suite
//! against the driver running in mock mode.

use ascom_alpaca::api::{ObservingConditions, Switch};
use ascom_alpaca::test::conformu_tests;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::{sleep, timeout};
use tracing_subscriber::{fmt, EnvFilter};

// Static mutex to ensure conformu tests run sequentially
// Required because both tests bind the ASCOM Alpaca discovery service to a fixed address
static CONFORMU_LOCK: Mutex<()> = Mutex::new(());

/// Parse the bound port from service stdout
/// Looks for "Bound Alpaca server bound_addr=0.0.0.0:PORT"
async fn parse_bound_port(stdout: tokio::process::ChildStdout) -> Option<u16> {
    let mut reader = BufReader::new(stdout).lines();

    while let Ok(Some(line)) = reader.next_line().await {
        if let Some(addr_str) = line.strip_prefix("Bound Alpaca server bound_addr=") {
            if let Some(port_str) = addr_str.split(':').last() {
                if let Ok(port) = port_str.parse::<u16>() {
                    return Some(port);
                }
            }
        }
    }
    None
}

#[tokio::test]
#[ignore] // Run with --ignored flag since it requires ConformU installation
async fn conformu_compliance_tests() -> Result<(), Box<dyn std::error::Error>> {
    // Acquire lock to ensure tests run sequentially (discovery service conflict)
    let _lock = CONFORMU_LOCK.lock().unwrap();

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
    let conformu_settings_path = test_dir.join("conformu-settings.json");

    // Create ConformU settings with reduced delays for faster CI
    // Note: ConformU requires a complete settings file - partial files are ignored
    let conformu_settings = serde_json::json!({
        "SettingsCompatibilityVersion": 1,
        "GoHomeOnDeviceSelected": true,
        "ConnectionTimeout": 2,
        "RunAs32Bit": false,
        "RiskAcknowledged": false,
        "DisplayMethodCalls": false,
        "UpdateCheck": false,
        "ApplicationPort": 0,
        "ConnectDisconnectTimeout": 5,
        "Debug": false,
        "TraceDiscovery": false,
        "TraceAlpacaCalls": false,
        "TestProperties": true,
        "TestMethods": true,
        "TestPerformance": false,
        "AlpacaDevice": {},
        "AlpacaConfiguration": {},
        "ComDevice": {},
        "ComConfiguration": {},
        "DeviceName": "No device selected",
        "DeviceTechnology": "NotSelected",
        "ReportGoodTimings": true,
        "ReportBadTimings": true,
        "TelescopeTests": {},
        "TelescopeExtendedRateOffsetTests": true,
        "TelescopeFirstUseTests": true,
        "TestSideOfPierRead": false,
        "TestSideOfPierWrite": false,
        "CameraFirstUseTests": true,
        "CameraTestImageArrayVariant": true,
        // Switch-specific settings with reduced delays for faster CI
        // Default: SwitchReadDelay=500, SwitchWriteDelay=3000
        "SwitchEnableSet": false,
        "SwitchReadDelay": 50,
        "SwitchWriteDelay": 100,
        "SwitchExtendedNumberTestRange": 100,
        "SwitchAsyncTimeout": 10,
        "SwitchTestOffsets": true
    });
    std::fs::write(
        &conformu_settings_path,
        serde_json::to_string_pretty(&conformu_settings)?,
    )?;

    let config = serde_json::json!({
        "serial": {
            "port": "/dev/mock",
            "baud_rate": 9600,
            "polling_interval_ms": 60000,
            "timeout_seconds": 2
        },
        "server": {
            "port": 0
        },
        "switch": {
            "enabled": true,
            "name": "ConformU Test PPBA",
            "unique_id": "conformu-ppba-001",
            "description": "Test PPBA Switch for ConformU compliance",
            "device_number": 0
        },
        "observingconditions": {
            "enabled": false,
            "name": "ConformU Test PPBA Weather",
            "unique_id": "conformu-ppba-weather-001",
            "description": "Test PPBA ObservingConditions",
            "device_number": 0
        }
    });

    std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;

    // Start ppba-driver service with mock feature, capturing stdout to parse bound port
    let mut child = Command::new("cargo")
        .args([
            "run",
            "-p",
            "ppba-driver",
            "--features",
            "mock",
            "--",
            "-c",
            config_path.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    // Parse the bound port from stdout
    let stdout = child.stdout.take().ok_or("Failed to capture stdout")?;
    let port = parse_bound_port(stdout)
        .await
        .ok_or("Failed to parse bound port from service output")?;

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

    // Run ConformU tests with reduced delays for faster CI
    let result = conformu_tests::<dyn Switch>(&format!("http://localhost:{}", port), 0)?
        .settings_file(&conformu_settings_path)
        .run()
        .await;

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

#[tokio::test]
#[ignore] // Run with --ignored flag since it requires ConformU installation
async fn conformu_compliance_tests_observingconditions() -> Result<(), Box<dyn std::error::Error>> {
    // Acquire lock to ensure tests run sequentially (discovery service conflict)
    let _lock = CONFORMU_LOCK.lock().unwrap();

    // Initialize tracing to capture ConformU detailed output
    let _ = fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("ascom_alpaca::conformu=trace,info")),
        )
        .with_test_writer()
        .try_init();

    // Create test config
    let test_dir = std::env::temp_dir().join("conformu_ppba_oc_test");
    std::fs::create_dir_all(&test_dir)?;

    let config_path = test_dir.join("config.json");
    let conformu_settings_path = test_dir.join("conformu-settings.json");

    // Create ConformU settings with reduced delays for faster CI
    let conformu_settings = serde_json::json!({
        "SettingsCompatibilityVersion": 1,
        "GoHomeOnDeviceSelected": true,
        "ConnectionTimeout": 2,
        "RunAs32Bit": false,
        "RiskAcknowledged": false,
        "DisplayMethodCalls": false,
        "UpdateCheck": false,
        "ApplicationPort": 0,
        "ConnectDisconnectTimeout": 5,
        "Debug": false,
        "TraceDiscovery": false,
        "TraceAlpacaCalls": false,
        "TestProperties": true,
        "TestMethods": true,
        "TestPerformance": false,
        "AlpacaDevice": {},
        "AlpacaConfiguration": {},
        "ComDevice": {},
        "ComConfiguration": {},
        "DeviceName": "No device selected",
        "DeviceTechnology": "NotSelected",
        "ReportGoodTimings": true,
        "ReportBadTimings": true,
        "TelescopeTests": {},
        "TelescopeExtendedRateOffsetTests": true,
        "TelescopeFirstUseTests": true,
        "TestSideOfPierRead": false,
        "TestSideOfPierWrite": false,
        "CameraFirstUseTests": true,
        "CameraTestImageArrayVariant": true,
        // ObservingConditions-specific settings
        "ObservingConditionsNumReadings": 5,
        "ObservingConditionsReadInterval": 50
    });
    std::fs::write(
        &conformu_settings_path,
        serde_json::to_string_pretty(&conformu_settings)?,
    )?;

    let config = serde_json::json!({
        "serial": {
            "port": "/dev/mock",
            "baud_rate": 9600,
            "polling_interval_ms": 60000,
            "timeout_seconds": 2
        },
        "server": {
            "port": 0
        },
        "switch": {
            "enabled": false,
            "name": "ConformU Test PPBA Switch",
            "unique_id": "conformu-ppba-switch-001",
            "description": "Test PPBA Switch",
            "device_number": 0
        },
        "observingconditions": {
            "enabled": true,
            "name": "ConformU Test PPBA Weather",
            "unique_id": "conformu-ppba-weather-001",
            "description": "Test PPBA ObservingConditions for ConformU compliance",
            "device_number": 0
        }
    });

    std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;

    // Start ppba-driver service with mock feature, capturing stdout to parse bound port
    let mut child = Command::new("cargo")
        .args([
            "run",
            "-p",
            "ppba-driver",
            "--features",
            "mock",
            "--",
            "-c",
            config_path.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    // Parse the bound port from stdout
    let stdout = child.stdout.take().ok_or("Failed to capture stdout")?;
    let port = parse_bound_port(stdout)
        .await
        .ok_or("Failed to parse bound port from service output")?;

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

    println!("::group::ConformU ObservingConditions Compliance Test Results");
    println!("Running ASCOM Alpaca ObservingConditions compliance tests...");

    // Run ConformU tests
    let result =
        conformu_tests::<dyn ObservingConditions>(&format!("http://localhost:{}", port), 0)?
            .settings_file(&conformu_settings_path)
            .run()
            .await;

    match &result {
        Ok(_) => {
            println!("ConformU ObservingConditions compliance tests PASSED");
            println!("All ASCOM Alpaca ObservingConditions compliance requirements met");
        }
        Err(e) => {
            println!("ConformU ObservingConditions compliance tests FAILED");
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
