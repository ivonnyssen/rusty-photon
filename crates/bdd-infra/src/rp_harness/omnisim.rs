//! OmniSim (ASCOM Alpaca simulator) process management for BDD tests.
//!
//! A single OmniSim process is shared across all scenarios in a test binary
//! via a [`tokio::sync::OnceCell`]. If an OmniSim is already listening on
//! the default port it is reused; otherwise one is spawned.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::sync::OnceCell;

/// OmniSim's default HTTP port when launched without `--urls`.
const OMNISIM_PORT: u16 = 32323;

/// Shared OmniSim info returned to each scenario.
#[derive(Debug, Clone)]
pub struct OmniSimHandle {
    pub base_url: String,
    pub port: u16,
}

/// Singleton that owns the OmniSim child process for the entire test run.
/// When `_child` is `None`, a pre-existing OmniSim was reused and we don't
/// own the process.
struct OmniSimProcess {
    _child: Option<std::process::Child>,
    base_url: String,
    port: u16,
}

/// Global singleton — one OmniSim process shared by all scenarios.
static OMNISIM: OnceCell<OmniSimProcess> = OnceCell::const_new();

impl OmniSimHandle {
    /// Get or start the shared OmniSim process. Returns a lightweight handle.
    ///
    /// If an OmniSim instance is already listening on the default port
    /// (32323), it is reused. Otherwise a new process is spawned with
    /// `PR_SET_PDEATHSIG` on Linux so the kernel kills it when the test
    /// process exits.
    ///
    /// Binary discovery order (when spawning):
    /// 1. `OMNISIM_PATH` env var — full path to the binary
    /// 2. `OMNISIM_DIR` env var — directory containing the binary
    /// 3. `ascom.alpaca.simulators` on `PATH`
    pub async fn start() -> Self {
        let process = OMNISIM
            .get_or_init(|| async { OmniSimProcess::get_or_spawn().await })
            .await;
        Self {
            base_url: process.base_url.clone(),
            port: process.port,
        }
    }

    /// Reset the telescope simulator device 0 to its OmniSim default state.
    /// See [`Self::restart_device`] for the underlying mechanism.
    pub async fn reset_telescope() -> Result<(), String> {
        Self::restart_device("telescope", 0).await
    }

    /// Reset the camera simulator device 0 to its OmniSim default state.
    pub async fn reset_camera() -> Result<(), String> {
        Self::restart_device("camera", 0).await
    }

    /// Reset the filter-wheel simulator device 0 to its OmniSim default state.
    pub async fn reset_filter_wheel() -> Result<(), String> {
        Self::restart_device("filterwheel", 0).await
    }

    /// Reset the focuser simulator device 0 to its OmniSim default state.
    pub async fn reset_focuser() -> Result<(), String> {
        Self::restart_device("focuser", 0).await
    }

    /// Reset the cover-calibrator simulator device 0 to its OmniSim default state.
    pub async fn reset_cover_calibrator() -> Result<(), String> {
        Self::restart_device("covercalibrator", 0).await
    }

