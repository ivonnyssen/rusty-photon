//! Cucumber `World` for the `ui-htmx` config-page BDD suite.
//!
//! Mirrors the binary-spawning pattern the other services use (see
//! [`sentinel`](../../../../sentinel/tests/bdd/world.rs)): every scenario
//! spawns the *real* `ui-htmx` binary **plus** a real `dsd-fp2` driver (mock
//! hardware) for it to configure, and drives the BFF over HTTP. There is no
//! in-process router and no stubbed client — the production
//! `ReqwestHttpClient` → `AlpacaConfigClient` path and the driver's real
//! `config.get` / `config.apply` / in-process reload are exercised end to end.
//!
//! Requires both binaries pre-built with `--all-features` (the `dsd-fp2` mock
//! transport is feature-gated): `cargo build --all-features --all-targets`.

use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

use bdd_infra::ServiceHandle;
use cucumber::World;
use serde_json::{json, Value};
use tempfile::TempDir;

/// The dsd-fp2 CoverCalibrator action endpoint the BFF (and these helpers) call.
const DRIVER_ACTION_PATH: &str = "/api/v1/covercalibrator/0/action";

#[derive(Debug, Default, World)]
pub struct UiWorld {
    /// The dsd-fp2 driver the BFF configures (absent in the "unreachable" case).
    pub driver: Option<ServiceHandle>,
    /// The BFF under test.
    pub ui: Option<ServiceHandle>,
    temp_dir: Option<TempDir>,
    /// The port the driver binds. Fixed (not OS-assigned) so the driver's
    /// in-process reload rebinds the *same* port and the BFF can reconnect.
    driver_port: u16,
    /// The rendered HTML of the last BFF response.
    pub last_body: String,
}

impl UiWorld {
    fn temp_path(&mut self, file: &str) -> PathBuf {
        self.temp_dir
            .get_or_insert_with(|| TempDir::new().expect("failed to create temp dir"))
            .path()
            .join(file)
    }

    // --- spawning real services -------------------------------------------

    /// Spawn a real dsd-fp2 driver (mock hardware) reporting the given
    /// effective `serial.port` and `max_brightness`, then point a fresh BFF at
    /// it.
    pub async fn start_driver_and_bff(&mut self, serial_port: &str, max_brightness: u32) {
        let port = free_port();
        let config_path = self.write_driver_config(serial_port, max_brightness, port);
        let handle = ServiceHandle::start("dsd-fp2", &config_path).await;
        self.driver_port = handle.port;
        self.driver = Some(handle);
        self.wait_for_driver_ready().await;
        self.start_bff_pointing_at(self.driver_port).await;
    }

    /// Spawn a real dsd-fp2 driver whose `serial.port` is pinned by a `--port`
    /// command-line override (so `config.get` reports it in `overrides[]`),
    /// then point a fresh BFF at it.
    pub async fn start_driver_with_serial_override_and_bff(&mut self, serial_port: &str) {
        let port = free_port();
        // The file carries its own serial.port; the override pins a different
        // effective value, which is what the page must render read-only.
        let config_path = self.write_driver_config("/dev/ttyACM0", 4096, port);
        let handle = ServiceHandle::start_with_args(
            "dsd-fp2",
            &["--config", &config_path, "--port", serial_port],
        )
        .await;
        self.driver_port = handle.port;
        self.driver = Some(handle);
        self.wait_for_driver_ready().await;
        self.start_bff_pointing_at(self.driver_port).await;
    }

    /// Spawn a BFF pointed at a driver that is not running (a free port nothing
    /// listens on), so `config.get` is refused.
    pub async fn start_bff_with_unreachable_driver(&mut self) {
        let dead_port = free_port();
        self.start_bff_pointing_at(dead_port).await;
    }

    async fn start_bff_pointing_at(&mut self, driver_port: u16) {
        let config = json!({
            "server": { "bind": "127.0.0.1", "port": 0 },
            "drivers": {
                "dsd-fp2": {
                    "base_url": format!("http://127.0.0.1:{driver_port}"),
                    "device_type": "covercalibrator",
                    "device_number": 0
                }
            }
        });
        let path = self.temp_path("ui-htmx.json");
        std::fs::write(&path, config.to_string()).expect("failed to write BFF config");
        let handle = ServiceHandle::start("ui-htmx", path.to_str().unwrap()).await;
        self.ui = Some(handle);
    }

    fn write_driver_config(&mut self, serial_port: &str, max_brightness: u32, port: u16) -> String {
        let config = json!({
            "serial": {
                "port": serial_port,
                "baud_rate": 115200,
                "polling_interval": "100ms",
                "timeout": "2s"
            },
            "server": { "port": port, "discovery_port": null },
            "cover_calibrator": {
                "name": "Deep Sky Dad FP2",
                "unique_id": "dsd-fp2-ui-bdd",
                "description": "BDD test instance",
                "enabled": true,
                "max_brightness": max_brightness
            }
        });
        let path = self.temp_path("dsd-fp2.json");
        std::fs::write(&path, config.to_string()).expect("failed to write driver config");
        path.to_str().unwrap().to_string()
    }

    // --- talking to the driver directly (to build realistic submissions) ---

