//! Step definitions for browser.feature (`@browser`) — Layer C / P3.
//!
//! These drive a real headless Firefox via WebDriver to prove behaviors only a
//! browser can establish: that `htmx.min.js` actually loads and executes the
//! declared swaps. They reuse the `Given a dsd-fp2 driver running …` setup from
//! [`super::config_page_steps`]; only the When/Then verbs here touch the browser.
//! The whole feature is gated behind `UI_BROWSER_TESTS=1` (see `tests/bdd.rs`).

use cucumber::{then, when};

use crate::world::UiWorld;

/// A generous bound for browser DOM polls: ~5s at 100ms/iter, well under the
/// nightly job's `timeout-minutes` cap but long enough for a real page load and
/// an htmx swap on a busy CI host.
const MAX_POLLS: usize = 50;

#[when("I load the dsd-fp2 config page in a browser")]
async fn load_in_browser(world: &mut UiWorld) {
    world.browser_goto("/config/dsd-fp2").await;
}

#[then("the browser renders the configuration form")]
async fn renders_form(world: &mut UiWorld) {
    // The Apply button is server-rendered inside the form; seeing it in the
    // *live* DOM proves Firefox fetched the page and rendered it.
    assert!(
        world
            .browser()
            .wait_present("button.primary", MAX_POLLS)
            .await,
        "the Apply button never appeared in the live DOM"
    );
}

#[when("I click the unlock link for cover_calibrator.unique_id")]
async fn click_unlock(world: &mut UiWorld) {
    world
        .browser()
        .click(r#"a[hx-get$="unlock=cover_calibrator.unique_id"]"#)
        .await;
}

#[then("the browser shows cover_calibrator.unique_id editable")]
async fn shows_unique_id_editable(world: &mut UiWorld) {
    // Proves real htmx executed the hx-get → outerHTML swap: the re-rendered
    // #config-card replaces the disabled identity input with an enabled one.
    assert!(
        world
            .browser()
            .wait_enabled(r#"input[name="cover_calibrator.unique_id"]"#, MAX_POLLS)
            .await,
        "cover_calibrator.unique_id never became editable after the htmx swap"
    );
}