    /// Reset every device class our BDD suites currently exercise
    /// (telescope, camera, filter wheel, focuser, cover calibrator) to
    /// OmniSim defaults. Issued in parallel — total wall-time is
    /// dominated by a single localhost round-trip.
    ///
    /// Returns `Ok(())` when no scenario has yet started OmniSim — the
    /// `before(scenario)` hook fires for the very first scenario before
    /// `OmniSimHandle::start()` has spawned anything, and at that point
    /// there is nothing to reset. Otherwise, returns the collected
    /// error messages from any per-device reset that didn't return 2xx.
    /// Errors used to be silently swallowed here, which masked
    /// intermittent macOS reset failures and let state leak between
    /// scenarios — see PR #172 / `bug/bdd-investigation`.
    ///
    /// Other device classes (dome, rotator, switch, observingconditions,
    /// safetymonitor) also expose `/restart`, but our scenarios don't
    /// touch them yet; add a call here when that changes.
    pub async fn reset_all_devices() -> Result<(), Vec<String>> {
        if OMNISIM.get().is_none() {
            return Ok(());
        }
        let (telescope, camera, filter_wheel, focuser, cover_calibrator) = tokio::join!(
            Self::reset_telescope(),
            Self::reset_camera(),
            Self::reset_filter_wheel(),
            Self::reset_focuser(),
            Self::reset_cover_calibrator(),
        );
        let errors: Vec<String> = [telescope, camera, filter_wheel, focuser, cover_calibrator]
            .into_iter()
            .filter_map(Result::err)
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Reset a single OmniSim device by class and instance number to
    /// its default state without restarting the simulator process.
    ///
    /// Posts to OmniSim's private `PUT /simulator/v1/{class}/{n}/restart`
    /// endpoint, which calls `DriverManager.Load{Class}(n)` server-side.
    /// The result is equivalent to OmniSim having just started for that
    /// device — e.g. for telescope: AtPark false, Tracking false,
    /// position at the configured startup alt/az (default ≈ alt 38.9°
    /// az 165° — above horizon).
    ///
    /// `DriverManager.Load{Class}(n)` runs server-side and the PUT can
    /// return 200 before the reload has finished. We then poll the
    /// standard Alpaca `GET /api/v1/{class}/{n}/connected` until it
    /// returns 2xx with `Value: false` — that's the post-reload default
    /// state and confirms the device is ready for the next scenario's
    /// rp startup to call `set_connected(true)` against it. Without
    /// this wait, the next rp's `connect_camera` races the reload,
    /// silently fails, and downstream test steps panic with "camera
    /// not connected: main-cam" (#171).
    ///
    /// Returns `Err(_)` on any non-success response, transport error,
    /// or if the reload doesn't complete within the polling timeout.
    /// Errors used to be silently swallowed here, which masked
    /// intermittent macOS / Windows failures.
    ///
    /// `class` must match one of OmniSim's device class slugs:
    /// `telescope`, `camera`, `covercalibrator`, `dome`, `filterwheel`,
    /// `focuser`, `observingconditions`, `rotator`, `safetymonitor`,
    /// `switch`.
    pub async fn restart_device(class: &str, n: u32) -> Result<(), String> {
        let base_url = OMNISIM
            .get()
            .map(|p| p.base_url.clone())
            .unwrap_or_else(|| format!("http://127.0.0.1:{}", OMNISIM_PORT));
        let restart_url = format!("{}/simulator/v1/{}/{}/restart", base_url, class, n);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| format!("reqwest client build failed: {e}"))?;
        let resp = client
            .put(&restart_url)
            .send()
            .await
            .map_err(|e| format!("PUT {restart_url} failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("PUT {restart_url} returned HTTP {}", resp.status()));
        }
        wait_for_device_reloaded(&client, &base_url, class, n).await
    }
}

/// Poll the standard Alpaca `GET /api/v1/{class}/{n}/connected` until
/// it returns 2xx with `Value: false`. Called after a successful
/// `PUT /simulator/v1/{class}/{n}/restart` to wait for OmniSim's
/// async device reload to finish. See [`OmniSimHandle::restart_device`]
/// for the full race description.
async fn wait_for_device_reloaded(
    client: &reqwest::Client,
    base_url: &str,
    class: &str,
    n: u32,
) -> Result<(), String> {
    let url = format!("{}/api/v1/{}/{}/connected", base_url, class, n);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut last_observation = String::from("never received a response");
    while std::time::Instant::now() < deadline {
        match client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    match resp.json::<serde_json::Value>().await {
                        Ok(json) => match json.get("Value").and_then(serde_json::Value::as_bool) {
                            Some(false) => return Ok(()),
                            Some(true) => {
                                last_observation =
                                    "Value=true (reload not yet visible)".to_string();
                            }
                            None => {
                                last_observation =
                                    format!("missing/non-bool Value field in body: {json}");
                            }
                        },
                        Err(e) => {
                            last_observation = format!("response body not JSON: {e}");
                        }
                    }
                } else {
                    last_observation = format!("HTTP {status}");
                }
            }
            Err(e) => {
                last_observation = format!("transport error: {e}");
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(format!(
        "{} did not return Value:false within 5s after restart (last: {})",
        url, last_observation
    ))
}

impl OmniSimProcess {
    async fn get_or_spawn() -> Self {
        let base_url = format!("http://127.0.0.1:{}", OMNISIM_PORT);

        if Self::is_healthy(&base_url).await {
            return Self {
                _child: None,
                base_url,
                port: OMNISIM_PORT,
            };
        }

        let binary = Self::find_binary();

        // Clear sanitizer-related env vars so the .NET runtime isn't broken
        // by LD_PRELOAD injection from ASAN/LSAN.
        let mut cmd = std::process::Command::new(&binary);
        cmd.stdout(Stdio::null())
            .stderr(Stdio::null())
            .env_remove("LD_PRELOAD")
            .env_remove("ASAN_OPTIONS")
            .env_remove("LSAN_OPTIONS");

        // Point OmniSim at a per-build XDG_CONFIG_HOME under cargo's
        // target dir, seeded from the checked-in profile templates in
        // `crates/bdd-infra/omnisim-config/...`. The seed copy is the
        // ONLY runtime write involved — subsequent OmniSim startup
        // writes (UniqueID emission, full-profile persistence) all
        // land in the per-build dir and never touch the checked-in
        // source. Failures here are non-fatal: if the seed dir can't
        // be prepared we just skip the override, OmniSim falls back
        // to the user's `$HOME/.config/...`, and tests run with
        // upstream defaults instead of our tuned ones.
        if let Some(xdg) = Self::prepare_xdg_config_home() {
            cmd.env("XDG_CONFIG_HOME", xdg);
        }

        // On Linux, set PR_SET_PDEATHSIG so the kernel will SIGKILL this
        // child when the test process exits (normal, panic, or SIGKILL).
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(|| {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                    Ok(())
                });
            }
        }

