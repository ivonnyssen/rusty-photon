//! Raw wire capture: every Alpaca HTTP request/response and every discovery
//! packet, appended as one JSON object per line.
//!
//! The JSONL file is the spike's primary artifact — the console shows the
//! narrative (connects, GoTos, verdicts), the file holds the evidence
//! (exact member casing, parameter spelling, poll cadence, unknown verbs).

use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use axum::body::{to_bytes, Body};
use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use serde::Serialize;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Cap on buffered request/response bodies. Telescope traffic is tiny
/// form/JSON payloads; anything larger is logged truncated.
const BODY_LIMIT: usize = 256 * 1024;

#[derive(Serialize)]
struct WireEntry<'a> {
    seq: u64,
    /// RFC 3339 receipt timestamp, millisecond precision — the cadence data.
    t: String,
    kind: &'a str,
    client: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    elapsed_us: Option<u128>,
}

/// Append-only JSONL sink, flushed per line so a Ctrl-C mid-session loses
/// nothing.
pub struct WireLog {
    file: Mutex<BufWriter<File>>,
    seq: AtomicU64,
}

impl WireLog {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        Ok(Self {
            file: Mutex::new(BufWriter::new(File::create(path)?)),
            seq: AtomicU64::new(0),
        })
    }

    fn write(&self, entry: &WireEntry<'_>) {
        match serde_json::to_string(entry) {
            Ok(line) => match self.file.lock() {
                Ok(mut file) => {
                    if writeln!(file, "{line}")
                        .and_then(|()| file.flush())
                        .is_err()
                    {
                        warn!("wire log write failed; entry lost: {line}");
                    }
                }
                Err(poisoned) => {
                    let mut file = poisoned.into_inner();
                    let _ = writeln!(file, "{line}");
                }
            },
            Err(e) => warn!("wire log serialize failed: {e}"),
        }
    }

    pub fn flush(&self) {
        if let Ok(mut file) = self.file.lock() {
            let _ = file.flush();
        }
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    /// Record a discovery datagram (and whether we answered it).
    pub fn record_discovery(&self, src: SocketAddr, payload: &str, reply: Option<&str>) {
        self.write(&WireEntry {
            seq: self.next_seq(),
            t: now_rfc3339_millis(),
            kind: "discovery",
            client: src.to_string(),
            method: None,
            path: None,
            query: None,
            body: Some(payload.to_owned()),
            status: None,
            response: reply.map(str::to_owned),
            elapsed_us: None,
        });
    }
}

fn now_rfc3339_millis() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Buffer, log, and forward one Alpaca HTTP exchange.
pub async fn wire_middleware(
    State(log): State<Arc<WireLog>>,
    ConnectInfo(client): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    let received_at = now_rfc3339_millis();
    let (parts, body) = request.into_parts();
    let method = parts.method.clone();
    let path = parts.uri.path().to_owned();
    let query = parts.uri.query().map(str::to_owned);
    let request_body = match to_bytes(body, BODY_LIMIT).await {
        Ok(bytes) => bytes,
        Err(e) => {
            warn!("request body read failed for {method} {path}: {e}");
            axum::body::Bytes::new()
        }
    };
    let request_body_text = String::from_utf8_lossy(&request_body).into_owned();

    let started = Instant::now();
    let response = next
        .run(Request::from_parts(parts, Body::from(request_body)))
        .await;
    let elapsed_us = started.elapsed().as_micros();

    let (parts, body) = response.into_parts();
    let response_body = match to_bytes(body, BODY_LIMIT).await {
        Ok(bytes) => bytes,
        Err(e) => {
            warn!("response body read failed for {method} {path}: {e}");
            axum::body::Bytes::new()
        }
    };
    let response_body_text = String::from_utf8_lossy(&response_body).into_owned();
    let status = parts.status.as_u16();

    // Mutations tell the intent story — surface them on the console. Reads
    // are the cadence data — JSONL only, debug on the console.
    if method == axum::http::Method::PUT {
        info!("{client} PUT {path} [{request_body_text}] → {status} {response_body_text}");
    } else {
        debug!("{client} {method} {path} → {status}");
    }

    log.write(&WireEntry {
        seq: log.next_seq(),
        t: received_at,
        kind: "http",
        client: client.to_string(),
        method: Some(method.to_string()),
        path: Some(path),
        query,
        body: Some(request_body_text),
        status: Some(status),
        response: Some(response_body_text),
        elapsed_us: Some(elapsed_us),
    });

    Response::from_parts(parts, Body::from(response_body))
}
