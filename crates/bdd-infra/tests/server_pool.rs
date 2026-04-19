//! Integration tests for `ServerPool`, including `try_acquire` semantics.

use bdd_infra::ServerPool;

fn leak_str(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Build a temp manifest dir with [package.metadata.bdd] pointing at a unique env var,
/// and set that env var to the test_service binary path. Returns leaked `&'static str`
/// handles for the manifest dir and package name (as required by `ServerPool::new`).
fn setup_pool_manifest(env_var_name: &str) -> (tempfile::TempDir, &'static str, &'static str) {
    std::env::set_var(env_var_name, env!("CARGO_BIN_EXE_test_service"));
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        format!(
            r#"
[package]
name = "test"
version = "0.1.0"
edition = "2021"

[package.metadata.bdd]
env_var = "{env_var_name}"
"#
        ),
    )
    .unwrap();
    let manifest_dir = leak_str(dir.path().to_string_lossy().into_owned());
    let package_name = leak_str("test-service".to_string());
    (dir, manifest_dir, package_name)
}

#[tokio::test]
async fn test_pool_try_acquire_failure_does_not_poison_hash_slot() {
    let (_dir, manifest_dir, package_name) = setup_pool_manifest("BDD_POOL_TRY_ACQUIRE_FAIL");
    // Exclude "marker" from the hash so the failing and healthy configs
    // hash identically and share a pool slot.
    let pool = ServerPool::new(manifest_dir, package_name, vec![vec!["marker".to_string()]]);

    // First attempt with "fail" content causes test_service to exit without
    // binding — pool must return Err and leave the slot body as None.
    let bad = serde_json::json!({ "marker": "fail" });
    let err = pool.try_acquire(&bad).await.unwrap_err();
    assert!(
        err.contains("exited without binding"),
        "expected startup failure, got: {}",
        err
    );

    // Second attempt on the SAME hash slot with valid content must succeed.
    // If the prior failure poisoned the slot, this would observe a stale
    // EntryBody or hang.
    let good = serde_json::json!({ "marker": "ok" });
    let guard = pool.try_acquire(&good).await.unwrap();
    assert!(guard.port > 0);
    drop(guard);

    pool.stop_all().await;
}

#[tokio::test]
async fn test_pool_try_acquire_succeeds_with_valid_config() {
    let (_dir, manifest_dir, package_name) = setup_pool_manifest("BDD_POOL_TRY_ACQUIRE_OK");
    let pool = ServerPool::new(manifest_dir, package_name, vec![]);

    let config = serde_json::json!({ "marker": "healthy" });
    let guard = pool.try_acquire(&config).await.unwrap();
    assert!(guard.port > 0);
    assert!(guard.base_url.starts_with("http://127.0.0.1:"));
    drop(guard);

    pool.stop_all().await;
}
