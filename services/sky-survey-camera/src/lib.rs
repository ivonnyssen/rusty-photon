#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! sky-survey-camera: ASCOM Alpaca Camera simulator backed by NASA SkyView.

pub mod camera;
pub mod config;
pub mod error;
pub mod fits;
#[cfg(feature = "mock")]
pub mod mock;
pub mod pointing;
pub mod routes;
pub mod survey;

pub use config::{load_config, Config};
pub use error::SkySurveyCameraError;
#[cfg(feature = "mock")]
pub use mock::MockSurveyClient;
pub use survey::SurveyClient;

use ascom_alpaca::api::CargoServerInfo;
use ascom_alpaca::Server;
use std::path::Path;
use std::sync::Arc;

/// Bind the ASCOM Alpaca Camera server (with the `/sky-survey/*`
/// custom routes composed in front of it), print `bound_addr=` for
/// the BDD harness, and serve forever.
pub async fn run(config_path: &Path) -> Result<(), SkySurveyCameraError> {
    let config = load_config(config_path).await?;
    let survey_client = build_survey_client(&config)?;
    run_with_client(config, survey_client).await
}

/// Variant of [`run`] used by tests / the `mock` feature where the
/// caller has already constructed an [`Arc<dyn SurveyClient>`].
pub async fn run_with_client(
    config: Config,
    survey_client: Arc<dyn SurveyClient>,
) -> Result<(), SkySurveyCameraError> {
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
        .map_err(|e| {
            SkySurveyCameraError::Bind(format!("bind 0.0.0.0:{}: {e}", config.server.port))
        })?;
    let local = listener
        .local_addr()
        .map_err(|e| SkySurveyCameraError::Bind(format!("local_addr: {e}")))?;
    println!("bound_addr={local}");
    tracing::info!(address = %local, "sky-survey-camera serving");

    // Graceful shutdown on Ctrl+C / SIGTERM. Required so coverage
    // profraw files flush when bdd-infra's ServiceHandle sends
    // SIGTERM at the end of each scenario.
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| SkySurveyCameraError::Server(e.to_string()))?;
    Ok(())
}

/// Construct the production [`SurveyClient`] from config. The `mock`
/// feature does NOT short-circuit this — production binaries always
/// hit the configured endpoint via [`survey::SkyViewClient`]. The
/// in-process [`mock::MockSurveyClient`] is exposed as a library
/// type so the conformu integration test can call [`run_with_client`]
/// directly with a synthetic backend; switching the binary itself
/// would break BDD scenarios that exercise the real HTTP error
/// paths against a stub server.
fn build_survey_client(config: &Config) -> Result<Arc<dyn SurveyClient>, SkySurveyCameraError> {
    let client = survey::SkyViewClient::new(&config.survey)
        .map_err(|e| SkySurveyCameraError::Server(e.to_string()))?;
    Ok(Arc::new(client))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::debug!("received Ctrl+C"),
        () = terminate => tracing::debug!("received SIGTERM"),
    }
}
