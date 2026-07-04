//! Fuzz target for the workflow expression language: the parser and
//! evaluator must never panic on operator-authored input — any string is
//! either a value or a structured error.
//!
//! Run with `cargo +nightly fuzz run expr_parse` from
//! `services/session-runner/`.

#![no_main]

use chrono::{DateTime, Utc};
use libfuzzer_sys::fuzz_target;
use session_runner::expr::{EvalContext, Expression};

fuzz_target!(|data: &[u8]| {
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(expr) = Expression::parse(src) else {
        return;
    };
    // Fixed clock + a small blackboard exercising every JSON shape.
    let now = DateTime::<Utc>::from_timestamp(1_782_100_800, 0).unwrap_or_default();
    let session = serde_json::json!({
        "x": 1.5,
        "flag": true,
        "s": "1m30s",
        "t": "2026-07-03T04:10:00Z",
        "arr": [1, 2, 3],
        "o": {"k": null}
    });
    let ctx = EvalContext {
        session: Some(&session),
        ..EvalContext::new(now)
    };
    let _ = expr.eval(&ctx);
});
