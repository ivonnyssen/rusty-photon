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

/// Process-wide serialization of `/restart` PUTs. OmniSim's restart
/// handler (`DriverManager.Load{Class}(n)`) mutates unsynchronised
/// process-wide static state, so concurrent restarts race inside the
/// simulator. `reset_all_devices` already issues its 5 PUTs
/// sequentially (#171), but that only serialises *within* one hook —
/// cucumber runs untagged scenarios concurrently, and every
/// concurrently-drawn scenario runs its own before-hook. In the
/// pi-nightly failure behind #431 the 11 non-`@serial` rp scenarios
/// were all drawn at once after the `@serial` queue drained, their 11
/// hooks issued ~11 concurrent restarts per device class, and OmniSim
/// deadlocked mid-wave (log torn then silent, no stderr, subsequent
/// PUTs timing out) — failing the remaining hooks loud. Holding this
/// mutex across each PUT caps in-flight restarts at one per test
/// process, which removes the trigger. Known limitation: it cannot
/// serialise across *processes* (e.g. two Bazel bdd actions sharing
/// one OmniSim on port 32323).
static RESTART_SERIALIZER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    /// OmniSim defaults. Issued **sequentially** — one PUT at a time.
    ///
    /// Why not parallel? OmniSim's `DriverManager.Load{Class}(n)`
    /// mutates a process-wide `static List<AlpacaConfiguredDevice>
    /// AlpacaDevices` via unsynchronised `List.Remove(...)` +
    /// `List.Add(...)`. When two of our 5 PUTs landed on different
    /// Kestrel threads they raced inside that list, leaving a `null`
    /// entry that the management endpoint then serialised verbatim
    /// into `configureddevices` responses. rp's deserialiser hit
    /// `invalid type: null, expected struct ConfiguredDevice` and
    /// silently registered the device as disconnected — which is the
    /// camera/calibrator/focuser "not connected" cascade in #171.
    /// Sequential PUTs eliminate that race *within* one hook; the
    /// process-wide [`RESTART_SERIALIZER`] taken inside each PUT
    /// eliminates it *across* concurrently-running hooks too — the
    /// end-of-run burst of non-`@serial` scenarios deadlocked OmniSim
    /// on the Pi nightly (#431).
    ///
    /// The wall-time cost is small: 5 localhost round-trips serialised
    /// is ~10-25 ms per scenario depending on runner.
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
        let mut errors: Vec<String> = Vec::new();
        let results = [
            Self::reset_telescope().await,
            Self::reset_camera().await,
            Self::reset_filter_wheel().await,
            Self::reset_focuser().await,
            Self::reset_cover_calibrator().await,
        ];
        for result in results {
            if let Err(e) = result {
                errors.push(e);
            }
        }
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
        Self::restart_device_at(&base_url, class, n).await
    }

    /// `restart_device` extracted to take an explicit `base_url` so unit
    /// tests can drive the HTTP path against an axum stub without
    /// touching the global `OMNISIM` singleton. See the `tests` module
    /// at the bottom of this file.
    ///
    /// The PUT is issued under [`RESTART_SERIALIZER`], so at most one
    /// restart is in flight per test process no matter how many
    /// scenario hooks run concurrently — see the mutex docs for the
    /// OmniSim deadlock (#431) this prevents.
    async fn restart_device_at(base_url: &str, class: &str, n: u32) -> Result<(), String> {
        let url = format!("{}/simulator/v1/{}/{}/restart", base_url, class, n);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| format!("reqwest client build failed: {e}"))?;
        // Lock only around the request itself — client construction and
        // URL formatting don't touch OmniSim and would just lengthen the
        // critical section when many hooks queue here.
        let _serialized = RESTART_SERIALIZER.lock().await;
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
        // Under Bazel there is no cargo target tree and `CARGO_MANIFEST_DIR` is a
        // compile-time sandbox path, so the cargo branch below resolves to a
        // directory that can't be created at test runtime — OmniSim's logs would
        // silently go to `Stdio::null` and a CI crash would leave no trace (the
        // #171 diagnostic gap, recurring under Bazel). Bazel sets
        // `TEST_UNDECLARED_OUTPUTS_DIR` for test actions; files written there are
        // collected under `bazel-testlogs/.../test.outputs`. Prefer it.
        if let Some(undeclared) = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR") {
            let dest = PathBuf::from(undeclared).join("omnisim-logs");
            if std::fs::create_dir_all(&dest).is_ok() {
                return Some(dest);
            }
        }
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
    ///
    /// File names embed the BDD test binary's PID so concurrent runs
    /// (e.g. `cargo test --workspace --test bdd`, where each package's
    /// BDD binary is a separate process sharing one `CARGO_TARGET_DIR`)
    /// don't truncate each other's logs. On Windows, file-locking on a
    /// shared name would also fail one of the spawns outright; the PID
    /// suffix avoids that.
    fn open_log_files() -> (Stdio, Stdio) {
        let dir = Self::log_dir();
        let pid = std::process::id();
        let stdout = dir
            .as_ref()
            .and_then(|d| std::fs::File::create(d.join(format!("omnisim.{pid}.stdout.log"))).ok())
            .map(Stdio::from)
            .unwrap_or_else(Stdio::null);
        // Under Bazel, inherit OmniSim's stderr into the test process so a
        // crash / unhandled exception (the cause of the rp:bdd / calibrator-flats
        // OmniSim cascades) shows up in the failed test output (`--test_output=errors`)
        // in the CI job log — the TEST_UNDECLARED_OUTPUTS_DIR files aren't uploaded
        // by the bazel workflow today, and the flake doesn't reproduce locally.
        // stdout stays filed: OmniSim's per-request logging is too chatty to inherit.
        let stderr = if std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR").is_some() {
            Stdio::inherit()
        } else {
            dir.as_ref()
                .and_then(|d| {
                    std::fs::File::create(d.join(format!("omnisim.{pid}.stderr.log"))).ok()
                })
                .map(Stdio::from)
                .unwrap_or_else(Stdio::null)
        };
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
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    use axum::http::StatusCode;
    use axum::routing::{get, put};
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

    /// Stub server that responds to `PUT /simulator/v1/{class}/{n}/restart`
    /// with the given `status`. The route is registered at the exact
    /// `class`/`n` the test will hit, so a request to a different
    /// device falls through to a 404 (which is what `restart_device`
    /// will surface as an error — useful for one of the tests below).
    async fn spawn_restart_stub(
        class: &str,
        n: u32,
        status: StatusCode,
    ) -> (String, tokio::sync::oneshot::Sender<()>) {
        let route = format!("/simulator/v1/{class}/{n}/restart");
        let app = Router::new().route(&route, put(move || async move { status }));
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

    #[tokio::test]
    async fn restart_device_returns_ok_on_success() {
        let (base_url, shutdown) = spawn_restart_stub("camera", 0, StatusCode::OK).await;
        let result = OmniSimHandle::restart_device_at(&base_url, "camera", 0).await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        let _ = shutdown.send(());
    }

    #[tokio::test]
    async fn restart_device_returns_err_on_404() {
        // Stub registers /camera/0/restart but the test hits /telescope/0/restart.
        let (base_url, shutdown) = spawn_restart_stub("camera", 0, StatusCode::OK).await;
        let err = OmniSimHandle::restart_device_at(&base_url, "telescope", 0)
            .await
            .expect_err("expected an error for unrouted path");
        assert!(err.contains("404"), "expected 404 in error: {err}");
        let _ = shutdown.send(());
    }

    #[tokio::test]
    async fn restart_device_returns_err_on_server_error() {
        let (base_url, shutdown) =
            spawn_restart_stub("camera", 0, StatusCode::INTERNAL_SERVER_ERROR).await;
        let err = OmniSimHandle::restart_device_at(&base_url, "camera", 0)
            .await
            .expect_err("expected an error for 500 response");
        assert!(err.contains("500"), "expected 500 in error: {err}");
        let _ = shutdown.send(());
    }

    #[tokio::test]
    async fn restart_device_serializes_concurrent_restarts() {
        use std::sync::atomic::{AtomicI32, Ordering};
        use std::sync::Arc;

        // Stub that records whether two restart requests were ever in
        // flight at the same time. Each handler bumps an in-flight
        // counter, holds the request open briefly, then decrements —
        // without RESTART_SERIALIZER, 16 concurrent PUTs overlap here
        // reliably (this test failed before the mutex was added).
        let in_flight = Arc::new(AtomicI32::new(0));
        let overlapped = Arc::new(AtomicI32::new(0));
        let (in_flight_h, overlapped_h) = (in_flight.clone(), overlapped.clone());
        let app = Router::new().route(
            "/simulator/v1/camera/0/restart",
            put(move || {
                let in_flight = in_flight_h.clone();
                let overlapped = overlapped_h.clone();
                async move {
                    if in_flight.fetch_add(1, Ordering::SeqCst) > 0 {
                        overlapped.fetch_add(1, Ordering::SeqCst);
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    StatusCode::OK
                }
            }),
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
        let base_url = format!("http://127.0.0.1:{port}");

        let puts: Vec<_> = (0..16)
            .map(|_| {
                let base_url = base_url.clone();
                tokio::spawn(async move {
                    OmniSimHandle::restart_device_at(&base_url, "camera", 0).await
                })
            })
            .collect();
        for put in puts {
            put.await.unwrap().unwrap();
        }
        assert_eq!(
            overlapped.load(Ordering::SeqCst),
            0,
            "restart PUTs overlapped despite RESTART_SERIALIZER"
        );
        let _ = tx.send(());
    }

    #[tokio::test]
    async fn restart_device_returns_err_when_connection_refused() {
        // Bind a listener to grab a free port, then drop it so subsequent
        // connects refuse — mirrors the is_healthy_returns_false test.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let base_url = format!("http://127.0.0.1:{port}");
        let err = OmniSimHandle::restart_device_at(&base_url, "camera", 0)
            .await
            .expect_err("expected a transport error");
        assert!(
            err.starts_with("PUT ") && err.contains("failed"),
            "unexpected transport error format: {err}"
        );
    }
}
