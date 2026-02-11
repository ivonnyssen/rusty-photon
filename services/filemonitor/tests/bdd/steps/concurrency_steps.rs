use crate::world::FilemonitorWorld;
use ascom_alpaca::api::Device;
use cucumber::{then, when};
use std::sync::Arc;
use tokio::time::{sleep, Duration};

#[when(expr = "{int} tasks toggle the connection state while {int} tasks read it")]
async fn concurrent_toggle_and_read(
    world: &mut FilemonitorWorld,
    toggle_count: usize,
    read_count: usize,
) {
    let device = world.device.clone().expect("device not created");

    let mut handles = Vec::new();

    for i in 0..toggle_count {
        let d = Arc::clone(&device);
        handles.push(tokio::spawn(async move {
            let connected = i % 2 == 0;
            let _ = d.set_connected(connected).await;
            sleep(Duration::from_millis(1)).await;
        }));
    }

    for _ in 0..read_count {
        let d = Arc::clone(&device);
        handles.push(tokio::spawn(async move {
            let _ = d.connected().await;
            sleep(Duration::from_millis(1)).await;
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }
}

#[when(expr = "{int} tasks evaluate {string} and {string} {int} times each")]
async fn concurrent_evaluate(
    world: &mut FilemonitorWorld,
    task_count: usize,
    safe_content: String,
    unsafe_content: String,
    iterations: usize,
) {
    let device = world.device.clone().expect("device not created");

    let mut safe_results = Vec::new();
    let mut unsafe_results = Vec::new();

    let mut handles = Vec::new();

    for i in 0..task_count {
        let d = Arc::clone(&device);
        let sc = safe_content.clone();
        let uc = unsafe_content.clone();
        handles.push(tokio::spawn(async move {
            let mut safe_res = Vec::new();
            let mut unsafe_res = Vec::new();
            for _ in 0..iterations {
                if i % 2 == 0 {
                    safe_res.push(d.evaluate_safety(&sc));
                } else {
                    unsafe_res.push(d.evaluate_safety(&uc));
                }
                sleep(Duration::from_millis(1)).await;
            }
            (safe_res, unsafe_res)
        }));
    }

    for handle in handles {
        let (sr, ur) = handle.await.unwrap();
        safe_results.extend(sr);
        unsafe_results.extend(ur);
    }

    // Store results for later assertions
    world.last_error = Some(format!(
        "safe:{} unsafe:{}",
        safe_results.iter().all(|r| *r),
        unsafe_results.iter().all(|r| !*r)
    ));
}

#[when(expr = "{int} tasks perform mixed operations concurrently")]
async fn concurrent_mixed_operations(world: &mut FilemonitorWorld, task_count: usize) {
    let device = world.device.clone().expect("device not created");

    let handles: Vec<_> = (0..task_count)
        .map(|i| {
            let d = Arc::clone(&device);
            tokio::spawn(async move {
                match i % 3 {
                    0 => {
                        let _ = d.set_connected(true).await;
                        let _ = d.connected().await;
                    }
                    1 => {
                        let _ = d.evaluate_safety("test content");
                    }
                    _ => {
                        let _ = d.description().await;
                        let _ = d.driver_version().await;
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

#[then(expr = "all {string} results should be safe")]
fn all_safe_results(world: &mut FilemonitorWorld, _content: String) {
    let info = world.last_error.as_ref().expect("no concurrency results");
    assert!(
        info.contains("safe:true"),
        "some safe evaluations returned unsafe: {info}"
    );
}

#[then(expr = "all {string} results should be unsafe")]
fn all_unsafe_results(world: &mut FilemonitorWorld, _content: String) {
    let info = world.last_error.as_ref().expect("no concurrency results");
    assert!(
        info.contains("unsafe:true"),
        "some unsafe evaluations returned safe: {info}"
    );
}
