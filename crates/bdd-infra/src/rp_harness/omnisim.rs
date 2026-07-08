//! OmniSim (ASCOM Alpaca simulator) process management for BDD tests.
//!
//! A single OmniSim process is shared across all scenarios in a test binary
//! via a [`tokio::sync::OnceCell`]. Each test process spawns its **own**
//! instance on a **dynamically chosen port**, passing `--multi-instance` —
//! the flag added to our OmniSim fork (`ivonnyssen/ASCOM.Alpaca.Simulators`,
//! release `v0.5.0-467.1`) that skips upstream's machine-global
//! single-instance guard (a named Mutex keyed on a fixed GUID, backed by a
//! file under `/tmp/.dotnet/shm/` on Unix). Combined with a per-instance
//! settings dir (see [`OmniSimProcess::prepare_settings_dir`]), any number
//! of BDD test processes — parallel Bazel targets, shards of one target, or a
//! stray dev instance on the default port — can run concurrently without
//! contending for one simulator. This is what lets Bazel run the
//! OmniSim-backed suites in parallel and shard `rp:bdd` (issue #467).
//!
//! The settings dir is passed via `OMNISIM_SETTINGS_DIR` (fork release
//! `v0.5.0-467.2`, the version floor), NOT `XDG_CONFIG_HOME`: .NET honors
//! XDG only on non-macOS Unix, so on macOS OmniSim's profile store defaults
//! to the shared `~/Library/Application Support` (and on Windows to
//! `%USERPROFILE%\.ASCOM`), neither redirectable by any environment
//! variable. Concurrent instances sharing one profile store race their
//! startup write-backs and leak persisted *settings* across suites — on
//! macOS CI, session-runner's computed telescope site leaked into rp's
//! shards through per-scenario `restart` (which reloads from the profile)
//! and rp refused to start on mount-site validation. The fork's env var
//! bypasses the platform lookup entirely, so isolation is deterministic on
//! every OS and the Bazel `omnisim` pool runs parallel everywhere.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::sync::OnceCell;

/// Attempts to spawn OmniSim before giving up. Each attempt picks a fresh
/// ephemeral port, so a lost bind race (another process grabbed the port
/// between our probe and OmniSim's bind) just costs one retry.
const SPAWN_ATTEMPTS: u32 = 3;

/// Shared OmniSim info returned to each scenario.
#[derive(Debug, Clone)]
pub struct OmniSimHandle {
    pub base_url: String,
    pub port: u16,
}

/// Singleton that owns the OmniSim child process for the entire test run.
struct OmniSimProcess {
    _child: std::process::Child,
    base_url: String,
    port: u16,
}

/// Global singleton — one OmniSim process shared by all scenarios.
static OMNISIM: OnceCell<OmniSimProcess> = OnceCell::const_new();

/// Process-wide serialization of `/restart` PUTs. OmniSim's restart
/// handler (`DriverManager.Load{Class}(n)`) mutates unsynchronised
/// process-wide static state, so concurrent restarts race inside the
/// simulator. `reset_all_devices` already issues its per-device PUTs
/// sequentially (#171), but that only serialises *within* one hook —
/// cucumber runs untagged scenarios concurrently, and every
/// concurrently-drawn scenario runs its own before-hook. In the
/// pi-nightly failure behind #431 the 11 non-`@serial` rp scenarios
/// were all drawn at once after the `@serial` queue drained, their 11
/// hooks issued ~11 concurrent restarts per device class, and OmniSim
/// deadlocked mid-wave (log torn then silent, no stderr, subsequent
/// PUTs timing out) — failing the remaining hooks loud. Holding this
/// mutex across each PUT caps in-flight restarts at one per test
/// process. A process-wide mutex is also *sufficient* now: every test
/// process owns a private OmniSim instance (`--multi-instance` +
/// dynamic port), so no other process can send restarts to ours — the
/// old cross-process caveat about two Bazel actions sharing one
/// OmniSim on port 32323 no longer applies.
static RESTART_SERIALIZER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

