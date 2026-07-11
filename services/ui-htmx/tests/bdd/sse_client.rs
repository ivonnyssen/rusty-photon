//! A minimal SSE reader for the BFF's `/stream/events` proxy.
//!
//! Deliberately test-local (mirrors `bdd_infra::rp_harness::sse`, which is
//! hard-wired to rp's subscribe path): connects to the given URL, accumulates
//! parsed frames on a background task, and exposes bounded waits. The reader
//! must be **dropped before the BFF stops** (testing.md §5.4) — [`Drop`]
//! aborts the task, closing the connection so graceful shutdown can finish.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

/// One SSE frame as received from the BFF.
#[derive(Debug, Clone)]
pub struct Frame {
    pub id: Option<String>,
    pub event: Option<String>,
    pub data: String,
}

#[derive(Debug)]
pub struct StreamEventsClient {
    frames: Arc<RwLock<Vec<Frame>>>,
    /// Set when the server closed the stream (the rp-unreachable contract:
    /// the proxy pushes a status frame, then ends for `EventSource` retry).
    ended: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl StreamEventsClient {
    pub async fn connect(url: &str, last_event_id: Option<u64>) -> Self {
        let mut request = reqwest::Client::new()
            .get(url)
            .header("accept", "text/event-stream");
        if let Some(id) = last_event_id {
            request = request.header("last-event-id", id.to_string());
        }
        let response = request.send().await.expect("SSE connect failed");
        assert_eq!(response.status().as_u16(), 200, "GET {url} must answer 200");

        let frames: Arc<RwLock<Vec<Frame>>> = Arc::new(RwLock::new(Vec::new()));
        let ended = Arc::new(AtomicBool::new(false));
        let frames_task = Arc::clone(&frames);
        let ended_task = Arc::clone(&ended);
        let task = tokio::spawn(async move {
            let mut response = response;
            let mut buffer = String::new();
            while let Ok(Some(chunk)) = response.chunk().await {
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                let parsed = drain_frames(&mut buffer);
                if !parsed.is_empty() {
                    frames_task.write().await.extend(parsed);
                }
            }
            ended_task.store(true, Ordering::SeqCst);
        });
        Self {
            frames,
            ended,
            task,
        }
    }

    pub async fn frames(&self) -> Vec<Frame> {
        self.frames.read().await.clone()
    }

    /// Wait (bounded) for a frame with the given `event:` name whose data
    /// contains `needle`.
    pub async fn wait_for(&self, event: &str, needle: &str) -> Frame {
        for _ in 0..100 {
            if let Some(frame) = self
                .frames
                .read()
                .await
                .iter()
                .find(|f| f.event.as_deref() == Some(event) && f.data.contains(needle))
            {
                return frame.clone();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!(
            "no `{event}` frame containing {needle:?} within 10s; got: {:?}",
            self.frames
                .read()
                .await
                .iter()
                .map(|f| (f.event.clone(), f.data.chars().take(80).collect::<String>()))
                .collect::<Vec<_>>()
        );
    }

    /// Wait (bounded) for the server to end the stream.
    pub async fn wait_for_end(&self) {
        for _ in 0..100 {
            if self.ended.load(Ordering::SeqCst) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("the SSE stream did not end within 10s");
    }
}

impl Drop for StreamEventsClient {
    fn drop(&mut self) {
        // Abort the reader so the connection closes and the BFF's graceful
        // shutdown (and coverage flush) is never blocked by this stream.
        self.task.abort();
    }
}

/// Extract every complete (blank-line-terminated) SSE frame from `buffer`,
/// leaving any trailing partial frame in place. Handles CRLF and LF, comment
/// lines, and multi-line `data:`.
fn drain_frames(buffer: &mut String) -> Vec<Frame> {
    let normalized = buffer.replace("\r\n", "\n");
    let mut frames = Vec::new();
    let mut rest = normalized.as_str();
    while let Some(end) = rest.find("\n\n") {
        let (raw, remainder) = rest.split_at(end);
        rest = &remainder[2..];
        let mut frame = Frame {
            id: None,
            event: None,
            data: String::new(),
        };
        for line in raw.lines() {
            if line.starts_with(':') {
                continue;
            }
            if let Some(value) = line.strip_prefix("id:") {
                frame.id = Some(value.trim_start().to_string());
            } else if let Some(value) = line.strip_prefix("event:") {
                frame.event = Some(value.trim_start().to_string());
            } else if let Some(value) = line.strip_prefix("data:") {
                if !frame.data.is_empty() {
                    frame.data.push('\n');
                }
                frame
                    .data
                    .push_str(value.strip_prefix(' ').unwrap_or(value));
            }
        }
        if frame.id.is_some() || frame.event.is_some() || !frame.data.is_empty() {
            frames.push(frame);
        }
    }
    *buffer = rest.to_string();
    frames
}
