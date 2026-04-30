#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! sky-survey-camera: ASCOM Alpaca Camera simulator backed by NASA SkyView.

pub mod camera;
pub mod config;
pub mod error;
pub mod fits;
pub mod pointing;
pub mod routes;
pub mod survey;

pub use config::{load_config, Config};
pub use error::SkySurveyCameraError;

use ascom_alpaca::api::CargoServerInfo;
use ascom_alpaca::Server;
use std::path::Path;

/// Bind the ASCOM Alpaca Camera server (with the `/sky-survey/*`
/// custom routes composed in front of it), print `bound_addr=` for
/// the BDD harness, and serve forever.
pub async fn run(config_path: &Path) -> Result<(), SkySurveyCameraError> {
    let config = load_config(config_path).await?;

    let survey_client = std::sync::Arc::new(
        survey::SkyViewClient::new(&config.survey)
            .map_err(|e| SkySurveyCameraError::Server(e.to_string()))?,
    );
    let device = camera::SkySurveyCamera::new(config.clone(), survey_client);
    let shared_state = device.shared_state();

    let mut server = Server::new(CargoServerInfo!());
    server.listen_addr = std::net::SocketAddr::from(([0, 0, 0, 0], config.server.port));
    // Disable the UDP discovery server in v0 — BDD tests bind to
    // ephemeral ports and don't need discovery, and the production
    // deployment story (systemd / Windows service) doesn't require it
    // either.
    server.discovery_port = None;
    server.devices.register(device);

    let alpaca_service = server.into_service();
    let app = routes::build_router(shared_state).fallback_service(alpaca_service);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", config.server.port))
        .await
        .map_err(SkySurveyCameraError::ConfigIo)?;
    let local = listener
        .local_addr()
        .map_err(SkySurveyCameraError::ConfigIo)?;
    println!("bound_addr={local}");
    tracing::info!(address = %local, "sky-survey-camera serving");

    axum::serve(listener, app)
        .await
        .map_err(|e| SkySurveyCameraError::Server(e.to_string()))?;
    Ok(())
}
