use ascom_alpaca::api::{
    Camera, CoverCalibrator, Dome, FilterWheel, Focuser, ImageArray, ObservingConditions, Rotator,
    SafetyMonitor, Switch, Telescope, TypedDevice,
};
use ascom_alpaca::Client;
use axum::debug_handler;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Json;
use axum::{routing::get, Router};
use tracing_subscriber::FmtSubscriber;

use std::collections::HashSet;

use std::net::SocketAddr;
use std::sync::Arc;

use tracing::{debug, error, trace, Level};

#[derive(Debug)]
struct AppState {
    cameras: HashSet<Arc<dyn Camera>>,
    cover_calibrators: HashSet<Arc<dyn CoverCalibrator>>,
    domes: HashSet<Arc<dyn Dome>>,
    filter_wheels: HashSet<Arc<dyn FilterWheel>>,
    focusers: HashSet<Arc<dyn Focuser>>,
    observing_conditions: HashSet<Arc<dyn ObservingConditions>>,
    rotators: HashSet<Arc<dyn Rotator>>,
    safety_monitors: HashSet<Arc<dyn SafetyMonitor>>,
    switches: HashSet<Arc<dyn Switch>>,
    telescopes: HashSet<Arc<dyn Telescope>>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct AlpacaCamera {
    id: String,
    name: String,
    connected: bool,
    description: String,
    driver_info: String,
    driver_version: String,
}

impl AlpacaCamera {
    async fn from_camera(camera: &Arc<dyn Camera>) -> Self {
        Self {
            id: camera.unique_id().to_string(),
            name: camera.static_name().to_string(),
            connected: camera.connected().await.unwrap_or_default(),
            description: camera.description().await.unwrap_or_default(),
            driver_info: camera.driver_info().await.unwrap_or_default(),
            driver_version: camera.driver_version().await.unwrap_or_default(),
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Sequence {
    target_ccd_temperature: f64,
    target_bin_x: i32,
    target_bin_y: i32,
    target_exposure_time: f64,
}

#[tokio::main]
async fn main() {
    // initialize tracing
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::TRACE)
        .with_test_writer()
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    // create a client
    let client = match Client::new("http://localhost:8000") {
        Ok(client) => client,
        Err(e) => {
            error!("Error: {}", e);
            return;
        }
    };

    let mut cameras = HashSet::new();
    let mut cover_calibrators = HashSet::new();
    let mut domes = HashSet::new();
    let mut filter_wheels = HashSet::new();
    let mut focusers = HashSet::new();
    let mut observing_conditions = HashSet::new();
    let mut rotators = HashSet::new();
    let mut safety_monitors = HashSet::new();
    let mut switches = HashSet::new();
    let mut telescopes = HashSet::new();

    match client.get_devices().await {
        Ok(devices) => devices.for_each(|device| match device {
            TypedDevice::Camera(camera) => {
                cameras.insert(camera);
            }
            TypedDevice::CoverCalibrator(cover_calibrator) => {
                cover_calibrators.insert(cover_calibrator);
            }
            TypedDevice::Dome(dome) => {
                domes.insert(dome);
            }
            TypedDevice::FilterWheel(filter_wheel) => {
                filter_wheels.insert(filter_wheel);
            }
            TypedDevice::Focuser(focuser) => {
                focusers.insert(focuser);
            }
            TypedDevice::ObservingConditions(observing_condition) => {
                observing_conditions.insert(observing_condition);
            }
            TypedDevice::Rotator(rotator) => {
                rotators.insert(rotator);
            }
            TypedDevice::SafetyMonitor(safety_monitor) => {
                safety_monitors.insert(safety_monitor);
            }
            TypedDevice::Switch(switch) => {
                switches.insert(switch);
            }
            TypedDevice::Telescope(telescope) => {
                telescopes.insert(telescope);
            }
        }),
        Err(e) => {
            error!("Error: {}", e);
            return;
        }
    };

    let state = AppState {
        cameras,
        cover_calibrators,
        domes,
        filter_wheels,
        focusers,
        observing_conditions,
        rotators,
        safety_monitors,
        switches,
        telescopes,
    };

    let app = Router::new()
        .route("/", get(root))
        .route("/cameras", get(get_cameras))
        .route("/cameras/:id", get(get_camera_by_id))
        .route("/cameras/:id/run_sequence", post(run_sequence))
        .route("/cover_calibrators", get(get_cover_calibrators))
        .route("/cover_calibrators/:id", get(get_cover_calibrator_by_id))
        .route("/domes", get(get_domes))
        .route("/domes/:id", get(get_dome_by_id))
        .route("/filter_wheels", get(get_filter_wheels))
        .route("/filter_wheels/:id", get(get_filter_wheel_by_id))
        .route("/focusers", get(get_focusers))
        .route("/focusers/:id", get(get_focuser_by_id))
        .route("/observing_conditions", get(get_observing_conditions))
        .route(
            "/observing_conditions/:id",
            get(get_observing_condition_by_id),
        )
        .route("/rotators", get(get_rotators))
        .route("/rotators/:id", get(get_rotator_by_id))
        .route("/safety_monitors", get(get_safety_monitors))
        .route("/safety_monitors/:id", get(get_safety_monitor_by_id))
        .route("/switches", get(get_switches))
        .route("/switches/:id", get(get_switch_by_id))
        .route("/telescopes", get(get_telescopes))
        .route("/telescopes/:id", get(get_telescope_by_id))
        .with_state(Arc::new(state));

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    tracing::debug!("listening on {}", addr);
    axum::serve(
        tokio::net::TcpListener::bind(addr).await.unwrap(),
        app.into_make_service(),
    )
    .await
    .unwrap();
}

async fn root(State(state): State<Arc<AppState>>) -> String {
    format!("{:?}", state)
}

async fn get_cameras(State(state): State<Arc<AppState>>) -> String {
    let mut acc = String::new();
    for camera in state.cameras.iter() {
        acc.push_str(format!("{:?}", camera).as_str())
    }
    acc
}

async fn get_camera_by_id(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match state.cameras.iter().find(|camera| camera.unique_id() == id) {
        Some(camera) => {
            let res = AlpacaCamera::from_camera(camera).await;
            (StatusCode::OK, Json(res)).into_response()
        }
        None => (StatusCode::NOT_FOUND, String::from("Camera not found")).into_response(),
    }
}
#[debug_handler]
async fn run_sequence(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(sequence): Json<Sequence>,
) -> impl IntoResponse {
    let camera = match state.cameras.iter().find(|camera| camera.unique_id() == id) {
        Some(camera) => camera.clone(),
        None => return (StatusCode::NOT_FOUND, String::from("Camera not found")).into_response(),
    };
    tokio::spawn(async move {
        trace!("Connecting camera");
        camera.set_connected(true).await.unwrap_or_default();
        trace!("Connecting camera");
        camera.set_connected(true).await.unwrap_or_default();
        trace!("Setting cooler on");
        camera
            .set_cooler_on(true)
            .await
            .expect("Failed to set cooler on");
        trace!("Setting binning");
        camera
            .set_bin_x(sequence.target_bin_x)
            .await
            .expect("Failed to set bin x");
        camera
            .set_bin_y(sequence.target_bin_y)
            .await
            .expect("Failed to set bin y");
        trace!("Starting exposure");
        camera
            .start_exposure(sequence.target_exposure_time, true)
            .await
            .expect("failed to start exposure");
        while !camera
            .image_ready()
            .await
            .expect("failed to get image ready")
        {
            trace!("Waiting for exposure");
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        trace!("image ready");
    });
    (StatusCode::OK, Json("Done")).into_response()
}

async fn get_cover_calibrators(State(state): State<Arc<AppState>>) -> String {
    let mut acc = String::new();
    for cover_calibrator in state.cover_calibrators.iter() {
        acc.push_str(format!("{:?}", cover_calibrator).as_str())
    }
    acc
}

async fn get_cover_calibrator_by_id(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> (StatusCode, String) {
    let cover_calibrator = match state
        .cover_calibrators
        .iter()
        .find(|cover_calibrator| cover_calibrator.unique_id() == id)
    {
        Some(cover_calibrator) => cover_calibrator,
        None => {
            return (
                StatusCode::NOT_FOUND,
                String::from("CoverCalibrator not found"),
            )
        }
    };
    (StatusCode::OK, format!("{:?}", cover_calibrator))
}

async fn get_domes(State(state): State<Arc<AppState>>) -> String {
    let mut acc = String::new();
    for dome in state.domes.iter() {
        acc.push_str(format!("{:?}", dome).as_str())
    }
    acc
}

async fn get_dome_by_id(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> (StatusCode, String) {
    let dome = match state.domes.iter().find(|dome| dome.unique_id() == id) {
        Some(dome) => dome,
        None => return (StatusCode::NOT_FOUND, String::from("Dome not found")),
    };
    (StatusCode::OK, format!("{:?}", dome))
}

async fn get_filter_wheels(State(state): State<Arc<AppState>>) -> String {
    let mut acc = String::new();
    for filter_wheel in state.filter_wheels.iter() {
        acc.push_str(format!("{:?}", filter_wheel).as_str())
    }
    acc
}

async fn get_filter_wheel_by_id(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> (StatusCode, String) {
    let filter_wheel = match state
        .filter_wheels
        .iter()
        .find(|filter_wheel| filter_wheel.unique_id() == id)
    {
        Some(filter_wheel) => filter_wheel,
        None => return (StatusCode::NOT_FOUND, String::from("FilterWheel not found")),
    };
    (StatusCode::OK, format!("{:?}", filter_wheel))
}

async fn get_focusers(State(state): State<Arc<AppState>>) -> String {
    let mut acc = String::new();
    for focuser in state.focusers.iter() {
        acc.push_str(format!("{:?}", focuser).as_str())
    }
    acc
}

async fn get_focuser_by_id(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> (StatusCode, String) {
    let focuser = match state
        .focusers
        .iter()
        .find(|focuser| focuser.unique_id() == id)
    {
        Some(focuser) => focuser,
        None => return (StatusCode::NOT_FOUND, String::from("Focuser not found")),
    };
    (StatusCode::OK, format!("{:?}", focuser))
}

async fn get_observing_conditions(State(state): State<Arc<AppState>>) -> String {
    let mut acc = String::new();
    for observing_condition in state.observing_conditions.iter() {
        acc.push_str(format!("{:?}", observing_condition).as_str())
    }
    acc
}

async fn get_observing_condition_by_id(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> (StatusCode, String) {
    let observing_condition = match state
        .observing_conditions
        .iter()
        .find(|observing_condition| observing_condition.unique_id() == id)
    {
        Some(observing_condition) => observing_condition,
        None => {
            return (
                StatusCode::NOT_FOUND,
                String::from("ObservingCondition not found"),
            )
        }
    };
    (StatusCode::OK, format!("{:?}", observing_condition))
}

async fn get_rotators(State(state): State<Arc<AppState>>) -> String {
    let mut acc = String::new();
    for rotator in state.rotators.iter() {
        acc.push_str(format!("{:?}", rotator).as_str())
    }
    acc
}

async fn get_rotator_by_id(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> (StatusCode, String) {
    let rotator = match state
        .rotators
        .iter()
        .find(|rotator| rotator.unique_id() == id)
    {
        Some(rotator) => rotator,
        None => return (StatusCode::NOT_FOUND, String::from("Rotator not found")),
    };
    (StatusCode::OK, format!("{:?}", rotator))
}

async fn get_safety_monitors(State(state): State<Arc<AppState>>) -> String {
    let mut acc = String::new();
    for safety_monitor in state.safety_monitors.iter() {
        acc.push_str(format!("{:?}", safety_monitor).as_str())
    }
    acc
}

async fn get_safety_monitor_by_id(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> (StatusCode, String) {
    let safety_monitor = match state
        .safety_monitors
        .iter()
        .find(|safety_monitor| safety_monitor.unique_id() == id)
    {
        Some(safety_monitor) => safety_monitor,
        None => {
            return (
                StatusCode::NOT_FOUND,
                String::from("SafetyMonitor not found"),
            )
        }
    };
    (StatusCode::OK, format!("{:?}", safety_monitor))
}

async fn get_switches(State(state): State<Arc<AppState>>) -> String {
    let mut acc = String::new();
    for switch in state.switches.iter() {
        acc.push_str(format!("{:?}", switch).as_str())
    }
    acc
}

async fn get_switch_by_id(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> (StatusCode, String) {
    let switch = match state
        .switches
        .iter()
        .find(|switch| switch.unique_id() == id)
    {
        Some(switch) => switch,
        None => return (StatusCode::NOT_FOUND, String::from("Switch not found")),
    };
    (StatusCode::OK, format!("{:?}", switch))
}

async fn get_telescopes(State(state): State<Arc<AppState>>) -> String {
    let mut acc = String::new();
    for telescope in state.telescopes.iter() {
        acc.push_str(format!("{:?}", telescope).as_str())
    }
    acc
}

async fn get_telescope_by_id(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> (StatusCode, String) {
    let telescope = match state
        .telescopes
        .iter()
        .find(|telescope| telescope.unique_id() == id)
    {
        Some(telescope) => telescope,
        None => return (StatusCode::NOT_FOUND, String::from("Telescope not found")),
    };
    (StatusCode::OK, format!("{:?}", telescope))
}