impl OmniSimHandle {
    /// Get or start this process's private OmniSim. Returns a lightweight
    /// handle.
    ///
    /// The first call spawns a fresh instance with `--multi-instance` on a
    /// dynamically chosen `127.0.0.1` port (with `PR_SET_PDEATHSIG` on Linux
    /// so the kernel kills it when the test process exits); subsequent calls
    /// share it. A pre-existing OmniSim — a dev instance on the default port,
    /// or another test process's instance — is never reused: private
    /// instances are what allow OmniSim-backed suites and shards to run
    /// concurrently, and what stopped cross-session dev instances from
    /// contending with test runs.
    ///
    /// Binary discovery order:
    /// 1. `OMNISIM_PATH` env var — full path to the binary
    /// 2. `OMNISIM_DIR` env var — directory containing the binary
    /// 3. `ascom.alpaca.simulators` on `PATH`
    ///
    /// The binary must support `--multi-instance` and `OMNISIM_SETTINGS_DIR`
    /// (fork release `v0.5.0-467.2` or newer). An older binary either exits
    /// immediately when another instance is running (pre-467.1, surfaces
    /// here as a spawn failure naming the flag) or silently ignores the
    /// settings-dir override and shares the platform-default profile store
    /// with every other instance (467.1 on macOS/Windows).
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

    /// Reset the safety-monitor simulator device 0 to its OmniSim default
    /// state. `restart` reloads the device from its persisted profile, and
    /// [`Self::set_safety_monitor_is_safe`] writes only the in-memory
    /// setting — so this restores the profile's `IsSafe` (true in our
    /// seeded config) after a safety scenario flipped it.
    pub async fn reset_safety_monitor() -> Result<(), String> {
        Self::restart_device("safetymonitor", 0).await
    }

    /// Reset every device class our BDD suites currently exercise
    /// (telescope, camera, filter wheel, focuser, cover calibrator,
    /// safety monitor) to OmniSim defaults. Issued **sequentially** —
    /// one PUT at a time.
    ///
    /// Why not parallel? OmniSim's `DriverManager.Load{Class}(n)`
    /// mutates a process-wide `static List<AlpacaConfiguredDevice>
    /// AlpacaDevices` via unsynchronised `List.Remove(...)` +
    /// `List.Add(...)`. When two of our PUTs landed on different
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
    /// The wall-time cost is small: 6 localhost round-trips serialised
    /// is ~10-30 ms per scenario depending on runner.
    ///
    /// When the shared `OMNISIM` singleton has not been initialised yet
    /// (no scenario has gone through `OmniSimHandle::start()`), this is
    /// a no-op: there is no instance to reset, and the one `start()`
    /// will eventually spawn is fresh by construction. (The pre-#467
    /// behaviour of firing best-effort PUTs at the default port to
    /// scrub a reusable dev instance is gone along with reuse itself.)
    /// Once the suite has called `OmniSimHandle::start()`, every reset
    /// failure is fatal — that's the loud-reset behaviour from #172
    /// that catches state leakage between scenarios.
    ///
    /// Other device classes (dome, rotator, switch, observingconditions)
    /// also expose `/restart`, but our scenarios don't touch them yet;
    /// add a call here when that changes.
    pub async fn reset_all_devices() -> Result<(), Vec<String>> {
        if OMNISIM.get().is_none() {
            return Ok(());
        }
        let mut errors: Vec<String> = Vec::new();
        let results = [
            Self::reset_telescope().await,
            Self::reset_camera().await,
            Self::reset_filter_wheel().await,
            Self::reset_focuser().await,
            Self::reset_cover_calibrator().await,
            Self::reset_safety_monitor().await,
        ];
        for result in results {
            if let Err(e) = result {
                errors.push(e);
            }
        }
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
        let base_url = Self::singleton_base_url().await;
        Self::restart_device_at(&base_url, class, n).await
    }

