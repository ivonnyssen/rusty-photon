//! BDD step definitions for target-store progress derivation
//! (`target_store_progress.feature`, rp.md § Target Store → Progress
//! derivation — *(planned, P1)*, not yet implemented; scenarios are
//! tagged `@wip`).
//!
//! Scoped to what's fully decided today: the reshaped per-goal
//! `{filter, binning, exposure, good, total, desired}` progress type,
//! against targets with no captured frames (`good`/`total` both 0).
//! Scenarios exercising actual on-disk good-vs-rejected frame counting
//! need the grading plugin's sidecar section shape, which
//! rp-targets.md's MVP scope explicitly defers ("the grading plugin
//! itself") — those scenarios land once that shape is decided.

use cucumber::gherkin::Step;
use cucumber::then;
use serde_json::Value;

use crate::world::RpWorld;

// "the MCP client calls \"get_session_progress\"" is registered
// globally in ephemeris_steps.rs. Reused here, not redefined.

#[then(expr = "the reported progress should be exactly:")]
fn reported_progress_exactly(world: &mut RpWorld, step: &Step) {
    let expected = progress_rows_from_table(step);
    let payload = world
        .last_tool_result
        .as_ref()
        .expect("no tool result")
        .as_ref()
        .expect("expected tool call to succeed");
    let actual = payload
        .get("progress")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("get_target result missing `progress` array: {payload}"));
    assert_eq!(actual, &expected, "get_target progress");
}

#[then(expr = "the progress for target {string} should be exactly:")]
fn progress_for_target_exactly(world: &mut RpWorld, target_slug: String, step: &Step) {
    let expected = progress_rows_from_table(step);
    let payload = world
        .last_tool_result
        .as_ref()
        .expect("no tool result")
        .as_ref()
        .expect("expected tool call to succeed");
    let progress = payload
        .get("progress")
        .and_then(|v| v.as_object())
        .unwrap_or_else(|| {
            panic!("get_session_progress result missing `progress` object: {payload}")
        });
    let actual = progress
        .get(&target_slug)
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| {
            panic!("get_session_progress missing target {target_slug:?}: {progress:?}")
        });
    assert_eq!(actual, &expected, "progress for target {target_slug}");
}

fn progress_rows_from_table(step: &Step) -> Vec<Value> {
    let table = step
        .table
        .as_ref()
        .expect("step requires a `| filter | binning | exposure | good | total | desired |` table");
    let mut rows = table.rows.iter();
    let header = rows.next().expect("progress table must have a header");
    assert_eq!(
        header.as_slice(),
        ["filter", "binning", "exposure", "good", "total", "desired"],
        "progress table header"
    );
    rows.map(|row| {
        serde_json::json!({
            "filter": row[0],
            "binning": row[1],
            "exposure": row[2],
            "good": row[3].parse::<u32>().expect("good must parse as u32"),
            "total": row[4].parse::<u32>().expect("total must parse as u32"),
            "desired": row[5].parse::<u32>().expect("desired must parse as u32"),
        })
    })
    .collect()
}
