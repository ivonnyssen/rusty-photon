//! Host facts from the platform's service manager (docs/services/doctor.md
//! §Platform inspectors).
//!
//! All service-manager knowledge is gathered once into [`PlatformFacts`] —
//! a platform-neutral inventory plus the platform-specific facts where they
//! exist. Real gathering shells out to `systemctl` / PowerShell /
//! `brew services`, read-only; a host whose service manager is absent or
//! unqueryable degrades to an empty inventory (a dev checkout), never an
//! error. Under the `mock` feature the whole struct can instead be loaded
//! from a JSON file so tests stage any host state on any OS.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use tracing::debug;

/// Which service manager's vocabulary the facts speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Linux,
    Windows,
    Macos,
}

/// One installed `rusty-photon-*` unit / service / formula.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnitFacts {
    /// The unit stem, e.g. `rusty-photon-qhy-focuser` (no `.service`).
    pub name: String,
    /// Enabled to start at boot (`enabled` unit-file state, Automatic start
    /// type, registered brew service).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The `ConditionPathExists=` path from the unit file, when the unit is
    /// gated on one. systemd only.
    #[serde(default)]
    pub condition_path: Option<PathBuf>,
    /// The service manager's own name for the unit when it differs from the
    /// stem — brew's nightly-channel formulas (`rusty-photon-<svc>-nightly`).
    /// Remediation text must use this name; catalog joins use `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_name: Option<String>,
    /// The unit's `SupplementaryGroups=` names. systemd only — and the
    /// only place group membership exists in this stack: no packaging step
    /// ever edits `/etc/group`, so the hardware access checks judge
    /// against this, never against login groups.
    #[serde(default)]
    pub supplementary_groups: Vec<String>,
}

fn default_true() -> bool {
    true
}

/// Everything doctor learns from the host's service manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformFacts {
    pub platform: Platform,
    /// The installed `rusty-photon-*` units. Empty means no packaged
    /// services — a dev checkout, diagnosed config-only.
    #[serde(default)]
    pub units: Vec<UnitFacts>,
    /// Whether a polkit rule grants sentinel's user the
    /// `org.freedesktop.systemd1.manage-units` action for `rusty-photon-*`
    /// units. `None` where the fact does not exist (non-Linux) or was not
    /// gathered; the privilege check runs only on `Some`.
    #[serde(default)]
    pub polkit_grants_sentinel_restart: Option<bool>,
    /// The device-surface facts (docs/services/doctor.md §Hardware).
    /// Staged facts files may carry them; on a real run they stay `None`
    /// here and the hardware family gathers them itself — it needs the
    /// scanned configs to know what to probe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware: Option<rusty_photon_doctor_checks::HardwareFacts>,
    /// True only on facts gathered from the real host. A staged facts
    /// file without a `hardware` object means "this scenario has no
    /// hardware story" — probing the real host underneath it would make
    /// every mock scenario's outcome depend on the machine running it —
    /// so the hardware family probes only when this is set.
    #[serde(skip)]
    pub probe_hardware: bool,
}

impl PlatformFacts {
    /// Look up an installed unit by stem.
    pub fn unit(&self, name: &str) -> Option<&UnitFacts> {
        self.units.iter().find(|u| u.name == name)
    }

    /// Load facts from a JSON file (the `--platform-facts` test affordance).
    #[cfg(feature = "mock")]
    pub fn load(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("could not read platform facts {}: {e}", path.display()))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("platform facts {} are invalid: {e}", path.display()))
    }

    /// Gather facts from the running host, read-only. Inspector failures
    /// (no service manager on PATH, unexpected output) degrade to an empty
    /// inventory with a `debug!` trail, because "no packaged services" is a
    /// legitimate host state, not an error.
    pub fn gather() -> Self {
        #[cfg(target_os = "linux")]
        {
            gather_linux()
        }
        #[cfg(target_os = "windows")]
        {
            gather_windows()
        }
        #[cfg(target_os = "macos")]
        {
            gather_macos()
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
        {
            PlatformFacts {
                platform: Platform::Linux,
                units: Vec::new(),
                polkit_grants_sentinel_restart: None,
                hardware: None,
                probe_hardware: true,
            }
        }
    }
}