    /// Set what the safety-monitor simulator device 0 reports for
    /// `IsSafe`, via OmniSim's private
    /// `PUT /simulator/v1/safetymonitor/{n}/issafesetting` endpoint.
    ///
    /// This writes the device's in-memory setting only (OmniSim persists
    /// it to the profile on its own save path, which this endpoint does
    /// not trigger), so [`Self::reset_safety_monitor`] — or the next
    /// process restart — restores the profile default (safe). Safety
    /// scenarios still set `true` explicitly during setup so they never
    /// depend on reset ordering.
    pub async fn set_safety_monitor_is_safe(is_safe: bool) -> Result<(), String> {
        let base_url = Self::singleton_base_url().await;
        Self::set_safety_monitor_is_safe_at(&base_url, 0, is_safe).await
    }

    /// `set_safety_monitor_is_safe` extracted to take an explicit
    /// `base_url` and device number so unit tests can drive the HTTP
    /// path against an axum stub without touching the global `OMNISIM`
    /// singleton.
    async fn set_safety_monitor_is_safe_at(
        base_url: &str,
        n: u32,
        is_safe: bool,
    ) -> Result<(), String> {
        let url = format!(
            "{}/simulator/v1/safetymonitor/{}/issafesetting",
            base_url, n
        );
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| format!("reqwest client build failed: {e}"))?;
        let resp = client
            .put(&url)
            .form(&[("IsSafeSetting", if is_safe { "true" } else { "false" })])
            .send()
            .await
            .map_err(|e| format!("PUT {url} failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("PUT {url} returned HTTP {}", resp.status()));
        }
        Ok(())
    }

    /// Set the telescope simulator's observer site (`SiteLatitude` /
    /// `SiteLongitude`, standard Alpaca telescope properties). rp
    /// hard-errors on mount connect when its configured `site` differs
    /// from the mount's reported site by more than 0.01° (rp.md § Site
    /// Validation Against the ASCOM Mount), so scenarios that compute a
    /// site at runtime must teach the simulated mount the same one
    /// before rp starts.
    ///
    /// **The write outlives the scenario.** OmniSim treats the site as
    /// a profile *setting*, not runtime state: the per-scenario
    /// `restart` does NOT restore the default (unlike tracking or the
    /// mount position), and on platforms without `PR_SET_PDEATHSIG`
    /// (macOS, Windows) the OmniSim process itself outlives the test
    /// binary, so a leaked site poisons the *next* suite that reuses
    /// the instance — rp's planner scenarios pin their config to
    /// OmniSim's default site and fail mount-site validation against a
    /// leftover computed one. Scenarios that call this must capture
    /// the prior site via [`Self::get_telescope_site`] and restore it
    /// when they finish.
    pub async fn set_telescope_site(
        latitude_degrees: f64,
        longitude_degrees: f64,
    ) -> Result<(), String> {
        let base_url = Self::singleton_base_url().await;
        Self::put_telescope_form_at(
            &base_url,
            0,
            "sitelatitude",
            &[("SiteLatitude", format!("{latitude_degrees}"))],
        )
        .await?;
        Self::put_telescope_form_at(
            &base_url,
            0,
            "sitelongitude",
            &[("SiteLongitude", format!("{longitude_degrees}"))],
        )
        .await
    }

    /// Read the telescope simulator's observer site — the capture half
    /// of the capture/restore contract on [`Self::set_telescope_site`].
    pub async fn get_telescope_site() -> Result<(f64, f64), String> {
        let base_url = Self::singleton_base_url().await;
        let lat = Self::get_telescope_number_at(&base_url, 0, "sitelatitude").await?;
        let lon = Self::get_telescope_number_at(&base_url, 0, "sitelongitude").await?;
        Ok((lat, lon))
    }

