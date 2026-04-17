//! BDD test entry point for filemonitor service

#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

bdd_infra::bdd_main! {
    use cucumber::World as _;
    use cucumber::writer::Stats as _;
    use world::FilemonitorWorld;

    // Use `.run(...)` instead of `.run_and_exit(...)` so that pooled servers can
    // be stopped after the suite. We replicate `run_and_exit`'s failure-panic
    // below so the binary still exits non-zero when scenarios fail.
    let writer = FilemonitorWorld::cucumber()
        .max_concurrent_scenarios(Some(1))
        .after(|_feature, _rule, _scenario, _finished, maybe_world| {
            Box::pin(async move {
                if let Some(world) = maybe_world {
                    // Stop only directly-started servers (TLS/auth scenarios).
                    // Pool-managed servers are kept alive across scenarios.
                    if let Some(fm) = world.filemonitor.as_mut() {
                        fm.stop().await;
                    }
                }
            })
        })
        .run("tests/features")
        .await;

    steps::infrastructure::stop_all_servers().await;

    if writer.execution_has_failed() {
        panic!(
            "{} step(s) failed, {} parsing error(s), {} hook error(s)",
            writer.failed_steps(),
            writer.parsing_errors(),
            writer.hook_errors()
        );
    }
}