    /// Call one of the driver's config actions directly, returning the parsed
    /// inner body, or `None` on any transport / ASCOM / decode failure (e.g.
    /// mid-reload). The driver wraps the body as a JSON string in `Value`.
    async fn driver_action(&self, action: &str, parameters: &str) -> Option<Value> {
        let url = format!(
            "http://127.0.0.1:{}{}",
            self.driver_port, DRIVER_ACTION_PATH
        );
        let resp = reqwest::Client::new()
            .put(&url)
            .form(&[
                ("Action", action),
                ("Parameters", parameters),
                ("ClientID", "1"),
                ("ClientTransactionID", "1"),
            ])
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let envelope: Value = resp.json().await.ok()?;
        if envelope.get("ErrorNumber").and_then(Value::as_i64) != Some(0) {
            return None;
        }
        let inner = envelope.get("Value")?.as_str()?;
        serde_json::from_str(inner).ok()
    }

    /// Wait until the freshly-spawned driver answers `config.get`.
    async fn wait_for_driver_ready(&self) {
        for _ in 0..100 {
            if self.driver_action("config.get", "").await.is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("dsd-fp2 driver did not answer config.get within 10s");
    }

    /// The driver's current `config.get` `(config, overrides)` — the exact blob
    /// the BFF embeds in the page's hidden field (`serde_json` is
    /// order-deterministic), so re-submitting it round-trips unchanged.
    async fn driver_config(&self) -> (Value, Vec<String>) {
        let body = self
            .driver_action("config.get", "")
            .await
            .expect("driver config.get failed");
        let config = body
            .get("config")
            .cloned()
            .expect("config.get response missing `config`");
        let overrides = body
            .get("overrides")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        (config, overrides)
    }

    // --- driving the BFF over HTTP ----------------------------------------

    fn ui_url(&self, path: &str) -> String {
        let ui = self.ui.as_ref().expect("BFF not started");
        format!("{}{}", ui.base_url, path)
    }

    /// GET a BFF page and capture the rendered HTML.
    pub async fn get(&mut self, path: &str) {
        let resp = reqwest::Client::new()
            .get(self.ui_url(path))
            .send()
            .await
            .expect("BFF GET failed");
        self.last_body = resp.text().await.unwrap_or_default();
    }

    /// Submit the config form the way the rendered page would, with the given
    /// editable fields overridden. The body starts from the driver's current
    /// `config.get` (the same blob the page embeds in its hidden field), so any
    /// field not listed round-trips unchanged through the BFF's overlay.
    pub async fn submit_form(&mut self, changes: &[(&str, &str)]) {
        let (config, overrides) = self.driver_config().await;

        let mut pairs: Vec<(String, String)> = vec![
            (
                "__config".to_string(),
                serde_json::to_string(&config).expect("serialize config blob"),
            ),
            (
                "__overrides".to_string(),
                serde_json::to_string(&overrides).expect("serialize overrides"),
            ),
        ];
        // `enabled` is read-only in the form (the BFF never overlays it), so a
        // browser submits nothing for it and it round-trips from the hidden blob
        // unchanged — no need to re-assert it here.
        for (name, value) in changes {
            pairs.push(((*name).to_string(), (*value).to_string()));
        }

        let body = serde_urlencoded::to_string(&pairs).expect("encode form body");
        let resp = reqwest::Client::new()
            .post(self.ui_url("/config/dsd-fp2"))
            .header("HX-Request", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .expect("BFF POST failed");
        self.last_body = resp.text().await.unwrap_or_default();
    }

    /// Poll the BFF reconnect endpoint until the *reloaded* driver serves the
    /// given value — matched as the refreshed form's `value="..."` attribute.
    ///
    /// Waiting only for the form to reappear is not enough: until the old
    /// server tears down, it keeps answering `config.get` with the pre-reload
    /// configuration, so the page would briefly re-render with the stale value.
    /// The new value appears only once the rebuilt server is serving.
    pub async fn poll_status_until_value(&mut self, expected: &str) {
        let needle = format!(r#"value="{expected}""#);
        for _ in 0..80 {
            self.get("/config/dsd-fp2/status").await;
            if self.last_body.contains(&needle) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        panic!(
            "driver did not serve {needle} within 20s; last body:\n{}",
            self.last_body
        );
    }

    /// The `<input ...>` tag whose `name` attribute is `name`.
    pub fn input_tag(&self, name: &str) -> String {
        let needle = format!("name=\"{name}\"");
        let pos = self
            .last_body
            .find(&needle)
            .unwrap_or_else(|| panic!("no input named {name:?} in:\n{}", self.last_body));
        let start = self.last_body[..pos]
            .rfind("<input")
            .expect("no <input before name attribute");
        let end = self.last_body[start..]
            .find('>')
            .expect("unterminated input tag")
            + start;
        self.last_body[start..=end].to_string()
    }
}

/// Reserve a free TCP port by binding to `0` and immediately releasing it. The
/// brief window before the driver re-binds is tolerable for tests, and using a
/// concrete port (rather than `0`) is what lets a config reload rebind the
/// *same* port so the BFF can reconnect to the reloaded driver.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("failed to bind a free port")
        .local_addr()
        .expect("failed to read local_addr")
        .port()
}