    /// One GET against the standard Alpaca telescope API, returning the
    /// numeric `Value` and checking both the HTTP status and the Alpaca
    /// `ErrorNumber`.
    async fn get_telescope_number_at(
        base_url: &str,
        n: u32,
        property: &str,
    ) -> Result<f64, String> {
        let url = format!("{}/api/v1/telescope/{}/{}", base_url, n, property);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| format!("reqwest client build failed: {e}"))?;
        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("GET {url} failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("GET {url} returned HTTP {}", resp.status()));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("GET {url} returned a non-JSON body: {e}"))?;
        // A response without a numeric ErrorNumber is not an Alpaca
        // response at all (wrong port, proxy error page, …) — reject
        // it rather than treating it as success.
        let error_number = body
            .get("ErrorNumber")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| {
                format!("GET {url} returned a body without a numeric ErrorNumber: {body}")
            })?;
        if error_number != 0 {
            let message = body
                .get("ErrorMessage")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            return Err(format!(
                "GET {url} returned Alpaca error {error_number}: {message}"
            ));
        }
        body.get("Value")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| format!("GET {url} returned no numeric Value: {body}"))
    }

    /// Enable or disable the telescope simulator's sidereal tracking
    /// (`Tracking`, standard Alpaca). OmniSim requires tracking to be
    /// on before `SyncToCoordinates` — call this before
    /// [`Self::sync_telescope_to`].
    pub async fn set_telescope_tracking(enabled: bool) -> Result<(), String> {
        let base_url = Self::singleton_base_url().await;
        Self::put_telescope_form_at(
            &base_url,
            0,
            "tracking",
            &[(
                "Tracking",
                if enabled { "true" } else { "false" }.to_string(),
            )],
        )
        .await
    }

    /// Sync the telescope simulator to equatorial coordinates
    /// (`SyncToCoordinates`, standard Alpaca): teleports the mount's
    /// coordinate frame without physical motion, so a scenario can
    /// start a session with the mount already "pointing" near its
    /// target and every document slew stays sub-degree (OmniSim slews
    /// at real-mount speed — a tens-of-degrees slew costs minutes).
    /// Requires tracking on (OmniSim-imposed; see
    /// [`Self::set_telescope_tracking`]).
    pub async fn sync_telescope_to(ra_hours: f64, dec_degrees: f64) -> Result<(), String> {
        let base_url = Self::singleton_base_url().await;
        Self::put_telescope_form_at(
            &base_url,
            0,
            "synctocoordinates",
            &[
                ("RightAscension", format!("{ra_hours}")),
                ("Declination", format!("{dec_degrees}")),
            ],
        )
        .await
    }

    /// The shared singleton's base URL, starting this process's OmniSim
    /// first if no scenario has done so yet. There is no fixed fallback
    /// port anymore — with per-process instances on dynamic ports, "the"
    /// OmniSim is always the one this process owns, so the state-arranging
    /// helpers (`restart_device`, the telescope-site/tracking/sync setters,
    /// the safety-monitor override) simply ensure it exists.
    async fn singleton_base_url() -> String {
        OmniSimHandle::start().await.base_url
    }

    /// One form-encoded PUT against the standard Alpaca telescope API
    /// (`/api/v1/telescope/{n}/{property}`), checking both the HTTP
    /// status and the Alpaca `ErrorNumber` in the response body — an
    /// Alpaca-level refusal (e.g. syncing with tracking off) arrives
    /// as HTTP 200 with a non-zero `ErrorNumber`.
    async fn put_telescope_form_at(
        base_url: &str,
        n: u32,
        property: &str,
        form: &[(&str, String)],
    ) -> Result<(), String> {
        let url = format!("{}/api/v1/telescope/{}/{}", base_url, n, property);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| format!("reqwest client build failed: {e}"))?;
        let resp = client
            .put(&url)
            .form(form)
            .send()
            .await
            .map_err(|e| format!("PUT {url} failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("PUT {url} returned HTTP {}", resp.status()));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("PUT {url} returned a non-JSON body: {e}"))?;
        // A response without a numeric ErrorNumber is not an Alpaca
        // response at all (wrong port, proxy error page, …) — reject
        // it rather than treating it as success.
        let error_number = body
            .get("ErrorNumber")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| {
                format!("PUT {url} returned a body without a numeric ErrorNumber: {body}")
            })?;
        if error_number != 0 {
            let message = body
                .get("ErrorMessage")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            return Err(format!(
                "PUT {url} returned Alpaca error {error_number}: {message}"
            ));
        }
        Ok(())
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
        let binary = Self::find_binary();
        let mut last_failure = String::new();
        for _ in 0..SPAWN_ATTEMPTS {
            let port = Self::pick_free_port();
            match Self::spawn_on_port(&binary, port).await {
                Ok(process) => return process,
                Err(failure) => last_failure = failure,
            }
        }
        panic!(
            "failed to start OmniSim binary '{}' after {} attempts: {}. \
             Note: bdd-infra spawns OmniSim with --multi-instance and \
             OMNISIM_SETTINGS_DIR, which need the patched fork \
             (ivonnyssen/ASCOM.Alpaca.Simulators release v0.5.0-467.2 or \
             newer) — an older binary exits at startup when any other \
             OmniSim instance is running on the host.",
            binary, SPAWN_ATTEMPTS, last_failure
        );
    }

    /// One spawn attempt: launch OmniSim on `port` and wait for it to become
    /// healthy. Returns `Err` with a diagnostic when the child exits early
    /// (lost the port-bind race, or the binary predates `--multi-instance`)
    /// or never turns healthy; the caller retries on a fresh port.
    async fn spawn_on_port(binary: &str, port: u16) -> Result<Self, String> {
        let base_url = format!("http://127.0.0.1:{}", port);

        // Capture OmniSim's stdout/stderr to per-run log files under the
        // cargo target tree. The previous `Stdio::null()` dropped every
        // line OmniSim emitted, which left CI failures with no insight
        // into what the simulator was doing — see #171 for the
        // diagnostic gap. Failures here fall back to `Stdio::null` so a
        // log-write problem can't stop the test suite from running.
        let (stdout_target, stderr_target) = Self::open_log_files(port);

        // `--multi-instance` (our fork's flag) skips OmniSim's machine-global
        // single-instance guard; `--urls` pins the Kestrel listener to the
        // port we probed as free. Clear sanitizer-related env vars so the
        // .NET runtime isn't broken by LD_PRELOAD injection from ASAN/LSAN.
        let mut cmd = std::process::Command::new(binary);
        cmd.arg("--multi-instance")
            .arg(format!("--urls={}", base_url))
            .stdout(stdout_target)
            .stderr(stderr_target)
            .env_remove("LD_PRELOAD")
            .env_remove("ASAN_OPTIONS")
            .env_remove("LSAN_OPTIONS");

        // Per-instance settings dir: concurrent OmniSims must not share a
        // writable profile store (see `prepare_settings_dir`). The fork's
        // OMNISIM_SETTINGS_DIR (467.2) re-roots the profile store on every
        // platform — XDG_CONFIG_HOME would cover Linux only (.NET ignores
        // it on macOS, and Windows never honored it).
        if let Some(dir) = Self::prepare_settings_dir() {
            cmd.env("OMNISIM_SETTINGS_DIR", dir);
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

        let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {}", e))?;

        match Self::wait_healthy(&mut child, &base_url).await {
            Ok(()) => Ok(Self {
                _child: child,
                base_url,
                port,
            }),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(e)
            }
        }
    }

    /// Probe the OS for a free `127.0.0.1` port by binding an ephemeral
    /// listener and immediately dropping it. Another process can grab the
    /// port in the window before OmniSim binds it — that lost race surfaces
    /// as an early child exit in [`Self::wait_healthy`] and costs one retry
    /// in [`Self::get_or_spawn`].
    fn pick_free_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .and_then(|listener| listener.local_addr())
            .map(|addr| addr.port())
            .unwrap_or_else(|e| panic!("failed to probe a free port for OmniSim: {}", e))
    }

    /// Seed a per-instance `OMNISIM_SETTINGS_DIR` for OmniSim and return
    /// its path. The seed is a recursive copy of the checked-in
    /// `crates/bdd-infra/omnisim-config/` tree, whose layout mirrors what
    /// the fork puts under the settings root
    /// (`ascom-alpaca-simulator/<device>/v1/instance-0.xml`; the lowercase
    /// names also satisfy Windows' case-insensitive lookups of its
    /// platform-cased paths). OmniSim writes back to this directory on
    /// startup (e.g. emitting missing UniqueIDs, persisting full default
    /// profiles), so we MUST copy the source into a scratch location and
    /// never let OmniSim see the repository copy directly.
    ///
    /// The destination is suffixed with the test process's PID
    /// (`bdd-infra-omnisim-<pid>/`) under [`Self::state_root`]: with
    /// parallel suites and shards each spawning a private OmniSim,
    /// instances must not share a writable profile dir either — a shared
    /// dir would race the startup write-backs and leak profile *settings*
    /// (e.g. the telescope site, which `restart` does not reset) between
    /// concurrently running suites. That leak is not hypothetical: it
    /// failed 4 of 8 rp:bdd shards on macOS CI when isolation still rode
    /// on XDG_CONFIG_HOME, which .NET ignores there. We fully reseed on
    /// every spawn so a write-back from a prior run can't leak into this
    /// one.
    ///
    /// Returns `None` (and the caller proceeds without the override) only
    /// when the destination dir can't be created at all. A missing seed
    /// source is non-fatal: the instance still gets a private, initially
    /// empty config dir and runs on upstream defaults.
    fn prepare_settings_dir() -> Option<PathBuf> {
        let dest = Self::state_root().join(format!("bdd-infra-omnisim-{}", std::process::id()));
        // Wipe whatever a prior spawn attempt (or a previous run that
        // recycled this PID) left behind so an OmniSim write-back from
        // then can't survive into this run's profile.
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::create_dir_all(&dest).ok()?;
        if let Some(src) = Self::seed_config_source() {
            // Best-effort: a partial copy still leaves a private dir.
            let _ = Self::copy_dir_recursive(&src, &dest);
        }
        Some(dest)
    }

    /// Locate the checked-in `omnisim-config` seed tree.
    ///
    /// 1. `env!("CARGO_MANIFEST_DIR")/omnisim-config` — resolves under
    ///    cargo. Under Bazel `CARGO_MANIFEST_DIR` is a compile-time
    ///    sandbox path that doesn't exist at test runtime.
    /// 2. Walking up from the cwd looking for
    ///    `crates/bdd-infra/omnisim-config` — resolves in the Bazel
    ///    runfiles tree (after the `bdd_main!` chdir the cwd is
    ///    `<runfiles>/_main/<package>`; the seed tree rides along as
    ///    `data` on the `bdd-infra_rp_harness` target).
    ///
    /// Returns `None` when neither resolves. Note that before #467 the
    /// Bazel path never resolved (branch 1 was dead and the tree wasn't in
    /// the runfiles), so Bazel-run suites always used upstream defaults;
    /// branch 2 closes that gap and brings the tuned timings (shorter
    /// cover-calibrator open/close) to Bazel runs too.
    fn seed_config_source() -> Option<PathBuf> {
        let compile_time = Path::new(env!("CARGO_MANIFEST_DIR")).join("omnisim-config");
        if compile_time.is_dir() {
            return Some(compile_time);
        }
        let cwd = std::env::current_dir().ok()?;
        for ancestor in cwd.ancestors() {
            let candidate = ancestor
                .join("crates")
                .join("bdd-infra")
                .join("omnisim-config");
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
        None
    }

    /// Root for per-instance scratch state: Bazel's per-action
    /// `TEST_TMPDIR` when present (cleaned up by Bazel), else the cargo
    /// target tree (reached by `cargo clean`).
    fn state_root() -> PathBuf {
        if let Some(tmp) = std::env::var_os("TEST_TMPDIR") {
            return PathBuf::from(tmp);
        }
        std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .and_then(|p| p.parent())
                    .map(|workspace| workspace.join("target"))
                    .unwrap_or_else(|| PathBuf::from("target"))
            })
    }

    /// Resolve the log directory for OmniSim's captured stdout/stderr.
    /// Lives at `<CARGO_TARGET_DIR>/bdd-infra-omnisim-logs/` (or
    /// `<workspace>/target/bdd-infra-omnisim-logs/` if unset). Kept
    /// outside the seeded settings dir so `prepare_settings_dir`'s
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
    /// suffix avoids that. The port distinguishes retried spawn attempts
    /// within one process, so a failed attempt's log (the bind-race /
    /// old-binary evidence) survives the retry.
    fn open_log_files(port: u16) -> (Stdio, Stdio) {
        let dir = Self::log_dir();
        let pid = std::process::id();
        let stdout = dir
            .as_ref()
            .and_then(|d| {
                std::fs::File::create(d.join(format!("omnisim.{pid}.{port}.stdout.log"))).ok()
            })
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
                    std::fs::File::create(d.join(format!("omnisim.{pid}.{port}.stderr.log"))).ok()
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

    /// Poll `base_url` until OmniSim answers, watching the child so an
    /// early exit (lost port-bind race; a pre-`--multi-instance` binary
    /// deferring to another running instance) fails fast with its exit
    /// status instead of burning the full 30-second health window.
    async fn wait_healthy(child: &mut std::process::Child, base_url: &str) -> Result<(), String> {
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!(
                    "OmniSim exited during startup ({}) — lost the port-bind \
                     race, or the binary does not support --multi-instance",
                    status
                ));
            }
            if Self::is_healthy(base_url).await {
                return Ok(());
            }
        }
        Err(format!(
            "OmniSim did not become healthy at {} within 30 seconds",
            base_url
        ))
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
    async fn set_safety_monitor_is_safe_puts_form_value() {
        use axum::Form;
        use std::collections::HashMap;

        let (tx_seen, rx_seen) = tokio::sync::oneshot::channel::<String>();
        let tx_seen = std::sync::Arc::new(std::sync::Mutex::new(Some(tx_seen)));
        let app = Router::new().route(
            "/simulator/v1/safetymonitor/0/issafesetting",
            put(move |Form(form): Form<HashMap<String, String>>| {
                let tx_seen = tx_seen.clone();
                async move {
                    if let Some(tx) = tx_seen.lock().unwrap().take() {
                        let _ = tx.send(form.get("IsSafeSetting").cloned().unwrap_or_default());
                    }
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
        let base_url = format!("http://127.0.0.1:{}", port);

        OmniSimHandle::set_safety_monitor_is_safe_at(&base_url, 0, false)
            .await
            .unwrap();
        assert_eq!(rx_seen.await.unwrap(), "false");
        let _ = tx.send(());
    }

    #[tokio::test]
    async fn set_safety_monitor_is_safe_returns_err_on_server_error() {
        let route = "/simulator/v1/safetymonitor/0/issafesetting";
        let app = Router::new().route(
            route,
            put(move || async move { StatusCode::INTERNAL_SERVER_ERROR }),
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
        let base_url = format!("http://127.0.0.1:{}", port);

        let err = OmniSimHandle::set_safety_monitor_is_safe_at(&base_url, 0, true)
            .await
            .expect_err("expected an error for 500 response");
        assert!(err.contains("500"), "expected 500 in error: {err}");
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

    /// Stub answering one Alpaca telescope property PUT with the given
    /// JSON body, capturing the submitted form for assertion.
    async fn spawn_telescope_put_stub(
        property: &str,
        body: serde_json::Value,
    ) -> (
        String,
        tokio::sync::oneshot::Receiver<std::collections::HashMap<String, String>>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        use axum::Form;
        use std::collections::HashMap;

        let (tx_seen, rx_seen) = tokio::sync::oneshot::channel::<HashMap<String, String>>();
        let tx_seen = std::sync::Arc::new(std::sync::Mutex::new(Some(tx_seen)));
        let route = format!("/api/v1/telescope/0/{property}");
        let app = Router::new().route(
            &route,
            put(move |Form(form): Form<HashMap<String, String>>| {
                let tx_seen = tx_seen.clone();
                let body = body.clone();
                async move {
                    if let Some(tx) = tx_seen.lock().unwrap().take() {
                        let _ = tx.send(form);
                    }
                    axum::Json(body)
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
        (format!("http://127.0.0.1:{}", port), rx_seen, tx)
    }

    #[tokio::test]
    async fn telescope_form_put_sends_values_and_accepts_error_number_zero() {
        let (base_url, rx_seen, shutdown) = spawn_telescope_put_stub(
            "synctocoordinates",
            serde_json::json!({ "ErrorNumber": 0, "ErrorMessage": "" }),
        )
        .await;
        OmniSimHandle::put_telescope_form_at(
            &base_url,
            0,
            "synctocoordinates",
            &[
                ("RightAscension", "2.5".to_string()),
                ("Declination", "0".to_string()),
            ],
        )
        .await
        .unwrap();
        let form = rx_seen.await.unwrap();
        assert_eq!(form.get("RightAscension").map(String::as_str), Some("2.5"));
        assert_eq!(form.get("Declination").map(String::as_str), Some("0"));
        let _ = shutdown.send(());
    }

    #[tokio::test]
    async fn telescope_number_get_parses_value_and_surfaces_alpaca_error() {
        use axum::routing::get;

        let app = Router::new()
            .route(
                "/api/v1/telescope/0/sitelatitude",
                get(|| async {
                    axum::Json(serde_json::json!({ "Value": 51.07861, "ErrorNumber": 0 }))
                }),
            )
            .route(
                "/api/v1/telescope/0/sitelongitude",
                get(|| async {
                    axum::Json(serde_json::json!({
                        "ErrorNumber": 1024,
                        "ErrorMessage": "property not implemented"
                    }))
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
        let base_url = format!("http://127.0.0.1:{}", port);

        let lat = OmniSimHandle::get_telescope_number_at(&base_url, 0, "sitelatitude")
            .await
            .unwrap();
        assert!((lat - 51.07861).abs() < 1e-9, "unexpected latitude {lat}");

        let err = OmniSimHandle::get_telescope_number_at(&base_url, 0, "sitelongitude")
            .await
            .expect_err("expected the Alpaca error to surface");
        assert!(
            err.contains("1024") && err.contains("not implemented"),
            "unexpected error format: {err}"
        );
        let _ = tx.send(());
    }

    #[tokio::test]
    async fn telescope_helpers_reject_a_body_without_an_error_number() {
        use axum::routing::get;

        // An empty JSON object is what a non-Alpaca endpoint (wrong
        // port, proxy) might answer — both helpers must reject it
        // rather than read the missing ErrorNumber as success.
        let app = Router::new()
            .route(
                "/api/v1/telescope/0/sitelatitude",
                get(|| async { axum::Json(serde_json::json!({})) }),
            )
            .route(
                "/api/v1/telescope/0/tracking",
                put(|| async { axum::Json(serde_json::json!({})) }),
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
        let base_url = format!("http://127.0.0.1:{}", port);

        let err = OmniSimHandle::get_telescope_number_at(&base_url, 0, "sitelatitude")
            .await
            .expect_err("a body without ErrorNumber must not read as success");
        assert!(err.contains("without a numeric ErrorNumber"), "{err}");

        let err = OmniSimHandle::put_telescope_form_at(
            &base_url,
            0,
            "tracking",
            &[("Tracking", "true".to_string())],
        )
        .await
        .expect_err("a body without ErrorNumber must not read as success");
        assert!(err.contains("without a numeric ErrorNumber"), "{err}");
        let _ = tx.send(());
    }

    #[tokio::test]
    async fn telescope_form_put_surfaces_alpaca_error_number() {
        // OmniSim refuses e.g. a sync with tracking off as HTTP 200 +
        // a non-zero ErrorNumber — the helper must fail loud on it.
        let (base_url, _rx_seen, shutdown) = spawn_telescope_put_stub(
            "synctocoordinates",
            serde_json::json!({
                "ErrorNumber": 1036,
                "ErrorMessage": "SyncToCoordinates is not allowed when tracking is False"
            }),
        )
        .await;
        let err = OmniSimHandle::put_telescope_form_at(
            &base_url,
            0,
            "synctocoordinates",
            &[("RightAscension", "2.5".to_string())],
        )
        .await
        .expect_err("expected the Alpaca error to surface");
        assert!(
            err.contains("1036") && err.contains("tracking is False"),
            "unexpected error format: {err}"
        );
        let _ = shutdown.send(());
    }
}
