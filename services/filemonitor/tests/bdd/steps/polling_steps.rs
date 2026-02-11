use crate::world::FilemonitorWorld;
use cucumber::when;
use tokio::time::{sleep, Duration};

#[when(expr = "the file content changes to {string}")]
fn file_content_changes(world: &mut FilemonitorWorld, content: String) {
    let path = world
        .temp_file_path
        .as_ref()
        .expect("temp file not created");
    std::fs::write(path, content).expect("failed to write to temp file");
}

#[when("I wait for the polling interval to elapse")]
async fn wait_for_polling(_world: &mut FilemonitorWorld) {
    sleep(Duration::from_millis(1500)).await;
}
