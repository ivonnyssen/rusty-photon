//! BDD entry point for `ui-htmx`. Purely in-process — the steps drive the axum
//! router via `tower::ServiceExt::oneshot` against a stubbed `ConfigClient`, so
//! no child process is spawned and the `bdd_main!` macro is not needed
//! (docs/skills/testing.md §5.2).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]

#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

#[tokio::main]
async fn main() {
    use cucumber::World as _;
    world::UiWorld::cucumber()
        .run_and_exit("tests/features")
        .await;
}
