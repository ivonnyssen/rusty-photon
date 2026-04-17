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
