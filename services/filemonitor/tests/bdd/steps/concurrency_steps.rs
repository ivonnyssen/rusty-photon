use crate::world::FilemonitorWorld;
use cucumber::{then, when};
use serde_json::Value;

#[when(expr = "{int} tasks toggle the connection while {int} tasks read it")]
async fn concurrent_toggle_and_read(
    world: &mut FilemonitorWorld,
    toggle_count: usize,
    read_count: usize,
) {
    let client = world.client.clone().expect("client not created");
    let base_url = world
        .filemonitor
        .as_ref()
        .expect("filemonitor not started")
        .base_url
        .clone();
    let connected_url = format!("{}/api/v1/safetymonitor/0/connected", base_url);

    let mut handles = Vec::new();

    for i in 0..toggle_count {
        let client = client.clone();
        let url = connected_url.clone();
        handles.push(tokio::spawn(async move {
            let connected = if i % 2 == 0 { "true" } else { "false" };
            let _ = client
                .put(&url)
                .form(&[
                    ("Connected", connected),
                    ("ClientID", "1"),
                    ("ClientTransactionID", "1"),
                ])
                .send()
                .await;
        }));
    }

    for _ in 0..read_count {
        let client = client.clone();
        let url = connected_url.clone();
        handles.push(tokio::spawn(async move {
            let _ = client.get(&url).send().await;
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }
}

#[when(expr = "{int} tasks check is_safe concurrently")]
async fn concurrent_issafe(world: &mut FilemonitorWorld, task_count: usize) {
    let client = world.client.clone().expect("client not created");
    let url = world.alpaca_url("issafe");

    let handles: Vec<_> = (0..task_count)
        .map(|_| {
            let client = client.clone();
            let url = url.clone();
            tokio::spawn(async move {
                let resp = client.get(&url).send().await.unwrap();
                let json: Value = resp.json().await.unwrap();
                json["Value"].as_bool().unwrap_or(false)
            })
        })
        .collect();

    let mut all_true = true;
    for handle in handles {
        let result = handle.await.unwrap();
        if !result {
            all_true = false;
        }
    }

    world.safety_result = Some(all_true);
}

#[when(expr = "{int} tasks perform mixed operations concurrently")]
async fn concurrent_mixed_operations(world: &mut FilemonitorWorld, task_count: usize) {
    let client = world.client.clone().expect("client not created");
    let base_url = world
        .filemonitor
        .as_ref()
        .expect("filemonitor not started")
        .base_url
        .clone();

    let handles: Vec<_> = (0..task_count)
        .map(|i| {
            let client = client.clone();
            let base_url = base_url.clone();
            tokio::spawn(async move {
                match i % 4 {
                    0 => {
                        let _ = client
                            .put(format!("{}/api/v1/safetymonitor/0/connected", base_url))
                            .form(&[
                                ("Connected", "true"),
                                ("ClientID", "1"),
                                ("ClientTransactionID", "1"),
                            ])
                            .send()
                            .await;
                    }
                    1 => {
                        let _ = client
                            .get(format!("{}/api/v1/safetymonitor/0/issafe", base_url))
                            .send()
                            .await;
                    }
                    2 => {
                        let _ = client
                            .get(format!("{}/api/v1/safetymonitor/0/name", base_url))
                            .send()
                            .await;
                    }
                    _ => {
                        let _ = client
                            .get(format!("{}/api/v1/safetymonitor/0/driverversion", base_url))
                            .send()
                            .await;
                    }
                }
            })
        })
        .collect();

    for handle in handles {
        handle.await.unwrap();
    }
}

#[then("no panics should occur")]
fn no_panics(_world: &mut FilemonitorWorld) {
    // If we got here, no panics occurred during concurrent operations
}

#[then("all concurrent is_safe results should be true")]
fn all_concurrent_true(world: &mut FilemonitorWorld) {
    let result = world.safety_result.expect("no concurrent results");
    assert!(result, "not all concurrent is_safe results were true");
}
