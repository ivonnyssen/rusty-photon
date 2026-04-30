//! ConformU compliance tests for QHY Q-Focuser driver
//!
//! These tests verify ASCOM Alpaca compliance by running the ConformU test suite
//! against the driver running in mock mode.
#![cfg(feature = "conformu")]
#![allow(clippy::await_holding_lock)]

use ascom_alpaca::api::Focuser;
use ascom_alpaca::test::ConformUTestBuilder;
use bdd_infra::ServiceHandle;
use std::sync::Mutex;
use tracing_subscriber::{fmt, EnvFilter};

static CONFORMU_LOCK: Mutex<()> = Mutex::new(());

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
            "polling_interval": "60s",
            "timeout": "2s"
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

    // Pre-built qhy-focuser binary must include the `mock` feature
    // (CI builds with --all-features); the binary is launched with the
    // mock serial port driving /dev/mock from the config.
    let mut handle = ServiceHandle::try_start(
        env!("CARGO_PKG_NAME"),
        config_path
            .to_str()
            .expect("conformu temp path must be UTF-8"),
    )
    .await?;

    println!("::group::ConformU Focuser Compliance Test Results");
    println!(
        "Running ASCOM Alpaca Focuser compliance tests on port {}...",
        handle.port
    );

    // Capture both builder-construction and run-time errors so `handle.stop()`
    // below is unconditional and the service gets a graceful SIGTERM with a
    // chance to flush coverage data.
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let builder = ConformUTestBuilder::new::<dyn Focuser>(&handle.base_url, 0)?;
        builder
            .settings_file(&conformu_settings_path)
            .run()
            .await
            .map_err(Into::into)
    }
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

    handle.stop().await;
    std::fs::remove_dir_all(&test_dir).ok();

    result?;
    Ok(())
}
