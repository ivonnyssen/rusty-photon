//! In-process HTTP webhook receiver that acts as an rp event plugin.
//!
//! Tests register the receiver's URL as a webhook in rp's plugin config
//! and then assert against captured events.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::Router;
use serde_json::Value;
use tokio::sync::RwLock;

/// A single event captured by [`WebhookReceiver`].
#[derive(Debug, Clone)]
pub struct ReceivedEvent {
    pub event_id: String,
    pub event_type: String,
    pub timestamp: String,
    pub payload: Value,
    pub received_at: std::time::Instant,
}

/// Shared state for the webhook receiver.
#[derive(Debug, Clone)]
struct WebhookReceiverState {
    events: Arc<RwLock<Vec<ReceivedEvent>>>,
    ack_estimated: Duration,
    ack_max: Duration,
}

/// In-process HTTP server that acts as an event plugin.
#[derive(Debug)]
pub struct WebhookReceiver {
    pub url: String,
    pub port: u16,
    pub events: Arc<RwLock<Vec<ReceivedEvent>>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl WebhookReceiver {
    /// Start the webhook receiver on a random port.
    ///
    /// The caller provides the shared events vector so test assertions can
    /// read from the same `Arc` the receiver writes to.
    pub async fn start(
        events: Arc<RwLock<Vec<ReceivedEvent>>>,
        ack_estimated: Duration,
        ack_max: Duration,
    ) -> Self {
        let state = WebhookReceiverState {
            events: events.clone(),
            ack_estimated,
            ack_max,
        };

        let app = Router::new()
            .route("/webhook", post(webhook_handler))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind webhook receiver");
        let port = listener.local_addr().unwrap().port();

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("webhook receiver failed");
        });

        Self {
            url: format!("http://127.0.0.1:{}/webhook", port),
            port,
            events,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    /// Stop the webhook receiver.
    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for WebhookReceiver {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn webhook_handler(
    State(state): State<WebhookReceiverState>,
    axum::Json(body): axum::Json<Value>,
) -> (StatusCode, axum::Json<Value>) {
    let event = ReceivedEvent {
        event_id: body
            .get("event_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        event_type: body
            .get("event")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        timestamp: body
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        payload: body.get("payload").cloned().unwrap_or(Value::Null),
        received_at: std::time::Instant::now(),
    };

    state.events.write().await.push(event);

    let ack = serde_json::json!({
        "estimated_duration": humantime::format_duration(state.ack_estimated).to_string(),
        "max_duration": humantime::format_duration(state.ack_max).to_string()
    });

    (StatusCode::OK, axum::Json(ack))
}
