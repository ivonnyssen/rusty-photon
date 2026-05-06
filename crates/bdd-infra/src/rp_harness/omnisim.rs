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
    /// Errors are collected and returned. **Exception:** when the
    /// shared `OMNISIM` singleton has not been initialised yet (i.e.
    /// no scenario has gone through `OmniSimHandle::start()`), errors
    /// are non-fatal — the PUTs still fire against the default
    /// `127.0.0.1:32323` URL so a pre-existing OmniSim from a prior
    /// dev session is reset before scenario 1 reuses it, but
    /// connection-refused (no OmniSim there yet) is the expected case
    /// and we don't want to panic the very first before-hook over it.
    /// Once the suite has called `OmniSimHandle::start()`, every reset
    /// failure is fatal — that's the loud-reset behaviour from #172
    /// that catches state leakage between scenarios.
    ///
    /// Other device classes (dome, rotator, switch, observingconditions,
    /// safetymonitor) also expose `/restart`, but our scenarios don't
    /// touch them yet; add a call here when that changes.
    pub async fn reset_all_devices() -> Result<(), Vec<String>> {
        let omnisim_was_started = OMNISIM.get().is_some();
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
        if errors.is_empty() || !omnisim_was_started {
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
    /// Returns `Err(_)` on any non-success response or transport error,
    /// with a string suitable for inclusion in a panic message. The
    /// endpoint is OmniSim-only (not part of standard Alpaca), so
    /// older or alternative simulators may 404 — those are surfaced as
    /// errors today; we run only against OmniSim and want to know if
    /// that ever changes. Errors used to be silently swallowed here,
    /// which masked intermittent macOS failures.
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
        let url = format!("{}/simulator/v1/{}/{}/restart", base_url, class, n);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| format!("reqwest client build failed: {e}"))?;
        let resp = client
            .put(&url)
            .send()
            .await
            .map_err(|e| format!("PUT {url} failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("PUT {url} returned HTTP {}", resp.status()));
        }
        Ok(())
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

        // Capture OmniSim's stdout/stderr to per-run log files under the
        // cargo target tree. The previous `Stdio::null()` dropped every
        // line OmniSim emitted, which left CI failures with no insight
        // into what the simulator was doing — see #171 for the
        // diagnostic gap. Failures here fall back to `Stdio::null` so a
        // log-write problem can't stop the test suite from running.
        let (stdout_target, stderr_target) = Self::open_log_files();

        // Clear sanitizer-related env vars so the .NET runtime isn't broken
        // by LD_PRELOAD injection from ASAN/LSAN.
        let mut cmd = std::process::Command::new(&binary);
        cmd.stdout(stdout_target)
            .stderr(stderr_target)
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

    /// Resolve the log directory for OmniSim's captured stdout/stderr.
    /// Lives at `<CARGO_TARGET_DIR>/bdd-infra-omnisim-logs/` (or
    /// `<workspace>/target/bdd-infra-omnisim-logs/` if unset). Kept
    /// outside the seeded XDG dir so `prepare_xdg_config_home`'s
    /// `remove_dir_all` can't sweep the previous run's logs.
    ///
    /// Returns `None` (caller falls back to `Stdio::null`) only if the
    /// directory can't be created.
    fn log_dir() -> Option<PathBuf> {
        let target_dir = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .and_then(|p| p.parent())
                    .map(|workspace| workspace.join("target"))
                    .unwrap_or_else(|| PathBuf::from("target"))
            });
        let dest = target_dir.join("bdd-infra-omnisim-logs");
        std::fs::create_dir_all(&dest).ok()?;
        Some(dest)
    }

    /// Open fresh (truncating) log files for OmniSim's stdout and
    /// stderr, returning `Stdio` handles ready to attach to the
    /// `Command`. Falls back to `Stdio::null()` for either stream
    /// individually if its file can't be opened.
    fn open_log_files() -> (Stdio, Stdio) {
        let dir = Self::log_dir();
        let stdout = dir
            .as_ref()
            .and_then(|d| std::fs::File::create(d.join("omnisim.stdout.log")).ok())
            .map(Stdio::from)
            .unwrap_or_else(Stdio::null);
        let stderr = dir
            .as_ref()
            .and_then(|d| std::fs::File::create(d.join("omnisim.stderr.log")).ok())
            .map(Stdio::from)
            .unwrap_or_else(Stdio::null);
        (stdout, stderr)
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
