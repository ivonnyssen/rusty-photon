//! BDD entry point for `ui-htmx`. Each scenario spawns the real `ui-htmx`
//! binary and a real `dsd-fp2` driver (mock hardware) and drives the BFF over
//! HTTP, so the `bdd_infra::bdd_main!` macro is used (child-process spawning,
//! skipped under Miri). Both binaries must be pre-built with `--all-features`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]

#[path = "bdd/browser.rs"]
mod browser;

#[path = "bdd/dom.rs"]
mod dom;

#[path = "bdd/snapshot.rs"]
mod snapshot;

#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

bdd_infra::bdd_main! {
    use cucumber::World as _;
    use world::UiWorld;

    UiWorld::cucumber()
        .after(|_feature, _rule, _scenario, _finished, maybe_world| {
            Box::pin(async move {
                if let Some(world) = maybe_world {
                    // Teardown order is load-bearing (UI-testing plan §10): quit
                    // the browser FIRST so the WebDriver session closes (and
                    // geckodriver tears Firefox down) before the BFF stops — a
                    // live session holds connections to the BFF open. Then stop
                    // the BFF (so it stops calling the driver), then the driver.
                    if let Some(browser) = world.browser.take() {
                        browser.quit().await;
                    }
                    if let Some(ui) = world.ui.as_mut() {
                        ui.stop().await;
                    }
                    if let Some(driver) = world.driver.as_mut() {
                        driver.stop().await;
                    }
                }
            })
        })
        // Scenario filter (the `_and_exit` variant: a bare `filter_run` lets
        // failures exit 0 under `harness = false` — see testing.md §2.7):
        //   * `@wip` is never run in the default suite (test-first artifacts).
        //   * `@browser` needs a real Firefox + geckodriver and is advisory;
        //     opt in with `UI_BROWSER_TESTS=1`. Gating on an env var (not a
        //     cargo feature) keeps browser flake out of the `--all-features`
        //     required gate while leaving `thirtyfour` always compiled.
        .filter_run_and_exit("tests/features", |feature, _rule, scenario| {
            let tagged = |tag: &str| {
                let want = tag.trim_start_matches('@');
                let matches = |t: &String| t.trim_start_matches('@') == want;
                feature.tags.iter().any(matches) || scenario.tags.iter().any(matches)
            };
            if tagged("wip") {
                return false;
            }
            if tagged("browser") && std::env::var("UI_BROWSER_TESTS").is_err() {
                return false;
            }
            true
        })
        .await;
}
