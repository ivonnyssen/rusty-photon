//! ConformU compliance tests for sky-survey-camera.
//!
//! Verifies ICameraV3 conformance against the simulator running with
//! the configured SkyView endpoint pointed at an in-process stub HTTP
//! server. The stub serves a minimal FITS payload synthesised by
//! [`MockSurveyClient`]'s helper so ConformU can drive
//! `StartExposure` / `ImageArray` end-to-end without touching NASA.
#![cfg(feature = "conformu")]
#![allow(clippy::await_holding_lock)]

use ascom_alpaca::api::Camera;
use ascom_alpaca::test::run_conformu_tests;
use bdd_infra::ServiceHandle;
use sky_survey_camera::mock::synthetic_fits;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use tempfile::TempDir;
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

    let temp_dir = TempDir::new()?;
    let cache_dir = temp_dir.path().join("cache");
    std::fs::create_dir_all(&cache_dir)?;

    let stub_url = spawn_stub_skyview().await?;
    let config_path = temp_dir.path().join("config.json");
    let config = serde_json::json!({
        "device": {
            "name": "ConformU Sky Survey Camera",
            "unique_id": "conformu-sky-survey-camera-001",
            "description": "Sky-survey-camera ConformU compliance instance",
        },
        "optics": {
            "focal_length_mm": 1000.0,
            "pixel_size_x_um": 3.76,
            "pixel_size_y_um": 3.76,
            // Small sensor keeps StartExposure rounds quick — ConformU
            // exercises full-frame readouts at 1×–4× binning.
            "sensor_width_px": 64,
            "sensor_height_px": 48,
        },
        "pointing": {
            "initial_ra_deg": 0.0,
            "initial_dec_deg": 0.0,
            "initial_rotation_deg": 0.0,
        },
        "survey": {
            "name": "DSS2 Red",
            "request_timeout": "5s",
            "cache_dir": cache_dir.to_string_lossy(),
            "endpoint": stub_url,
        },
        "server": { "port": 0 },
    });
    std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;

    // The `conformu` feature transitively pulls in `mock`, which
    // exposes the `synthetic_fits` helper used by the in-process stub
    // below. The binary itself always uses `SkyViewClient` pointed at
    // the stub URL — no runtime mock swap.
    let mut handle = ServiceHandle::try_start(
        env!("CARGO_PKG_NAME"),
        config_path
            .to_str()
            .expect("conformu temp path must be UTF-8"),
    )
    .await?;

    println!("::group::ConformU Compliance Test Results");
    println!(
        "Running ASCOM Alpaca Camera compliance tests on port {}...",
        handle.port
    );

    let result = run_conformu_tests::<dyn Camera>(&handle.base_url, 0).await;

    match &result {
        Ok(_) => {
            println!("ConformU compliance tests PASSED");
            println!("All ASCOM Alpaca Camera compliance requirements met");
        }
        Err(e) => {
            println!("ConformU compliance tests FAILED");
            println!("Error: {}", e);
        }
    }

    println!("::endgroup::");

    handle.stop().await;
    let _ = temp_dir.close();

    result?;
    Ok(())
}

/// Spawn a tiny axum stub on `127.0.0.1:0` that responds to HEAD with
/// 200 and to GET with a synthetic FITS cutout of the requested
/// `Pixels`. Returns the URL the camera should use as
/// `survey.endpoint`.
async fn spawn_stub_skyview() -> Result<String, Box<dyn std::error::Error>> {
    use axum::extract::Query;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::any;
    use axum::Router;

    #[derive(serde::Deserialize)]
    struct Params {
        #[serde(rename = "Pixels")]
        pixels: Option<String>,
    }

    let counter: std::sync::Arc<AtomicU32> = std::sync::Arc::new(AtomicU32::new(0));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    let app = Router::new().fallback(any(
        move |method: axum::http::Method, params: Query<Params>| {
            let counter = std::sync::Arc::clone(&counter);
            async move {
                if method == axum::http::Method::HEAD {
                    return (StatusCode::OK, Vec::<u8>::new()).into_response();
                }
                counter.fetch_add(1, Ordering::SeqCst);
                let (w, h) = parse_pixels(params.pixels.as_deref()).unwrap_or((64, 48));
                (StatusCode::OK, synthetic_fits(w, h)).into_response()
            }
        },
    ));

    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    Ok(format!("http://{addr}/"))
}

fn parse_pixels(s: Option<&str>) -> Option<(u32, u32)> {
    let raw = s?;
    let mut parts = raw.split(',');
    let w = parts.next()?.trim().parse::<u32>().ok()?;
    let h = parts.next()?.trim().parse::<u32>().ok()?;
    Some((w, h))
}
