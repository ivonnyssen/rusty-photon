//! OmniSim (ASCOM Alpaca simulator) process management for BDD tests.
//!
//! A single OmniSim process is shared across all scenarios in a test binary
//! via a [`tokio::sync::OnceCell`]. If an OmniSim is already listening on
//! the default port it is reused; otherwise one is spawned.

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
    pub async fn reset_telescope() {
        Self::restart_device("telescope", 0).await;
    }

    /// Reset the camera simulator device 0 to its OmniSim default state.
    pub async fn reset_camera() {
        Self::restart_device("camera", 0).await;
    }

    /// Reset the filter-wheel simulator device 0 to its OmniSim default state.
    pub async fn reset_filter_wheel() {
        Self::restart_device("filterwheel", 0).await;
    }

    /// Reset the focuser simulator device 0 to its OmniSim default state.
    pub async fn reset_focuser() {
        Self::restart_device("focuser", 0).await;
    }

    /// Reset the cover-calibrator simulator device 0 to its OmniSim default state.
    pub async fn reset_cover_calibrator() {
        Self::restart_device("covercalibrator", 0).await;
    }

    /// Reset every device class our BDD suites currently exercise
    /// (telescope, camera, filter wheel, focuser, cover calibrator) to
    /// OmniSim defaults. Issued in parallel — total wall-time is
    /// dominated by a single localhost round-trip.
    ///
    /// Other device classes (dome, rotator, switch, observingconditions,
    /// safetymonitor) also expose `/restart`, but our scenarios don't
    /// touch them yet; add a call here when that changes.
    pub async fn reset_all_devices() {
        tokio::join!(
            Self::reset_telescope(),
            Self::reset_camera(),
            Self::reset_filter_wheel(),
            Self::reset_focuser(),
            Self::reset_cover_calibrator(),
        );
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
    /// Errors are silently ignored: a failed reset shouldn't sink the
    /// scenario; if state pollution caused by a missed reset breaks a
    /// later assertion, the scenario will fail loudly there. The
    /// endpoint is OmniSim-only (not part of standard Alpaca), so
    /// older or alternative simulators may 404 — that's expected and
    /// non-fatal.
    ///
    /// If the shared `OMNISIM` singleton has not been initialised yet
    /// (i.e. no scenario has gone through `OmniSimHandle::start()`),
    /// the request is sent to the default base URL anyway. This lets a
    /// `before(scenario)` hook reset a pre-existing OmniSim that the
    /// suite is about to reuse, eliminating state leakage into the
    /// very first scenario from a prior dev session. Connection
    /// failures (no OmniSim on that port) are non-fatal — see above.
    ///
    /// `class` must match one of OmniSim's device class slugs:
    /// `telescope`, `camera`, `covercalibrator`, `dome`, `filterwheel`,
    /// `focuser`, `observingconditions`, `rotator`, `safetymonitor`,
    /// `switch`.
    pub async fn restart_device(class: &str, n: u32) {
        let base_url = OMNISIM
            .get()
            .map(|p| p.base_url.clone())
            .unwrap_or_else(|| format!("http://127.0.0.1:{}", OMNISIM_PORT));
        let url = format!("{}/simulator/v1/{}/{}/restart", base_url, class, n);
        let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
        else {
            return;
        };
        let _ = client.put(&url).send().await;
    }
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
