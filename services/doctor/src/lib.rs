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
        let etc = Path::new("/etc/rusty-photon");
        if etc.is_dir() {
            debug!("using the packaged /etc/rusty-photon config tree");
            return Ok(etc.to_path_buf());
        }
    }
    let dir = rusty_photon_config::default_config_dir()
        .map_err(|e| format!("could not resolve a config directory: {e}"))?;
    debug!(dir = %dir.display(), "using the platform default config directory");
    Ok(dir)
}

/// Run the whole diagnosis: scan, check, report.
pub fn diagnose(config_dir: PathBuf, facts: PlatformFacts) -> Report {
    debug!(config_dir = %config_dir.display(), platform = ?facts.platform,
           units = facts.units.len(), "gathering context");
    let ctx = checks::Context::gather(config_dir, facts);
    let checks = checks::run_all(&ctx);
    Report::new(ctx.mode, ctx.config_dir, checks)
}
