//! Event surface for `rp`.
//!
//! `rp` emits events when operations happen. Two delivery paths share one
//! [`EventBus`]:
//!
//! 1. **Webhooks** — the historical path. Plugins register a callback URL
//!    and a set of subscribed event types; the bus fire-and-forget POSTs
//!    matching events to each (see [`docs/services/rp.md` §Event System]).
//! 2. **In-process broadcast** — a [`tokio::sync::broadcast`] channel that
//!    carries every emitted [`EventEnvelope`]. Phase 3 of the
//!    predictive-deadlines plan wires this to a `/api/events/subscribe`
//!    SSE endpoint; unit tests use it as the assertion seam.
//!
//! Every event is wrapped in a uniform [`EventEnvelope`]. The envelope is
//! **additive** over the historical webhook body: the `event_id`, `event`,
//! `timestamp`, and `payload` keys keep their exact meaning, so existing
//! plugins are unaffected. New fields (`event_seq`, `operation_id`, the
//! `started_at` / `ended_at` / `elapsed_ms` timing, and the reserved
//! `predicted_duration_ms` / `max_duration_ms` deadline slots that Phase 2
//! will populate) are carried alongside.
//!
//! Blocking operations emit a *triple*: a `*_started` envelope at the
//! entry point and a `*_complete` or `*_failed` envelope at the end, all
//! sharing one `operation_id` so a consumer can correlate them.

use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;
use tracing::debug;
use uuid::Uuid;

/// Capacity of the in-process broadcast channel. A consumer that falls
/// further behind than this many events sees a
/// [`broadcast::error::RecvError::Lagged`] and resumes from the channel's
/// current tail; the channel itself keeps running.
const BROADCAST_CAPACITY: usize = 256;

/// A webhook subscriber: a plugin that receives matching events via HTTP POST.
pub struct EventPlugin {
    pub name: String,
    pub webhook_url: String,
    pub subscribes_to: Vec<String>,
}

/// The uniform wire shape for every emitted event.
///
/// Serializes to the webhook body and (Phase 3) the SSE `data:` payload.
/// Optional fields are omitted from JSON when absent (`skip_serializing_if`),
/// so historical point events (e.g. `filter_switch`) keep their original
/// `{event_id, event, timestamp, payload}` shape plus the new `event_seq`.
#[derive(Debug, Clone, Serialize)]
pub struct EventEnvelope {
    /// Per-emission UUID. Unchanged from the historical webhook body; this
    /// is the routing key for the plugin completion contract
    /// (`POST /api/plugins/{event_id}/complete`). Assigned by the bus.
    pub event_id: String,
    /// Monotonically increasing per-emission counter. Total order across
    /// all events; used as the SSE `id` (and `Last-Event-ID` replay key)
    /// in Phase 3. Assigned by the bus.
    pub event_seq: u64,
    /// Correlation key shared by an operation's `*_started`, `*_complete`,
    /// and `*_failed` events. `None` for historical point events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    /// The event type string, e.g. `"slew_started"`.
    pub event: String,
    /// ISO-8601 emission timestamp. Unchanged historical format. Assigned
    /// by the bus.
    pub timestamp: String,
    /// When the operation began (RFC-3339, millisecond precision).
    /// Present on operation events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// When the operation ended. Present on `*_complete` / `*_failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    /// Wall-clock duration of the operation. Present on
    /// `*_complete` / `*_failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    /// Predicted operation duration. Reserved for Phase 2; always `None`
    /// (omitted) in Phase 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predicted_duration_ms: Option<u64>,
    /// Hard ceiling beyond which the operation is considered overrun.
    /// Reserved for Phase 2; always `None` (omitted) in Phase 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_duration_ms: Option<u64>,
    /// Operation-specific detail. For `*_started` this carries the inputs
    /// (e.g. target coordinates); for `*_complete` / `*_failed` it carries
    /// the outcome (result fields, or `{"error": "..."}`). Kept under the
    /// historical `payload` key so existing subscribers are unaffected.
    pub payload: Value,
}

impl EventEnvelope {
    /// Build a `*_started` envelope. `operation` is the event-name prefix
    /// (e.g. `"slew"` → `"slew_started"`). Identity fields (`event_id`,
    /// `event_seq`, `timestamp`) are filled in by the bus at emit time.
    pub fn started(
        operation: &str,
        operation_id: &str,
        started_at: DateTime<Utc>,
        payload: Value,
    ) -> Self {
        Self {
            event_id: String::new(),
            event_seq: 0,
            operation_id: Some(operation_id.to_string()),
            event: format!("{operation}_started"),
            timestamp: String::new(),
            started_at: Some(started_at.to_rfc3339_opts(SecondsFormat::Millis, true)),
            ended_at: None,
            elapsed_ms: None,
            predicted_duration_ms: None,
            max_duration_ms: None,
            payload,
        }
    }

