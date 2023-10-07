use ascom_alpaca::api::{Camera, ServerInfo, TypedDevice};
use ascom_alpaca::Client;
use axum::extract::State;
use axum::{routing::get, Router};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::error;

#[derive(Debug)]
struct AppState {
    server_url: String,
    server_info: ServerInfo,
    client: Client,
    cameras: HashSet<Arc<dyn Camera>>,
}

#[tokio::main]
async fn main() {
    // initialize tracing
    tracing_subscriber::fmt::init();

    // create a client
    let client = match Client::new("http://localhost:32323") {
        Ok(client) => client,
        Err(e) => {
            error!("Error: {}", e);
            return;
        }
    };

    // `get_server_info` returns high-level metadata of the server.
    let server_info = match client.get_server_info().await {
        Ok(info) => info,
        Err(e) => {
            error!("Error: {}", e);
            return;
        }
    };

    let cameras = match client.get_devices().await {
        Ok(devices) => devices
            .filter_map(|device| match device {
                TypedDevice::Camera(camera) => Some(camera),
                _ => None,
            })
            .collect::<HashSet<_>>(),
        Err(e) => {
            error!("Error: {}", e);
            return;
        }
    };

    let state = AppState {
        server_url: String::from("http://localhost:32323"),
        server_info,
        client,
        cameras,
    };

    // build our application with a route
    let app = Router::new()
        // `GET /` goes to `root`
        .route("/", get(root))
        // `POST /users` goes to `create_user`
        .route("/camera", get(get_camera))
        .with_state(Arc::new(state));

    // run our app with hyper
    // `axum::Server` is a re-export of `hyper::Server`
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    tracing::debug!("listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

// basic handler that responds with a static string
async fn root(State(state): State<Arc<AppState>>) -> String {
    format!("{:?}", state)
}

async fn get_camera(State(state): State<Arc<AppState>>) -> String {
    let camera = match state.cameras.iter().next() {
        Some(camera) => camera,
        None => return String::from("No camera found"),
    };
    format!("{:?}", camera)
}
