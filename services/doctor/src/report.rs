//! The doctor report schema (docs/services/doctor.md §Report).
//!
//! This schema crosses a binary boundary from D5 on (per-service `doctor`
//! subcommands emit it, central doctor aggregates it), so unlike every
//! config shape it parses **permissively**: unknown fields are tolerated,
//! missing ones default, and an unknown status from a newer binary degrades
//! to [`Status::Unknown`] instead of refusing the whole report.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The version of this schema; bumped on incompatible shape changes.
pub const SCHEMA_VERSION: u32 = 1;

/// How the run diagnosed the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// At least one `rusty-photon-*` unit is installed: the full check set.
    Packaged,
    /// No units — a dev checkout; config-only checks.
    #[default]
    ConfigOnly,
}

/// One check's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Ok,
    Warn,
    Fail,
    /// A status this doctor build does not know — a report from a newer
    /// binary. Never emitted, only parsed.
    #[serde(other)]
    Unknown,
}

/// One diagnosis: a stable name, the service it concerns (when
/// service-scoped), the outcome, and a human-readable detail. `suggestion`
/// carries a concrete remedy as text where doctor can offer one;
/// machine-applicable fixes arrive with D3.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(default)]
    pub status: Status,
    #[serde(default)]
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

impl Check {
    pub fn ok(name: &str, service: impl Into<Option<String>>, detail: impl Into<String>) -> Self {
        Self::new(name, service, Status::Ok, detail, None)
    }

    pub fn warn(
        name: &str,
        service: impl Into<Option<String>>,
        detail: impl Into<String>,
        suggestion: impl Into<Option<String>>,
    ) -> Self {
        Self::new(name, service, Status::Warn, detail, suggestion.into())
    }

    pub fn fail(
        name: &str,
        service: impl Into<Option<String>>,
        detail: impl Into<String>,
        suggestion: impl Into<Option<String>>,
    ) -> Self {
        Self::new(name, service, Status::Fail, detail, suggestion.into())
    }

    fn new(
        name: &str,
        service: impl Into<Option<String>>,
        status: Status,
        detail: impl Into<String>,
        suggestion: Option<String>,
    ) -> Self {
        Self {
            name: name.to_string(),
            service: service.into(),
            status,
            detail: detail.into(),
            suggestion,
        }
    }
}

/// The whole report. `ok` checks are included: an empty report must never
/// be mistaken for a clean one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub doctor_version: String,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub config_dir: PathBuf,
    #[serde(default)]
    pub checks: Vec<Check>,
}

impl Report {
    pub fn new(mode: Mode, config_dir: PathBuf, checks: Vec<Check>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            doctor_version: env!("CARGO_PKG_VERSION").to_string(),
            mode,
            config_dir,
            checks,
        }
    }

    /// True when at least one check failed — the exit-1 condition.
    /// [`Status::Unknown`] counts as a failure: it only appears when
    /// aggregating a newer binary's report, and treating an unrecognized
    /// outcome as clean would let the exit code disagree with the rendered
    /// report.
    pub fn has_failures(&self) -> bool {
        self.checks
            .iter()
            .any(|c| matches!(c.status, Status::Fail | Status::Unknown))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn test_report_round_trips() {
        let report = Report::new(
            Mode::Packaged,
            PathBuf::from("/etc/rusty-photon"),
            vec![Check::fail(
                "ports.collision",
                Some("qhy-focuser".to_string()),
                "qhy-focuser and dsd-fp2 both resolve to port 11113",
                Some("set a distinct server.port".to_string()),
            )],
        );
        let json = serde_json::to_string(&report).unwrap();
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema_version, SCHEMA_VERSION);
        assert_eq!(back.mode, Mode::Packaged);
        assert_eq!(back.checks[0].status, Status::Fail);
        assert!(back.has_failures());
    }

    #[test]
    fn test_report_from_a_newer_binary_degrades_instead_of_refusing() {
        let json = r#"{
            "schema_version": 3,
            "mode": "config-only",
            "checks": [
                { "name": "hardware.usb", "status": "degraded", "novel_field": 1 },
                { "name": "ports.collision", "status": "fail" }
            ],
            "novel_top_level": {}
        }"#;
        let report: Report = serde_json::from_str(json).unwrap();
        assert_eq!(report.checks[0].status, Status::Unknown);
        assert_eq!(report.checks[1].status, Status::Fail);
        assert!(report.has_failures());
        assert_eq!(report.doctor_version, "");
    }

    #[test]
    fn test_unknown_statuses_alone_count_as_failures() {
        let json = r#"{ "checks": [ { "name": "x", "status": "degraded" } ] }"#;
        let report: Report = serde_json::from_str(json).unwrap();
        assert!(
            report.has_failures(),
            "an unrecognized outcome must not exit clean"
        );
    }

    #[test]
    fn test_warnings_are_not_failures() {
        let report = Report::new(
            Mode::ConfigOnly,
            PathBuf::new(),
            vec![Check::warn("x", None, "d", None)],
        );
        assert!(!report.has_failures());
    }
}