    /// Build a `*_complete` envelope, computing `ended_at` and `elapsed_ms`
    /// from `started_at` and the current time. `payload` carries the
    /// operation outcome.
    pub fn complete(
        operation: &str,
        operation_id: &str,
        started_at: DateTime<Utc>,
        payload: Value,
    ) -> Self {
        Self::ended(operation, "complete", operation_id, started_at, payload)
    }

    /// Build a `*_failed` envelope. The error message is carried as
    /// `{"error": <message>}` in the payload.
    pub fn failed(
        operation: &str,
        operation_id: &str,
        started_at: DateTime<Utc>,
        error: &str,
    ) -> Self {
        Self::ended(
            operation,
            "failed",
            operation_id,
            started_at,
            serde_json::json!({ "error": error }),
        )
    }

    fn ended(
        operation: &str,
        suffix: &str,
        operation_id: &str,
        started_at: DateTime<Utc>,
        payload: Value,
    ) -> Self {
        let ended_at = Utc::now();
        let elapsed_ms = (ended_at - started_at).num_milliseconds().max(0) as u64;
        Self {
            event_id: String::new(),
            event_seq: 0,
            operation_id: Some(operation_id.to_string()),
            event: format!("{operation}_{suffix}"),
            timestamp: String::new(),
            started_at: Some(started_at.to_rfc3339_opts(SecondsFormat::Millis, true)),
            ended_at: Some(ended_at.to_rfc3339_opts(SecondsFormat::Millis, true)),
            elapsed_ms: Some(elapsed_ms),
            predicted_duration_ms: None,
            max_duration_ms: None,
            payload,
        }
    }
}

/// Fans an emitted event out to webhook subscribers and to in-process
/// broadcast consumers.
pub struct EventBus {
    plugins: Vec<EventPlugin>,
    broadcast: broadcast::Sender<EventEnvelope>,
    next_seq: AtomicU64,
}

impl EventBus {
    pub fn from_config(plugin_configs: &[Value]) -> Self {
        let plugins = plugin_configs
            .iter()
            .filter(|p| p.get("type").and_then(|v| v.as_str()) == Some("event"))
            .filter_map(|p| {
                let name = p.get("name")?.as_str()?.to_string();
                let webhook_url = p.get("webhook_url")?.as_str()?.to_string();
                let subscribes_to = p
                    .get("subscribes_to")?
                    .as_array()?
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
                Some(EventPlugin {
                    name,
                    webhook_url,
                    subscribes_to,
                })
            })
            .collect();

        let (broadcast, _rx) = broadcast::channel(BROADCAST_CAPACITY);

        Self {
            plugins,
            broadcast,
            // Start at 1 so the first event has event_seq == 1.
            next_seq: AtomicU64::new(1),
        }
    }

