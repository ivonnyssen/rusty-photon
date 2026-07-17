//! Applying machine-applicable fixes (docs/services/doctor.md §Repair).
//!
//! Fixes are primitive JSON-pointer operations ([`FixOp`]) the checks
//! planned. They are grouped per service file and applied as one
//! read-modify-write through `rusty_photon_config::save` — the same atomic
//! temp→fsync→rename→fsync-dir path the services' own `config.apply` uses —
//! so a crash mid-fix never corrupts a config, and every field doctor does
//! not touch is preserved (the mutation is on the raw JSON value, not a
//! typed round-trip; `save` normalizes formatting to the same
//! pretty-printed shape `config.apply` writes).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use tracing::{debug, warn};

use crate::report::{AppliedFix, Check, FixOp};

/// Apply every fix planned by `checks`, grouped per service file. Returns
/// the ops actually applied (an op whose target is already gone is skipped
/// silently — another fix round or a concurrent edit got there first).
/// A read or write error on one file aborts with an error: half-applied
/// repair must be reported, not glossed over.
pub fn apply_fixes(config_dir: &Path, checks: &[Check]) -> Result<Vec<AppliedFix>, String> {
    let mut by_service: BTreeMap<String, Vec<(String, FixOp)>> = BTreeMap::new();
    for check in checks {
        for op in &check.fixes {
            let Some(service) = op.service() else {
                warn!("skipping a fix op this doctor build does not know");
                continue;
            };
            by_service
                .entry(service.to_string())
                .or_default()
                .push((check.name.clone(), op.clone()));
        }
    }

    let mut applied = Vec::new();
    for (service, ops) in by_service {
        let path = config_dir.join(format!("{service}.json"));
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot re-read {} to fix it: {e}", path.display()))?;
        let mut value: Value = serde_json::from_str(&content)
            .map_err(|e| format!("{} is no longer valid JSON: {e}", path.display()))?;

        let mut wrote_any = false;
        for (check_name, op) in ops {
            if apply_op(&mut value, &op) {
                debug!("applied: {op}");
                wrote_any = true;
                applied.push(AppliedFix {
                    check: check_name,
                    op,
                });
            } else {
                debug!("fix target already gone, skipping: {op}");
            }
        }
        if wrote_any {
            rusty_photon_config::save(&path, &value)
                .map_err(|e| format!("could not write {}: {e}", path.display()))?;
        }
    }
    Ok(applied)
}

/// Apply one op to a config value. Returns whether anything changed. Ops
/// never create intermediate structure: the pointer was derived from the
/// same file moments ago, so a missing parent means the target is already
/// gone.
fn apply_op(value: &mut Value, op: &FixOp) -> bool {
    match op {
        FixOp::SetNumber {
            pointer, value: n, ..
        } => set(value, pointer, Value::from(*n)),
        FixOp::SetString {
            pointer, value: s, ..
        } => set(value, pointer, Value::from(s.as_str())),
        FixOp::RemoveKey { pointer, .. } => remove(value, pointer),
        FixOp::Unknown => false,
    }
}

fn set(value: &mut Value, pointer: &str, new: Value) -> bool {
    match value.pointer_mut(pointer) {
        Some(slot) if *slot != new => {
            *slot = new;
            true
        }
        _ => false,
    }
}

fn remove(value: &mut Value, pointer: &str) -> bool {
    let (parent_ptr, key) = match pointer.rsplit_once('/') {
        Some(split) => split,
        None => return false,
    };
    let key = key.replace("~1", "/").replace("~0", "~");
    match value.pointer_mut(parent_ptr) {
        Some(Value::Object(map)) => map.remove(&key).is_some(),
        _ => false,
    }
}

/// Escape one JSON-pointer reference token (RFC 6901).
pub fn escape_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    fn sample() -> Value {
        serde_json::json!({
            "server": { "port": 11119 },
            "drivers": { "a/b": { "base_url": "http://x" } },
            "keep": [1, 2, 3]
        })
    }

    #[test]
    fn test_set_changes_and_reports_noop() {
        let mut v = sample();
        assert!(set(&mut v, "/server/port", Value::from(11113u64)));
        assert_eq!(v["server"]["port"], 11113);
        assert!(
            !set(&mut v, "/server/port", Value::from(11113u64)),
            "setting the same value is a no-op"
        );
        assert!(
            !set(&mut v, "/absent/port", Value::from(1u64)),
            "a missing parent is never created"
        );
    }

    #[test]
    fn test_remove_deletes_and_tolerates_absent() {
        let mut v = sample();
        assert!(remove(&mut v, "/server/port"));
        assert!(v["server"].get("port").is_none());
        assert!(!remove(&mut v, "/server/port"), "second removal is a no-op");
        assert!(!remove(&mut v, "/absent/key"));
        assert!(!remove(&mut v, "/keep/0"), "array elements are not keys");
    }

    #[test]
    fn test_remove_unescapes_pointer_tokens() {
        let mut v = sample();
        assert!(remove(&mut v, &format!("/drivers/{}", escape_token("a/b"))));
        assert!(v["drivers"].get("a/b").is_none());
    }

    #[test]
    fn test_apply_fixes_groups_writes_and_preserves_untouched_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("qhy-focuser.json");
        std::fs::write(
            &path,
            r#"{ "server": { "port": 11119 }, "device_overrides": { "keep": true } }"#,
        )
        .unwrap();
        let checks = vec![Check::fail(
            "ports.collision",
            Some("qhy-focuser".to_string()),
            "collision",
            None,
        )
        .with_fixes(vec![FixOp::SetNumber {
            service: "qhy-focuser".to_string(),
            pointer: "/server/port".to_string(),
            value: 11113,
        }])];

        let applied = apply_fixes(dir.path(), &checks).unwrap();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].check, "ports.collision");

        let back: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back["server"]["port"], 11113);
        assert_eq!(
            back["device_overrides"]["keep"], true,
            "untouched fields survive"
        );
    }

    #[test]
    fn test_apply_fixes_skips_already_gone_targets() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sentinel.json"), r#"{ "server": {} }"#).unwrap();
        let checks = vec![Check::fail(
            "config.retired-keys",
            Some("sentinel".to_string()),
            "retired",
            None,
        )
        .with_fixes(vec![FixOp::RemoveKey {
            service: "sentinel".to_string(),
            pointer: "/services".to_string(),
        }])];
        let applied = apply_fixes(dir.path(), &checks).unwrap();
        assert!(applied.is_empty(), "nothing to remove, nothing recorded");
    }

    #[test]
    fn test_apply_fixes_errors_when_the_file_vanished() {
        let dir = tempfile::tempdir().unwrap();
        let checks = vec![Check::fail(
            "config.retired-keys",
            Some("sentinel".to_string()),
            "retired",
            None,
        )
        .with_fixes(vec![FixOp::RemoveKey {
            service: "sentinel".to_string(),
            pointer: "/services".to_string(),
        }])];
        let err = apply_fixes(dir.path(), &checks).unwrap_err();
        assert!(err.contains("sentinel.json"), "{err}");
    }
}
