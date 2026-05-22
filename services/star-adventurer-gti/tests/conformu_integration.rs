//! ConformU compliance tests for the Star Adventurer GTi driver.
//!
//! Run the ConformU ASCOM Telescope test suite against the driver running
//! in mock mode. Same shape as the qhy-focuser integration test.
#![cfg(feature = "conformu")]
#![allow(clippy::await_holding_lock)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]

use ascom_alpaca::api::telescope::Telescope;
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

    let test_dir = std::env::temp_dir().join("conformu_star_adventurer_gti_test");
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
        "TelescopeExtendedRateOffsetTests": false,
        "TelescopeFirstUseTests": false,
        "TestSideOfPierRead": true,
        "TestSideOfPierWrite": false,
        "CameraFirstUseTests": false,
        "CameraTestImageArrayVariant": false,
        "FocuserTimeout": 30
    });
    std::fs::write(
        &conformu_settings_path,
        serde_json::to_string_pretty(&conformu_settings)?,
    )?;

    // Mock-mode config: tagged USB transport with a placeholder port
    // (the binary is built with `--features mock`, so the
    // MockTransportFactory replaces both serial and UDP factories — the
    // device path doesn't matter).
    let config = serde_json::json!({
        "transport": {
            "kind": "usb",
            "port": "/dev/mock",
            "baud_rate": 115200,
            "command_timeout": "2s",
            "polling_interval": "200ms"
        },
        "server": {
            "port": 0
        },
        "mount": {
            "name": "ConformU Test Star Adventurer GTi",
            "unique_id": "conformu-star-adventurer-gti-001",
            "description": "Test mount for ConformU compliance",
            "enabled": true,
            "site_latitude_deg": 47.6062,
            "site_longitude_deg": -122.3321,
            "site_elevation_m": 56.0,
            "settle_after_slew": "200ms",
            "tracking_rate": "sidereal"
        }
    });

    std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;

    let mut handle = ServiceHandle::try_start(
        env!("CARGO_PKG_NAME"),
        config_path
            .to_str()
            .expect("conformu temp path must be UTF-8"),
    )
    .await?;

    println!("::group::ConformU Telescope Compliance Test Results");
    println!(
        "Running ASCOM Alpaca Telescope compliance tests on port {}...",
        handle.port
    );

    let result: Result<(), Box<dyn std::error::Error>> = async {
        let builder = ConformUTestBuilder::new::<dyn Telescope>(&handle.base_url, 0)?;
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
            println!("All ASCOM Alpaca Telescope compliance requirements met");
        }
        Err(e) => {
            println!("ConformU compliance tests FAILED");
            println!("Error: {e}");
        }
    }

    println!("::endgroup::");

    handle.stop().await;
    std::fs::remove_dir_all(&test_dir).ok();

    result?;
    Ok(())
}
