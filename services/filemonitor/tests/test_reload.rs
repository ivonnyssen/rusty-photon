#[cfg(not(miri))]
use filemonitor::run_server_loop;
#[cfg(not(miri))]
use std::io::Write;
#[cfg(not(miri))]
use std::sync::atomic::{AtomicU32, Ordering};
#[cfg(not(miri))]
use std::sync::Arc;

// Both tests bind the ASCOM Alpaca discovery port, so they must run sequentially.
// We combine them into a single test to avoid parallel port conflicts.
#[tokio::test(flavor = "multi_thread")]
#[cfg(not(miri))]
async fn test_server_loop_stop_and_reload() {
    // --- Part 1: stop signal ---
    {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let monitor_file = dir.path().join("monitor.txt");

        std::fs::write(&monitor_file, "SAFE").unwrap();

        let config = serde_json::json!({
            "device": {
                "name": "Test",
                "unique_id": "test-stop-001",
                "description": "Test device"
            },
            "file": {
                "path": monitor_file.to_str().unwrap(),
                "polling_interval_seconds": 60
            },
            "parsing": {
                "rules": [],
                "case_sensitive": false
            },
            "server": {
                "port": 0,
                "device_number": 0
            }
        });

        let mut f = std::fs::File::create(&config_path).unwrap();
        f.write_all(config.to_string().as_bytes()).unwrap();

        let result = run_server_loop(
            &config_path,
            || {
                Box::pin(async {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                })
            },
            || Box::pin(std::future::pending()),
        )
        .await;

        assert!(result.is_ok(), "stop test failed: {:?}", result.err());
    }

    // --- Part 2: reload signal ---
    {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let monitor_file = dir.path().join("monitor.txt");

        std::fs::write(&monitor_file, "SAFE").unwrap();

        let config = serde_json::json!({
            "device": {
                "name": "Test",
                "unique_id": "test-reload-001",
                "description": "Test device"
            },
            "file": {
                "path": monitor_file.to_str().unwrap(),
                "polling_interval_seconds": 60
            },
            "parsing": {
                "rules": [],
                "case_sensitive": false
            },
            "server": {
                "port": 0,
                "device_number": 0
            }
        });

        let mut f = std::fs::File::create(&config_path).unwrap();
        f.write_all(config.to_string().as_bytes()).unwrap();

        let loop_count = Arc::new(AtomicU32::new(0));
        let loop_count_reload = Arc::clone(&loop_count);
        let loop_count_stop = Arc::clone(&loop_count);

        let result = run_server_loop(
            &config_path,
            move || {
                let count = loop_count_stop.clone();
                Box::pin(async move {
                    // Stop after the reload has happened (loop_count >= 2)
                    loop {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        if count.load(Ordering::Relaxed) >= 2 {
                            break;
                        }
                    }
                })
            },
            move || {
                let count = loop_count_reload.clone();
                Box::pin(async move {
                    let current = count.fetch_add(1, Ordering::Relaxed);
                    if current == 0 {
                        // First iteration: trigger a reload after a brief delay
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    } else {
                        // Subsequent iterations: don't reload again
                        std::future::pending::<()>().await;
                    }
                })
            },
        )
        .await;

        assert!(result.is_ok(), "reload test failed: {:?}", result.err());
        assert!(
            loop_count.load(Ordering::Relaxed) >= 2,
            "Server loop should have run at least twice (once initial + once after reload)"
        );
    }
}