/// Run a command and return stdout on success; `None` (with a `debug!`
/// trail) when the binary is missing or exits non-zero.
fn run(cmd: &mut Command) -> Option<String> {
    match cmd.output() {
        Ok(output) if output.status.success() => {
            Some(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(output) => {
            debug!(
                command = ?cmd,
                status = ?output.status,
                stderr = %String::from_utf8_lossy(&output.stderr),
                "service-manager query failed"
            );
            None
        }
        Err(e) => {
            debug!(command = ?cmd, error = %e, "service-manager query could not run");
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn gather_linux() -> PlatformFacts {
    let units = list_systemd_units();
    let polkit = if units.iter().any(|u| u.name == "rusty-photon-sentinel") {
        Some(polkit_grants_sentinel_restart(&[
            Path::new("/etc/polkit-1/rules.d"),
            Path::new("/usr/share/polkit-1/rules.d"),
        ]))
    } else {
        None
    };
    PlatformFacts {
        platform: Platform::Linux,
        units,
        polkit_grants_sentinel_restart: polkit,
        hardware: None,
        probe_hardware: true,
    }
}

#[cfg(target_os = "linux")]
fn list_systemd_units() -> Vec<UnitFacts> {
    let Some(listing) = run(Command::new("systemctl").args([
        "list-unit-files",
        "--type=service",
        "--no-legend",
        "--plain",
        "rusty-photon-*",
    ])) else {
        return Vec::new();
    };
    parse_unit_file_listing(&listing)
        .into_iter()
        .map(|(name, enabled)| {
            let unit_file = run(Command::new("systemctl").args(["cat", &name]));
            UnitFacts {
                name,
                enabled,
                condition_path: unit_file.as_deref().and_then(parse_condition_path),
                source_name: None,
                supplementary_groups: unit_file
                    .as_deref()
                    .map(parse_supplementary_groups)
                    .unwrap_or_default(),
            }
        })
        .collect()
}

/// Parse `systemctl list-unit-files --no-legend --plain` lines
/// (`<unit> <state> [preset]`) into `(stem, enabled)` pairs.
pub fn parse_unit_file_listing(listing: &str) -> Vec<(String, bool)> {
    listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let unit = fields.next()?;
            let state = fields.next().unwrap_or("");
            let stem = unit.strip_suffix(".service").unwrap_or(unit);
            if !stem.starts_with("rusty-photon-") {
                return None;
            }
            // `enabled-runtime` is enabled-until-reboot — still a unit that
            // will start, so the enabled-gated checks apply to it too.
            let enabled = state == "enabled" || state == "enabled-runtime";
            Some((stem.to_string(), enabled))
        })
        .collect()
}

/// Extract the `SupplementaryGroups=` names from a unit file dump
/// (`systemctl cat`). Multiple assignments accumulate; an empty
/// assignment resets the list — systemd's own semantics, which matter
/// when a drop-in overrides the packaged unit.
pub fn parse_supplementary_groups(unit_file: &str) -> Vec<String> {
    let mut groups: Vec<String> = Vec::new();
    for line in unit_file.lines() {
        let Some(value) = line.trim().strip_prefix("SupplementaryGroups=") else {
            continue;
        };
        if value.trim().is_empty() {
            groups.clear();
            continue;
        }
        for name in value.split_whitespace() {
            if !groups.iter().any(|g| g == name) {
                groups.push(name.to_string());
            }
        }
    }
    groups
}

/// Extract the `ConditionPathExists=` path from a unit file dump
/// (`systemctl cat`). Negated (`!`-prefixed) conditions are not gates on a
/// file the operator must provide, so they are ignored.
pub fn parse_condition_path(unit_file: &str) -> Option<PathBuf> {
    unit_file.lines().find_map(|line| {
        let value = line.trim().strip_prefix("ConditionPathExists=")?.trim();
        if value.is_empty() || value.starts_with('!') {
            return None;
        }
        Some(PathBuf::from(value))
    })
}

/// Scan polkit rules directories for a rule that mentions the
/// `manage-units` action, the `rusty-photon-` unit prefix, and the quoted
/// `"rusty-photon"` user literal (the shape of the rule the sentinel
/// packages ship). A heuristic — polkit rules are JavaScript and doctor
/// does not execute them — and the check's detail text says so.
pub fn polkit_grants_sentinel_restart(rules_dirs: &[&Path]) -> bool {
    rules_dirs.iter().any(|dir| {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        entries.filter_map(Result::ok).any(|entry| {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "rules") {
                return false;
            }
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    content.contains("org.freedesktop.systemd1.manage-units")
                        && content.contains("rusty-photon-")
                        && content.contains("\"rusty-photon\"")
                }
                Err(e) => {
                    debug!(path = %path.display(), error = %e, "unreadable polkit rules file");
                    false
                }
            }
        })
    })
}