    /// Subscribe to the in-process event stream. Each subscriber receives
    /// every envelope emitted after it subscribes. Used by the SSE endpoint
    /// (Phase 3) and by tests.
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.broadcast.subscribe()
    }

    /// Emit a historical point event (no operation lifecycle). Keeps the
    /// original signature; the envelope carries `operation_id == None`.
    pub fn emit(&self, event_type: &str, payload: Value) {
        let envelope = EventEnvelope {
            event_id: String::new(),
            event_seq: 0,
            operation_id: None,
            event: event_type.to_string(),
            timestamp: String::new(),
            started_at: None,
            ended_at: None,
            elapsed_ms: None,
            predicted_duration_ms: None,
            max_duration_ms: None,
            payload,
        };
        self.dispatch(envelope);
    }

    /// Emit an operation lifecycle event built via [`EventEnvelope::started`],
    /// [`EventEnvelope::complete`], or [`EventEnvelope::failed`].
    pub fn emit_operation(&self, envelope: EventEnvelope) {
        self.dispatch(envelope);
    }

    /// Assign identity fields, then fan out to webhooks and the broadcast
    /// channel.
    fn dispatch(&self, mut envelope: EventEnvelope) {
        envelope.event_id = Uuid::new_v4().to_string();
        envelope.event_seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        envelope.timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

        self.deliver_to_plugins(&envelope);

        // Broadcast for in-process consumers. An error means there are no
        // active subscribers (or all have lagged out); that is expected and
        // harmless — the webhook path above is independent.
        let _ = self.broadcast.send(envelope);
    }

    fn deliver_to_plugins(&self, envelope: &EventEnvelope) {
        let event_type = &envelope.event;
        for plugin in &self.plugins {
            if plugin.subscribes_to.iter().any(|s| s == event_type) {
                let url = plugin.webhook_url.clone();
                let name = plugin.name.clone();
                let event_type = event_type.clone();
                let body = match serde_json::to_value(envelope) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(
                            plugin = %name,
                            event = %event_type,
                            error = %e,
                            "skipping event delivery: envelope serialization failed"
                        );
                        continue;
                    }
                };

                tokio::spawn(async move {
                    debug!(plugin = %name, event = %event_type, url = %url, "emitting event to plugin");
                    let client = match reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(5))
                        .build()
                    {
                        Ok(c) => c,
                        Err(e) => {
                            tracing::warn!(
                                plugin = %name,
                                event = %event_type,
                                error = %e,
                                "skipping event delivery: reqwest client build failed (likely TLS init); subsequent events will retry"
                            );
                            return;
                        }
                    };
                    match client.post(&url).json(&body).send().await {
                        Ok(resp) => {
                            debug!(plugin = %name, event = %event_type, status = %resp.status(), "event delivered");
                        }
                        Err(e) => {
                            debug!(plugin = %name, event = %event_type, error = %e, "failed to deliver event");
                        }
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Test code opts out of the workspace restriction lints (see root
    // Cargo.toml `[workspace.lints.clippy]`); per AGENTS rule 7 tests
    // prefer `unwrap()` so a failure shows what the error was.
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::indexing_slicing)]

    use super::*;

    fn bus() -> EventBus {
        EventBus::from_config(&[])
    }

    #[tokio::test]
    async fn legacy_emit_has_no_operation_id_and_monotonic_seq() {
        let bus = bus();
        let mut rx = bus.subscribe();

        bus.emit("filter_switch", serde_json::json!({ "filter_name": "Ha" }));
        bus.emit("safety_changed", serde_json::json!({ "monitor": "roof" }));

        let first = rx.recv().await.unwrap();
        let second = rx.recv().await.unwrap();

        assert_eq!(first.event, "filter_switch");
        assert!(first.operation_id.is_none());
        assert_eq!(first.payload["filter_name"], "Ha");
        // event_seq is monotonically increasing across emissions.
        assert_eq!(second.event_seq, first.event_seq + 1);
        // Distinct per-emission event_id.
        assert_ne!(first.event_id, second.event_id);
    }

    #[tokio::test]
    async fn operation_triple_shares_operation_id() {
        let bus = bus();
        let mut rx = bus.subscribe();
        let operation_id = "op-123";
        let started_at = Utc::now();

        bus.emit_operation(EventEnvelope::started(
            "slew",
            operation_id,
            started_at,
            serde_json::json!({ "ra": 12.0, "dec": -30.0 }),
        ));
        bus.emit_operation(EventEnvelope::complete(
            "slew",
            operation_id,
            started_at,
            serde_json::json!({ "actual_ra": 12.0, "actual_dec": -30.0 }),
        ));

        let started = rx.recv().await.unwrap();
        let complete = rx.recv().await.unwrap();

        assert_eq!(started.event, "slew_started");
        assert_eq!(complete.event, "slew_complete");
        // Same operation_id correlates the two events.
        assert_eq!(started.operation_id.as_deref(), Some(operation_id));
        assert_eq!(complete.operation_id.as_deref(), Some(operation_id));
        // ...but each emission has its own event_id and event_seq.
        assert_ne!(started.event_id, complete.event_id);
        assert!(complete.event_seq > started.event_seq);
        // started carries started_at but no end timing.
        assert!(started.started_at.is_some());
        assert!(started.ended_at.is_none());
        // complete carries the full timing trio.
        assert!(complete.ended_at.is_some());
        assert!(complete.elapsed_ms.is_some());
        // Phase 1 reserves but never populates the deadline fields.
        assert!(started.predicted_duration_ms.is_none());
        assert!(complete.max_duration_ms.is_none());
    }

    #[tokio::test]
    async fn failed_carries_error_payload() {
        let bus = bus();
        let mut rx = bus.subscribe();
        let started_at = Utc::now();

        bus.emit_operation(EventEnvelope::failed(
            "park",
            "op-9",
            started_at,
            "mount unreachable",
        ));

        let failed = rx.recv().await.unwrap();
        assert_eq!(failed.event, "park_failed");
        assert_eq!(failed.payload["error"], "mount unreachable");
        assert!(failed.elapsed_ms.is_some());
    }

    #[test]
    fn envelope_omits_absent_optional_fields_in_json() {
        // A legacy point event serializes to the historical shape plus
        // event_seq — no operation_id / timing / deadline keys.
        let envelope = EventEnvelope {
            event_id: "e1".to_string(),
            event_seq: 7,
            operation_id: None,
            event: "filter_switch".to_string(),
            timestamp: "2026-05-19T20:14:33Z".to_string(),
            started_at: None,
            ended_at: None,
            elapsed_ms: None,
            predicted_duration_ms: None,
            max_duration_ms: None,
            payload: serde_json::json!({ "filter_name": "Ha" }),
        };
        let json = serde_json::to_value(&envelope).unwrap();
        let obj = json.as_object().unwrap();
        assert!(obj.contains_key("event_id"));
        assert!(obj.contains_key("event_seq"));
        assert!(obj.contains_key("event"));
        assert!(obj.contains_key("timestamp"));
        assert!(obj.contains_key("payload"));
        assert!(!obj.contains_key("operation_id"));
        assert!(!obj.contains_key("started_at"));
        assert!(!obj.contains_key("predicted_duration_ms"));
    }
}
