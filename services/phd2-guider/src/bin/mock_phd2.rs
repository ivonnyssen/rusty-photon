//! Mock PHD2 server for testing
//!
//! A simple mock PHD2 server that responds to JSON-RPC requests.
//! Used for testing process management and connection handling.
//!
//! Usage:
//!   mock_phd2 [--port PORT]
//!
//! The port can also be set via the MOCK_PHD2_PORT environment variable.
//! Command line argument takes precedence over environment variable.
//! Default port is 4400 (same as PHD2).

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn main() {
    // Port priority: command line arg > environment variable > default (4400)
    let port = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            std::env::var("MOCK_PHD2_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(4400u16);

    eprintln!("Mock PHD2 starting on port {}", port);

    let listener = match TcpListener::bind(format!("127.0.0.1:{}", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Failed to bind to port {}: {}", port, e);
            std::process::exit(1);
        }
    };

    let shutdown = Arc::new(AtomicBool::new(false));

    // Set a timeout so we can check shutdown flag periodically
    listener
        .set_nonblocking(true)
        .expect("Cannot set non-blocking");

    eprintln!("Mock PHD2 listening on port {}", port);

    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, addr)) => {
                eprintln!("Connection from {}", addr);
                let shutdown_clone = shutdown.clone();
                std::thread::spawn(move || {
                    handle_client(stream, shutdown_clone);
                });
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No connection available, sleep briefly
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                eprintln!("Accept error: {}", e);
            }
        }
    }

    eprintln!("Mock PHD2 shutting down");
}

fn handle_client(mut stream: TcpStream, shutdown: Arc<AtomicBool>) {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(1)))
        .ok();
    stream
        .set_write_timeout(Some(std::time::Duration::from_secs(5)))
        .ok();

    // Send Version event on connect
    let version_event =
        r#"{"Event":"Version","PHDVersion":"2.6.11-mock","PHDSubver":"test","MsgVersion":1}"#;
    if writeln!(stream, "{}", version_event).is_err() {
        return;
    }
    if stream.flush().is_err() {
        return;
    }

    eprintln!("Sent version event");

    let reader = BufReader::new(stream.try_clone().unwrap());

    for line in reader.lines() {
        match line {
            Ok(request) => {
                if request.is_empty() {
                    continue;
                }

                eprintln!("Received: {}", request);

                let response = handle_request(&request, &shutdown);
                eprintln!("Sending: {}", response);

                if writeln!(stream, "{}", response).is_err() {
                    break;
                }
                if stream.flush().is_err() {
                    break;
                }

                // Check if we should shutdown
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // Timeout, check shutdown flag
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                continue;
            }
            Err(_) => {
                break;
            }
        }
    }

    eprintln!("Client disconnected");
}

fn handle_request(request: &str, shutdown: &Arc<AtomicBool>) -> String {
    // Parse JSON-RPC request
    let req: serde_json::Value = match serde_json::from_str(request) {
        Ok(v) => v,
        Err(_) => {
            return r#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"Parse error"},"id":null}"#
                .to_string();
        }
    };

    let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");

    // Mock responses for different methods
    let result = match method {
        "get_app_state" => serde_json::json!("Stopped"),
        "get_connected" => serde_json::json!(false),
        "set_connected" => serde_json::json!(0),
        "get_profiles" => serde_json::json!([
            {"id": 1, "name": "Mock Profile"}
        ]),
        "get_profile" => serde_json::json!({"id": 1, "name": "Mock Profile"}),
        "set_profile" => serde_json::json!(0),
        "get_current_equipment" => serde_json::json!({
            "camera": {"name": "Mock Camera", "connected": false},
            "mount": {"name": "Mock Mount", "connected": false},
            "aux_mount": null,
            "AO": null,
            "rotator": null
        }),
        "get_exposure" => serde_json::json!(1000),
        "set_exposure" => serde_json::json!(0),
        "get_exposure_durations" => serde_json::json!([100, 200, 500, 1000, 2000, 3000]),
        "get_camera_frame_size" => serde_json::json!([640, 480]),
        "get_use_subframes" => serde_json::json!(false),
        "get_calibrated" => serde_json::json!(false),
        "get_calibration_data" => serde_json::json!({
            "calibrated": false,
            "xAngle": 0.0,
            "xRate": 0.0,
            "xParity": "+",
            "yAngle": 0.0,
            "yRate": 0.0,
            "yParity": "+"
        }),
        "clear_calibration" => serde_json::json!(0),
        "flip_calibration" => serde_json::json!(0),
        "get_lock_position" => serde_json::json!(null),
        "set_lock_position" => serde_json::json!(0),
        "find_star" => serde_json::json!(0),
        "get_paused" => serde_json::json!(false),
        "set_paused" => serde_json::json!(0),
        "guide" => serde_json::json!(0),
        "loop" => serde_json::json!(0),
        "stop_capture" => serde_json::json!(0),
        "dither" => serde_json::json!(0),
        "get_algo_param_names" => serde_json::json!(["Aggressiveness", "MinMove"]),
        "get_algo_param" => serde_json::json!(0.5),
        "set_algo_param" => serde_json::json!(0),
        "get_ccd_temperature" => serde_json::json!(20.0),
        "get_cooler_status" => serde_json::json!({
            "temperature": 20.0,
            "coolerOn": false
        }),
        "set_cooler_state" => serde_json::json!(0),
        "get_star_image" => serde_json::json!({
            "frame": 1,
            "width": 32,
            "height": 32,
            "star_pos": [16.0, 16.0],
            "pixels": "AAAA"
        }),
        "save_image" => serde_json::json!("/tmp/mock_image.fits"),
        "capture_single_frame" => serde_json::json!(0),
        "shutdown" => {
            eprintln!("Shutdown requested");
            shutdown.store(true, Ordering::Relaxed);
            serde_json::json!(0)
        }
        _ => {
            return format!(
                r#"{{"jsonrpc":"2.0","error":{{"code":-32601,"message":"Method not found: {}"}},"id":{}}}"#,
                method, id
            );
        }
    };

    format!(r#"{{"jsonrpc":"2.0","result":{},"id":{}}}"#, result, id)
}
