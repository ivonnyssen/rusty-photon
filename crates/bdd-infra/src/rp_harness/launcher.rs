//! Helpers for launching rp (or any plugin) from a JSON config Value.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;

use crate::ServiceHandle;

/// Per-process counter so each call to [`write_temp_config_file`] produces a
/// distinct path. Combined with the PID, this guarantees uniqueness across
/// parallel scenarios and across test binaries spawned by the same
/// `cargo test` invocation, matching the pattern used by
/// [`RpConfigBuilder::build`](super::config::RpConfigBuilder::build) for
/// `data_directory` / `session_state_file`.
static CONFIG_SEQ: AtomicU64 = AtomicU64::new(0);

/// Write a `serde_json::Value` to a uniquely-named file in the system temp
/// directory and return its path as a `String`.
///
/// The `prefix` disambiguates configs across services (e.g. `"rp-test-config"`
/// vs `"calibrator-flats-config"`); PID + monotonic sequence guarantee
/// collision-free paths even under coarse system clocks or concurrent calls.
pub async fn write_temp_config_file(prefix: &str, config: &Value) -> String {
    let pid = std::process::id();
    let seq = CONFIG_SEQ.fetch_add(1, Ordering::Relaxed);
    let config_path = std::env::temp_dir()
        .join(format!("{}-{}-{}.json", prefix, pid, seq))
        .to_string_lossy()
        .to_string();
    tokio::fs::write(&config_path, serde_json::to_string_pretty(config).unwrap())
        .await
        .unwrap_or_else(|e| panic!("failed to write temp config '{}': {}", config_path, e));
    config_path
}

/// Start rp with the given config. Returns the [`ServiceHandle`].
///
/// `rp_manifest_dir` must be the absolute path to `services/rp/`. Callers in
/// rp's own tests pass `env!("CARGO_MANIFEST_DIR")` directly; callers in
/// sibling crates (e.g. calibrator-flats tests) compute the path via
/// [`sibling_service_dir`].
///
/// The caller is responsible for calling [`wait_for_rp_healthy`] afterwards
/// if they need to block until rp is serving requests.
pub async fn start_rp(rp_manifest_dir: &str, config: &Value) -> ServiceHandle {
    let config_path = write_temp_config_file("rp-test-config", config).await;
    ServiceHandle::start(rp_manifest_dir, "rp", &config_path).await
}

/// Poll `GET <rp_base_url>/health` until it returns 200, up to 30 seconds.
/// Returns `true` if rp became healthy, `false` on timeout.
pub async fn wait_for_rp_healthy(rp_base_url: &str) -> bool {
    let client = reqwest::Client::new();
    let url = format!("{}/health", rp_base_url);
    for _ in 0..120 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().as_u16() == 200 {
                return true;
            }
        }
    }
    false
}

/// Resolve the manifest directory of a sibling service in the `services/`
/// layout from the caller's `env!("CARGO_MANIFEST_DIR")`.
///
/// The workspace convention is `workspace/services/<name>/`. This helper
/// navigates one level up from `caller_manifest_dir` and then into `name`.
/// For a caller at `services/calibrator-flats`, `sibling_service_dir(..., "rp")`
/// returns `services/rp`.
pub fn sibling_service_dir(caller_manifest_dir: &str, sibling_name: &str) -> PathBuf {
    Path::new(caller_manifest_dir)
        .parent()
        .unwrap_or_else(|| {
            panic!(
                "caller_manifest_dir '{}' has no parent",
                caller_manifest_dir
            )
        })
        .join(sibling_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sibling_service_dir_navigates_up_one_level() {
        let result = sibling_service_dir("/home/me/repo/services/calibrator-flats", "rp");
        assert_eq!(result, PathBuf::from("/home/me/repo/services/rp"));
    }

    #[tokio::test]
    async fn write_temp_config_file_produces_readable_json() {
        let config = serde_json::json!({ "foo": "bar", "n": 42 });
        let path = write_temp_config_file("bdd-infra-test", &config).await;

        let bytes = tokio::fs::read(&path).await.unwrap();
        let parsed: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed, config);
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn write_temp_config_file_paths_are_unique_across_calls() {
        let config = serde_json::json!({ "k": 1 });
        let a = write_temp_config_file("bdd-infra-unique", &config).await;
        let b = write_temp_config_file("bdd-infra-unique", &config).await;
        assert_ne!(a, b);
        let _ = tokio::fs::remove_file(&a).await;
        let _ = tokio::fs::remove_file(&b).await;
    }
}
