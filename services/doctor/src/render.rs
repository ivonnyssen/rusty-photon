//! Human-readable report rendering. Warnings and failures print in full;
//! passing checks are summarized by count — the operator reads this at the
//! rig, so signal density beats completeness (the full list is in --json).

use std::fmt::Write as _;

use crate::report::{Mode, Report, Status};

pub fn render(report: &Report) -> String {
    let mut out = String::new();
    let mode = match report.mode {
        Mode::Packaged => "packaged",
        Mode::ConfigOnly => "config-only (no rusty-photon units installed)",
    };
    let _ = writeln!(
        out,
        "rusty-photon-doctor {} — mode: {mode}, config dir: {}",
        report.doctor_version,
        report.config_dir.display()
    );
    let mut ok = 0usize;
    let mut warn = 0usize;
    let mut fail = 0usize;
    for check in &report.checks {
        let label = match check.status {
            Status::Ok => {
                ok += 1;
                continue;
            }
            Status::Warn => {
                warn += 1;
                "WARN"
            }
            Status::Fail | Status::Unknown => {
                fail += 1;
                "FAIL"
            }
        };
        let scope = check
            .service
            .as_deref()
            .map(|s| format!(" ({s})"))
            .unwrap_or_default();
        let _ = writeln!(out, "{label} {}{scope}: {}", check.name, check.detail);
        if let Some(suggestion) = &check.suggestion {
            let _ = writeln!(out, "     fix: {suggestion}");
        }
    }
    let _ = writeln!(out, "summary: {ok} ok, {warn} warn, {fail} fail");
    out
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::report::Check;

    #[test]
    fn test_render_summarizes_ok_and_prints_failures_in_full() {
        let report = Report::new(
            Mode::Packaged,
            "/etc/rusty-photon".into(),
            vec![
                Check::ok("config.server-shape", Some("rp".to_string()), "fine"),
                Check::fail(
                    "ports.collision",
                    Some("qhy-focuser".to_string()),
                    "port clash",
                    Some("change a port".to_string()),
                ),
                Check::warn("urls.sentinel-suffix", None, "no suffix", None),
            ],
        );
        let text = render(&report);
        assert!(text.contains("summary: 1 ok, 1 warn, 1 fail"), "{text}");
        assert!(text.contains("FAIL ports.collision (qhy-focuser): port clash"));
        assert!(text.contains("fix: change a port"));
        assert!(text.contains("WARN urls.sentinel-suffix: no suffix"));
        assert!(
            !text.contains("config.server-shape"),
            "ok checks are counted, not listed: {text}"
        );
    }
}
