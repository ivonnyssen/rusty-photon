//! Real headless-browser session for the `@browser` BDD scenarios — Layer C of
//! the UI-testing plan ([`docs/plans/ui-testing.md`] §6/§9, obligation P3).
//!
//! geckodriver is treated as an external **system tool** (like OmniSim/ConformU):
//! discovered via `GECKODRIVER_BINARY`, else `geckodriver` on `PATH`. It is
//! spawned on an ephemeral port (no 4444 collisions) with `kill_on_drop`, then a
//! headless Firefox is connected through [`thirtyfour`]. Firefox is likewise
//! discovered via `FIREFOX_BINARY` when set, else geckodriver auto-detects the
//! system browser.
//!
//! Teardown is load-bearing (plan §10): [`BrowserSession::quit`] closes the
//! WebDriver session — so geckodriver tears Firefox down — and then kills
//! geckodriver, and it **must run before** the BFF/driver are stopped (a live
//! session holds connections to the BFF open).
//!
//! [`docs/plans/ui-testing.md`]: ../../../../docs/plans/ui-testing.md

use std::time::Duration;

use thirtyfour::prelude::*;
use tokio::process::{Child, Command};

/// A live browser session: the WebDriver client plus the geckodriver child it
/// talks to.
pub struct BrowserSession {
    driver: WebDriver,
    geckodriver: Child,
}

impl std::fmt::Debug for BrowserSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // WebDriver/Child detail is noise in a World dump; keep the field opaque.
        f.debug_struct("BrowserSession").finish_non_exhaustive()
    }
}

impl BrowserSession {
    /// Spawn geckodriver on an ephemeral port and connect a headless Firefox.
    pub async fn start() -> Self {
        let gecko_bin =
            std::env::var("GECKODRIVER_BINARY").unwrap_or_else(|_| "geckodriver".to_string());
        let port = free_port();
        let geckodriver = Command::new(&gecko_bin)
            .arg("--port")
            .arg(port.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn geckodriver ({gecko_bin:?}): {e}"));

        let server = format!("http://127.0.0.1:{port}");
        wait_for_geckodriver(&server).await;

        let mut caps = DesiredCapabilities::firefox();
        caps.set_headless().expect("enable Firefox headless");
        if let Ok(binary) = std::env::var("FIREFOX_BINARY") {
            caps.set_firefox_binary(&binary)
                .expect("set Firefox binary path");
        }

        let driver = WebDriver::new(&server, caps)
            .await
            .expect("connect WebDriver to geckodriver");
        // Implicit wait 0: every browser assertion polls explicitly (a
        // snapshot-once read races the async htmx swap — plan §10).
        driver
            .set_implicit_wait_timeout(Duration::ZERO)
            .await
            .expect("set implicit wait to 0");

        Self {
            driver,
            geckodriver,
        }
    }

    /// Navigate to `url` (top-level; `htmx.min.js` loads and runs).
    pub async fn goto(&self, url: &str) {
        self.driver
            .goto(url)
            .await
            .expect("browser navigation failed");
    }

    /// Poll the **live** DOM (implicit-wait 0, explicit bounded retries) until a
    /// CSS match exists; returns whether it appeared.
    pub async fn wait_present(&self, css: &str, max_iters: usize) -> bool {
        for _ in 0..max_iters {
            if self.driver.find(By::Css(css)).await.is_ok() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    /// Poll until a CSS match exists **and** is enabled; returns success. Used
    /// to observe an htmx swap landing (a disabled field becomes editable).
    pub async fn wait_enabled(&self, css: &str, max_iters: usize) -> bool {
        for _ in 0..max_iters {
            if let Ok(el) = self.driver.find(By::Css(css)).await {
                if el.is_enabled().await.unwrap_or(false) {
                    return true;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    /// Click the first element matching `css`.
    pub async fn click(&self, css: &str) {
        let element = self
            .driver
            .find(By::Css(css))
            .await
            .expect("element to click not found");
        element.click().await.expect("click failed");
    }

    /// Graceful teardown: close the session (geckodriver quits Firefox), then
    /// kill geckodriver. Call **before** stopping the BFF/driver (plan §10).
    pub async fn quit(self) {
        let Self {
            driver,
            mut geckodriver,
        } = self;
        // Closes the WebDriver session; geckodriver tears down its Firefox.
        let _ = driver.quit().await;
        // Belt-and-suspenders: ensure geckodriver itself is gone (kill_on_drop
        // would also fire, but make the wait explicit so no zombie lingers).
        let _ = geckodriver.start_kill();
        let _ = geckodriver.wait().await;
    }
}

/// Reserve an ephemeral loopback port by binding `:0` and reading it back. A
/// tiny TOCTOU window exists before geckodriver rebinds it; negligible on a test
/// host and far cheaper than parsing geckodriver's stdout.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind an ephemeral port")
        .local_addr()
        .expect("read local_addr")
        .port()
}

/// Poll geckodriver's `/status` until it reports ready (bounded ~10s).
async fn wait_for_geckodriver(server: &str) {
    let status_url = format!("{server}/status");
    for _ in 0..100 {
        if let Ok(resp) = reqwest::Client::new().get(&status_url).send().await {
            if resp.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("geckodriver did not become ready at {server} within 10s");
}
