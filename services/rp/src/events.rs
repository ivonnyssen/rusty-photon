use chrono::Utc;
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
        let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

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
