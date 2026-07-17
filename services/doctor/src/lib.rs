//! rusty-photon-doctor: read-only diagnosis of a multi-service install.
//!
//! Design: docs/services/doctor.md. Plan and ownership decisions:
//! docs/plans/service-config-doctor.md, ADR-016. This crate links no
//! service crate: it knows the derived catalog, the two shared server
//! shapes, and the known cross-reference blocks; every other byte of every
//! config file is opaque.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

pub mod catalog;
pub mod checks;
pub mod facts;
pub mod fix;
pub mod render;
pub mod report;
pub mod scan;

use std::path::{Path, PathBuf};

use facts::PlatformFacts;
use report::Report;
use tracing::debug;

/// Resolve which config directory to diagnose (docs/services/doctor.md
/// §Config-root resolution): the explicit flag (which must exist), else the
/// packaged `/etc/rusty-photon` symlink, else the platform default the
/// services themselves resolve — which may not exist yet on a fresh host
/// and is then diagnosed as empty.
///
/// A packaged tree that exists but is **unreadable** is a hard error, not a
/// fall-through: the tree is owned by the `rusty-photon` user, and silently
/// diagnosing the invoking user's own empty config directory instead would
/// report seventeen missing configs on a perfectly healthy rig.
pub fn resolve_config_dir(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(dir) = explicit {
        if !dir.is_dir() {
            return Err(format!(
                "--config-dir {} does not exist or is not a directory",
                dir.display()
            ));
        }
        return Ok(dir);
    }
    #[cfg(unix)]
    {
        if let Some(resolved) = packaged_config_dir(Path::new("/etc/rusty-photon"))? {
            return Ok(resolved);
        }
    }
    let dir = rusty_photon_config::default_config_dir()
        .map_err(|e| format!("could not resolve a config directory: {e}"))?;
    debug!(dir = %dir.display(), "using the platform default config directory");
    Ok(dir)
}

/// The packaged-tree leg of the resolution: `Ok(Some)` when the symlink
/// exists and is traversable, `Ok(None)` to fall through (absent, or
/// dangling — the packages were removed), `Err` when it exists but this
/// user cannot read it.
#[cfg(unix)]
fn packaged_config_dir(packaged: &Path) -> Result<Option<PathBuf>, String> {
    if packaged.symlink_metadata().is_err() {
        return Ok(None);
    }
    match std::fs::read_dir(packaged) {
        Ok(_) => {
            debug!(dir = %packaged.display(), "using the packaged config tree");
            Ok(Some(packaged.to_path_buf()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Err(format!(
            "{} exists but is not readable by this user — a packaged install's \
             config tree is owned by the rusty-photon user, so run doctor with \
             elevated privileges (e.g. sudo)",
            packaged.display()
        )),
        Err(e) => {
            debug!(dir = %packaged.display(), error = %e,
                   "packaged config tree unusable (dangling symlink?) — falling through");
            Ok(None)
        }
    }
}

/// Run the whole diagnosis: scan, check, report.
pub fn diagnose(config_dir: PathBuf, facts: PlatformFacts) -> Report {
    debug!(config_dir = %config_dir.display(), platform = ?facts.platform,
           units = facts.units.len(), "gathering context");
    let ctx = checks::Context::gather(config_dir, facts);
    let checks = checks::run_all(&ctx);
    Report::new(ctx.mode, ctx.config_dir, checks)
}

/// One fix can unlock the next (a freed default port makes another
/// collision fixable), so `--fix` iterates plan→apply to a fixpoint. The
/// cap is a runaway backstop, far above any real chain — ops are
/// idempotent, so even a pathological planner converges to no-ops.
const MAX_FIX_ROUNDS: usize = 4;

/// Diagnose, apply the machine-applicable fixes, re-diagnose — repeated
/// until a round plans nothing — and report the post-fix state plus what
/// was written. `Err` means a fix write itself failed (exit 2 territory);
/// the diagnosis outcome stays in the report.
pub fn diagnose_and_fix(config_dir: PathBuf, facts: PlatformFacts) -> Result<Report, String> {
    let mut applied = Vec::new();
    for round in 0..MAX_FIX_ROUNDS {
        let report = diagnose(config_dir.clone(), facts.clone());
        let planned: usize = report.checks.iter().map(|c| c.fixes.len()).sum();
        if planned == 0 {
            debug!(round, applied = applied.len(), "fix rounds converged");
            return Ok(report.with_fixes_applied(applied));
        }
        let round_applied = fix::apply_fixes(&config_dir, &report.checks)?;
        if round_applied.is_empty() {
            // Planned targets were already gone (a concurrent edit landed
            // between diagnosis and apply). Nothing was written, but the
            // diagnosis in hand is stale now — loop so the returned report
            // is always a fresh post-state diagnosis.
            continue;
        }
        applied.extend(round_applied);
    }
    Ok(diagnose(config_dir, facts).with_fixes_applied(applied))
}

#[cfg(all(test, unix))]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn test_packaged_dir_is_used_when_traversable() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = packaged_config_dir(dir.path()).unwrap();
        assert_eq!(resolved, Some(dir.path().to_path_buf()));
    }

    #[test]
    fn test_absent_and_dangling_packaged_dirs_fall_through() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            packaged_config_dir(&dir.path().join("absent")).unwrap(),
            None
        );
        let dangling = dir.path().join("etc-rusty-photon");
        std::os::unix::fs::symlink(dir.path().join("gone"), &dangling).unwrap();
        assert_eq!(packaged_config_dir(&dangling).unwrap(), None);
    }

    #[test]
    fn test_unreadable_packaged_dir_demands_privileges() {
        let dir = tempfile::tempdir().unwrap();
        let sealed = dir.path().join("sealed");
        std::fs::create_dir(&sealed).unwrap();
        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o000)).unwrap();
        let result = packaged_config_dir(&sealed);
        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o755)).unwrap();
        if result.is_ok() {
            return; // running privileged — mode 000 is still readable by root
        }
        let err = result.unwrap_err();
        assert!(err.contains("sudo"), "{err}");
    }

    #[test]
    fn test_explicit_config_dir_must_exist() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_config_dir(Some(dir.path().join("nope"))).unwrap_err();
        assert!(err.contains("--config-dir"), "{err}");
        let ok = resolve_config_dir(Some(dir.path().to_path_buf())).unwrap();
        assert_eq!(ok, dir.path());
    }
}
