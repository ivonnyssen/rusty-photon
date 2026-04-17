//! Test infrastructure: filemonitor process management.

pub use bdd_infra::ServerPool;
pub use bdd_infra::ServiceHandle;

use std::sync::LazyLock;
use tokio::sync::Mutex;

/// Excluded config fields for hashing (per-run artifacts, not functional config).
const EXCLUDE_PATHS: &[&[&str]] = &[
    &["server", "port"],
    &["server", "discovery_port"],
    &["file", "path"],
];

pub static POOL: LazyLock<Mutex<ServerPool>> = LazyLock::new(|| {
    Mutex::new(ServerPool::new(
        env!("CARGO_MANIFEST_DIR"),
        env!("CARGO_PKG_NAME"),
        EXCLUDE_PATHS
            .iter()
            .map(|p| p.iter().map(|s| s.to_string()).collect())
            .collect(),
    ))
});

pub async fn stop_all_servers() {
    POOL.lock().await.stop_all().await;
}
