use crate::world::FilemonitorWorld;
use cucumber::{then, when};

#[when(expr = "{int} tasks toggle the connection while {int} tasks read it")]
async fn concurrent_toggle_and_read(
    world: &mut FilemonitorWorld,
    toggle_count: usize,
    read_count: usize,
) {
    let monitor = world.monitor().clone();
    let mut handles = Vec::new();

    for i in 0..toggle_count {
        let m = monitor.clone();
        handles.push(tokio::spawn(async move {
            let _ = m.set_connected(i % 2 == 0).await;
        }));
    }

    for _ in 0..read_count {
        let m = monitor.clone();
        handles.push(tokio::spawn(async move {
            let _ = m.connected().await;
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }
}

#[when(expr = "{int} tasks check is_safe concurrently")]
async fn concurrent_issafe(world: &mut FilemonitorWorld, task_count: usize) {
    let monitor = world.monitor().clone();

    let handles: Vec<_> = (0..task_count)
        .map(|_| {
            let m = monitor.clone();
            tokio::spawn(async move { m.is_safe().await.unwrap_or(false) })
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
    let monitor = world.monitor().clone();

    let handles: Vec<_> = (0..task_count)
        .map(|i| {
            let m = monitor.clone();
            tokio::spawn(async move {
                match i % 4 {
                    0 => {
                        let _ = m.set_connected(true).await;
                    }
                    1 => {
                        let _ = m.is_safe().await;
                    }
                    2 => {
                        let _ = m.name().await;
                    }
                    _ => {
                        let _ = m.driver_version().await;
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