        let child = cmd
            .spawn()
            .unwrap_or_else(|e| panic!("failed to start OmniSim binary '{}': {}", binary, e));

        let process = Self {
            _child: Some(child),
            base_url,
            port: OMNISIM_PORT,
        };
        process.wait_healthy().await;
        process
    }

    /// Seed a per-build `XDG_CONFIG_HOME` for OmniSim and return its
    /// path. The seed is a recursive copy of the checked-in
    /// `crates/bdd-infra/omnisim-config/...` tree. OmniSim writes back
    /// to this directory on startup (e.g. emitting missing UniqueIDs,
    /// persisting full default profiles), so we MUST copy the source
    /// into a target-tree location and never let OmniSim see the
    /// repository copy directly.
    ///
    /// The destination lives under `$CARGO_TARGET_DIR/bdd-infra-omnisim/`
    /// (or `target/bdd-infra-omnisim/` if `CARGO_TARGET_DIR` is unset),
    /// so it ends up in the same place `cargo clean` already reaches.
    /// We fully reseed on every spawn so an OmniSim write-back from a
    /// prior run can't leak into the next one.
    ///
    /// Returns `None` (and the caller proceeds without the override) on
    /// any I/O failure — the simulator still works, just with upstream
    /// defaults.
    fn prepare_xdg_config_home() -> Option<PathBuf> {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("omnisim-config");
        let target_dir = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .and_then(|p| p.parent())
                    .map(|workspace| workspace.join("target"))
                    .unwrap_or_else(|| PathBuf::from("target"))
            });
        let dest = target_dir.join("bdd-infra-omnisim");
        // Wipe whatever a prior run left behind so an OmniSim write-back
        // from then can't survive into this run's profile.
        let _ = std::fs::remove_dir_all(&dest);
        Self::copy_dir_recursive(&src, &dest).ok()?;
        Some(dest)
    }

    fn copy_dir_recursive(src: &Path, dest: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dest)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let from = entry.path();
            let to = dest.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                Self::copy_dir_recursive(&from, &to)?;
            } else {
                std::fs::copy(&from, &to)?;
            }
        }
        Ok(())
    }

    fn find_binary() -> String {
        if let Ok(path) = std::env::var("OMNISIM_PATH") {
            return path;
        }

        let binary_name = if cfg!(target_os = "windows") {
            "ascom.alpaca.simulators.exe"
        } else {
            "ascom.alpaca.simulators"
        };

        if let Ok(dir) = std::env::var("OMNISIM_DIR") {
            let path = std::path::Path::new(&dir).join(binary_name);
            return path.to_string_lossy().to_string();
        }

        binary_name.to_string()
    }

    async fn is_healthy(base_url: &str) -> bool {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("failed to build reqwest client");
        let url = format!("{}/api/v1/camera/0/connected", base_url);
        matches!(client.get(&url).send().await, Ok(resp) if resp.status().is_success())
    }

    async fn wait_healthy(&self) {
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if Self::is_healthy(&self.base_url).await {
                return;
            }
        }
        panic!("OmniSim did not become healthy within 30 seconds");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::http::StatusCode;
    use axum::routing::get;
    use axum::Router;

    async fn spawn_stub(status: StatusCode) -> (String, tokio::sync::oneshot::Sender<()>) {
        let app = Router::new().route(
            "/api/v1/camera/0/connected",
            get(move || async move { status }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await
                .unwrap();
        });
        (format!("http://127.0.0.1:{}", port), tx)
    }

    #[tokio::test]
    async fn is_healthy_returns_true_on_success() {
        let (base_url, shutdown) = spawn_stub(StatusCode::OK).await;
        assert!(OmniSimProcess::is_healthy(&base_url).await);
        let _ = shutdown.send(());
    }

    #[tokio::test]
    async fn is_healthy_returns_false_on_server_error() {
        let (base_url, shutdown) = spawn_stub(StatusCode::INTERNAL_SERVER_ERROR).await;
        assert!(!OmniSimProcess::is_healthy(&base_url).await);
        let _ = shutdown.send(());
    }

    #[tokio::test]
    async fn is_healthy_returns_false_when_connection_refused() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let base_url = format!("http://127.0.0.1:{}", port);
        assert!(!OmniSimProcess::is_healthy(&base_url).await);
    }
}
