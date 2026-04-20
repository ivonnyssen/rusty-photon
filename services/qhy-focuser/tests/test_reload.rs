//! Integration test for `qhy_focuser::run_server_loop` stop and reload paths.
//!
//! Drives the loop with injected `stop` and `reload` closures so both the
//! graceful-shutdown branch and the reload-and-continue branch are exercised
//! without relying on OS signals.

#![cfg(feature = "mock")]

use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use qhy_focuser::{run_server_loop, MockSerialPortFactory, SerialPortFactory};

fn write_config(path: &std::path::Path, device_suffix: &str) {
    let config = serde_json::json!({
        "serial": {
            "port": "/dev/null",
            "baud_rate": 9600,
            "polling_interval_ms": 1000,
            "timeout_seconds": 2
        },
        "server": {
            "port": 0,
            "discovery_port": null
        },
        "focuser": {
            "name": "Test Focuser",
            "unique_id": format!("test-focuser-{}", device_suffix),
            "description": "Test focuser",
            "device_number": 0,
            "enabled": true,
            "max_step": 64000,
            "speed": 0,
            "reverse": false
        }
    });
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(config.to_string().as_bytes()).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_server_loop_stop_and_reload() {
    // --- Part 1: stop signal ---
    {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        write_config(&config_path, "stop");

        let factory: Arc<dyn SerialPortFactory> = Arc::new(MockSerialPortFactory::default());

        let result = run_server_loop(
            &config_path,
            factory,
            |_| {},
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
        write_config(&config_path, "reload");

        let factory: Arc<dyn SerialPortFactory> = Arc::new(MockSerialPortFactory::default());

        let loop_count = Arc::new(AtomicU32::new(0));
        let loop_count_reload = Arc::clone(&loop_count);
        let loop_count_stop = Arc::clone(&loop_count);

        let result = run_server_loop(
            &config_path,
            factory,
            |_| {},
            move || {
                let count = loop_count_stop.clone();
                Box::pin(async move {
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
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                })
            },
        )
        .await;

        assert!(result.is_ok(), "reload test failed: {:?}", result.err());
        assert!(
            loop_count.load(Ordering::Relaxed) >= 2,
            "Server loop should iterate at least twice (initial + post-reload)"
        );
    }
}
