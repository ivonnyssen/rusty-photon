//! ConformU compliance tests for QHY Q-Focuser driver
//!
//! These tests verify ASCOM Alpaca compliance by running the ConformU test suite
//! against the driver running in mock mode.
#![allow(clippy::await_holding_lock)]

use ascom_alpaca::api::Focuser;
use ascom_alpaca::test::conformu_tests;
use std::process::Stdio;
use std::sync::Mutex;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing_subscriber::{fmt, EnvFilter};

static CONFORMU_LOCK: Mutex<()> = Mutex::new(());

/// Parse the bound port from service stdout.
async fn parse_bound_port(
    stdout: tokio::process::ChildStdout,
) -> Option<(u16, tokio::task::JoinHandle<()>)> {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();

    while reader.read_line(&mut line).await.ok()? > 0 {
        if let Some(addr_str) = line.trim().strip_prefix("Bound Alpaca server bound_addr=") {
            if let Some(port_str) = addr_str.split(':').next_back() {
                if let Ok(port) = port_str.parse::<u16>() {
                    let drain_handle = tokio::spawn(async move {
                        let mut buf = String::new();
                        while reader.read_line(&mut buf).await.unwrap_or(0) > 0 {
                            buf.clear();
                        }
                    });
                    return Some((port, drain_handle));
                }
            }
        }
        line.clear();
    }
    None
}

#[tokio::test]
#[ignore] // Run with --ignored flag since it requires ConformU installation
async fn conformu_compliance_tests() -> Result<(), Box<dyn std::error::Error>> {
    let _lock = CONFORMU_LOCK.lock().unwrap();

    let _ = fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("ascom_alpaca::conformu=trace,info")),
        )
        .with_test_writer()
        .try_init();

    let test_dir = std::env::temp_dir().join("conformu_qhy_focuser_test");
    std::fs::create_dir_all(&test_dir)?;

    let config_path = test_dir.join("config.json");
    let conformu_settings_path = test_dir.join("conformu-settings.json");

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
        "FocuserTimeout": 30
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
        "focuser": {
            "enabled": true,
            "name": "ConformU Test QHY Focuser",
            "unique_id": "conformu-qhy-focuser-001",
            "description": "Test QHY Q-Focuser for ConformU compliance",
            "device_number": 0,
            "max_step": 64000,
            "speed": 0,
            "reverse": false
        }
    });

    std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;

    let mut child = Command::new("cargo")
        .args([
            "run",
            "-p",
            "qhy-focuser",
            "--features",
            "mock",
            "--",
            "-c",
            config_path.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    let stdout = child.stdout.take().ok_or("Failed to capture stdout")?;
    let (port, stdout_drain) = parse_bound_port(stdout)
        .await
        .ok_or("Failed to parse bound port from service output")?;

    println!("::group::ConformU Focuser Compliance Test Results");
    println!(
        "Running ASCOM Alpaca Focuser compliance tests on port {}...",
        port
    );

    let result = conformu_tests::<dyn Focuser>(&format!("http://localhost:{}", port), 0)?
        .settings_file(&conformu_settings_path)
        .run()
        .await;

    match &result {
        Ok(_) => {
            println!("ConformU compliance tests PASSED");
            println!("All ASCOM Alpaca Focuser compliance requirements met");
        }
        Err(e) => {
            println!("ConformU compliance tests FAILED");
            println!("Error: {}", e);
        }
    }

    println!("::endgroup::");

    let _ = child.kill().await;
    let _ = child.wait().await;
    stdout_drain.abort();
    std::fs::remove_dir_all(&test_dir).ok();

    result?;
    Ok(())
}