#[cfg(target_os = "windows")]
fn gather_windows() -> PlatformFacts {
    let units = run(Command::new("powershell.exe").args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "Get-Service -Name 'rusty-photon-*' -ErrorAction SilentlyContinue | \
         ForEach-Object { \"$($_.Name)`t$($_.StartType)\" }",
    ]))
    .map(|listing| parse_windows_service_listing(&listing))
    .unwrap_or_default();
    PlatformFacts {
        platform: Platform::Windows,
        units,
        polkit_grants_sentinel_restart: None,
        hardware: None,
        probe_hardware: true,
    }
}

/// Parse `Name<TAB>StartType` lines from the `Get-Service` query.
pub fn parse_windows_service_listing(listing: &str) -> Vec<UnitFacts> {
    listing
        .lines()
        .filter_map(|line| {
            let (name, start_type) = line.trim().split_once('\t')?;
            if !name.starts_with("rusty-photon-") {
                return None;
            }
            Some(UnitFacts {
                name: name.to_string(),
                enabled: start_type.trim().starts_with("Automatic"),
                condition_path: None,
                source_name: None,
                supplementary_groups: Vec::new(),
            })
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn gather_macos() -> PlatformFacts {
    let units = run(Command::new("brew").args(["services", "list"]))
        .map(|listing| parse_brew_services_listing(&listing))
        .unwrap_or_default();
    PlatformFacts {
        platform: Platform::Macos,
        units,
        polkit_grants_sentinel_restart: None,
        hardware: None,
        probe_hardware: true,
    }
}

/// Parse `brew services list` (`Name Status User File` columns, header
/// line first) down to the registered `rusty-photon-*` formulas. The
/// nightly channel's formulas are `rusty-photon-<svc>-nightly` but install
/// the same binaries and services, so the channel suffix is stripped to
/// recover the unit stem.
pub fn parse_brew_services_listing(listing: &str) -> Vec<UnitFacts> {
    listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let name = fields.next()?;
            let status = fields.next().unwrap_or("none");
            if !name.starts_with("rusty-photon-") {
                return None;
            }
            let stem = name.strip_suffix("-nightly").unwrap_or(name);
            let source_name = (stem != name).then(|| name.to_string());
            Some(UnitFacts {
                name: stem.to_string(),
                source_name,
                // `none` means installed but never registered to start;
                // anything else (started/scheduled/error/stopped) is a
                // service the operator has wired up.
                enabled: status != "none",
                condition_path: None,
                supplementary_groups: Vec::new(),
            })
        })
        .collect()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn test_unit_file_listing_parses_states_and_filters_foreign_units() {
        let listing = "rusty-photon-qhy-focuser.service enabled enabled\n\
                       rusty-photon-rp.service disabled enabled\n\
                       rusty-photon-sentinel.service enabled-runtime enabled\n\
                       ssh.service enabled enabled\n";
        let units = parse_unit_file_listing(listing);
        assert_eq!(
            units,
            vec![
                ("rusty-photon-qhy-focuser".to_string(), true),
                ("rusty-photon-rp".to_string(), false),
                ("rusty-photon-sentinel".to_string(), true),
            ]
        );
    }

    #[test]
    fn test_condition_path_parses_plain_and_ignores_negated() {
        let unit = "# /usr/lib/systemd/system/x.service\n\
                    [Unit]\n\
                    ConditionPathExists=/etc/rusty-photon/plate-solver.json\n";
        assert_eq!(
            parse_condition_path(unit),
            Some(PathBuf::from("/etc/rusty-photon/plate-solver.json"))
        );
        assert_eq!(parse_condition_path("ConditionPathExists=!/x\n"), None);
        assert_eq!(parse_condition_path("[Service]\nUser=rusty-photon\n"), None);
    }

    #[test]
    fn test_polkit_scan_matches_the_shipped_rule_shape() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!polkit_grants_sentinel_restart(&[dir.path()]));
        std::fs::write(
            dir.path().join("50-rusty-photon-sentinel.rules"),
            r#"polkit.addRule(function (action, subject) {
                if (action.id == "org.freedesktop.systemd1.manage-units" &&
                    subject.user == "rusty-photon" &&
                    unit.indexOf("rusty-photon-") == 0) { return polkit.Result.YES; }
            });"#,
        )
        .unwrap();
        assert!(polkit_grants_sentinel_restart(&[dir.path()]));
        // A rule for some other user does not count as sentinel's grant.
        let dir2 = tempfile::tempdir().unwrap();
        std::fs::write(
            dir2.path().join("50-other.rules"),
            r#"action.id == "org.freedesktop.systemd1.manage-units" &&
               subject.user == "operator" && unit.indexOf("rusty-photon-") == 0"#,
        )
        .unwrap();
        assert!(!polkit_grants_sentinel_restart(&[dir2.path()]));
        // Non-.rules files never match.
        let dir3 = tempfile::tempdir().unwrap();
        std::fs::write(
            dir3.path().join("readme.txt"),
            "org.freedesktop.systemd1.manage-units \"rusty-photon\" rusty-photon-",
        )
        .unwrap();
        assert!(!polkit_grants_sentinel_restart(&[dir3.path()]));
    }

    #[test]
    fn test_windows_service_listing_parses_start_types() {
        let listing = "rusty-photon-rp\tAutomatic\n\
                       rusty-photon-sentinel\tManual\n\
                       Spooler\tAutomatic\n";
        let units = parse_windows_service_listing(listing);
        assert_eq!(units.len(), 2);
        assert!(units[0].enabled);
        assert_eq!(units[0].name, "rusty-photon-rp");
        assert!(!units[1].enabled);
    }

    #[test]
    fn test_brew_services_listing_skips_header_and_foreign_formulas() {
        let listing = "Name              Status  User File\n\
                       postgresql        started igor ~/Library/...\n\
                       rusty-photon-rp   started igor ~/Library/...\n\
                       rusty-photon-ui-htmx none\n";
        let units = parse_brew_services_listing(listing);
        assert_eq!(units.len(), 2);
        assert!(units[0].enabled);
        assert!(!units[1].enabled);
    }

    #[test]
    fn test_brew_nightly_formula_names_normalize_to_the_unit_stem() {
        let listing = "rusty-photon-sentinel-nightly started igor ~/Library/...\n\
                       rusty-photon-rp none\n";
        let units = parse_brew_services_listing(listing);
        assert_eq!(units[0].name, "rusty-photon-sentinel");
        assert!(units[0].enabled);
        assert_eq!(
            units[0].source_name.as_deref(),
            Some("rusty-photon-sentinel-nightly"),
            "remediation text needs the installable formula name"
        );
        assert_eq!(units[1].source_name, None, "stable names carry no alias");
    }

    #[cfg(unix)]
    #[test]
    fn test_run_captures_stdout_and_degrades_on_failure() {
        let out = run(Command::new("echo").arg("systemctl-stand-in")).unwrap();
        assert!(out.contains("systemctl-stand-in"));
        assert!(run(&mut Command::new("false")).is_none(), "non-zero exit");
        assert!(
            run(&mut Command::new("/nonexistent/doctor-test-binary")).is_none(),
            "missing binary"
        );
    }

    /// Exercises the real host-gathering path end to end: on a systemd host
    /// it queries the service manager, elsewhere the query fails and
    /// degrades to an empty inventory — both are legitimate outcomes, and
    /// every unit that does come back is a rusty-photon stem.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_gather_reports_host_facts_without_panicking() {
        let facts = PlatformFacts::gather();
        assert_eq!(facts.platform, Platform::Linux);
        for unit in &facts.units {
            assert!(unit.name.starts_with("rusty-photon-"), "{}", unit.name);
        }
        if facts.unit("rusty-photon-sentinel").is_none() {
            assert!(
                facts.polkit_grants_sentinel_restart.is_none(),
                "the polkit fact is gathered only when sentinel is installed"
            );
        }
    }

    #[test]
    fn test_facts_parse_permissively_and_unit_lookup_works() {
        let facts: PlatformFacts = serde_json::from_str(
            r#"{ "platform": "linux",
                 "units": [ { "name": "rusty-photon-rp" } ],
                 "future_field": true }"#,
        )
        .unwrap();
        assert_eq!(facts.platform, Platform::Linux);
        let unit = facts.unit("rusty-photon-rp").unwrap();
        assert!(unit.enabled, "enabled defaults to true");
        assert!(unit.condition_path.is_none());
        assert!(facts.unit("rusty-photon-ghost").is_none());
        assert!(facts.polkit_grants_sentinel_restart.is_none());
    }
}
