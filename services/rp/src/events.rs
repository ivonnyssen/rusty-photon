use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tracing::debug;
use uuid::Uuid;

pub struct EventPlugin {
    pub name: String,
    pub webhook_url: String,
    pub subscribes_to: Vec<String>,
}

pub struct EventBus {
    plugins: Vec<EventPlugin>,
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

        Self { plugins }
    }

    pub fn emit(&self, event_type: &str, payload: Value) {
        let event_id = Uuid::new_v4().to_string();
        let timestamp = format_timestamp();

        let event_body = serde_json::json!({
            "event_id": event_id,
            "event": event_type,
            "timestamp": timestamp,
            "payload": payload,
        });

        for plugin in &self.plugins {
            if plugin.subscribes_to.iter().any(|s| s == event_type) {
                let url = plugin.webhook_url.clone();
                let body = event_body.clone();
                let name = plugin.name.clone();
                let event_type = event_type.to_string();

                tokio::spawn(async move {
                    debug!(plugin = %name, event = %event_type, url = %url, "emitting event to plugin");
                    let client = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(5))
                        .build()
                        .unwrap();
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

fn format_timestamp() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();

    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let seconds = time_secs % 60;

    // Calculate year, month, day from days since epoch
    let mut y = 1970i64;
    let mut remaining_days = days as i64;

    loop {
        let days_in_year = if is_leap_year(y) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        y += 1;
    }

    let month_days = if is_leap_year(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut m = 0;
    for (i, &md) in month_days.iter().enumerate() {
        if remaining_days < md {
            m = i;
            break;
        }
        remaining_days -= md;
    }

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m + 1,
        remaining_days + 1,
        hours,
        minutes,
        seconds
    )
}

fn is_leap_year(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}
