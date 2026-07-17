//! The derived service catalog (docs/services/doctor.md §The derived
//! catalog).
//!
//! Each packaged service's `pkg/doctor.toml` is embedded at build time and
//! parsed once on first access. The embed list below is the one hand-typed
//! encoding of the service set inside doctor; it is kept honest by the
//! `catalog_matches_the_packaging_tree` test (Cargo runs, walks
//! `services/*/pkg`) and by each service's own parity test against its
//! config defaults.

use std::sync::LazyLock;

use rusty_photon_server_config::doctor_toml::{self, ServerClass};

/// One packaged service the doctor knows about.
#[derive(Debug, Clone, Copy)]
pub struct CatalogEntry {
    /// The service name — the `services/<name>` directory, the `<name>.json`
    /// config file, and the `rusty-photon-<name>` unit stem.
    pub name: &'static str,
    /// Which shared `server` shape the config uses.
    pub class: ServerClass,
    /// The port the service defaults to when its config omits one.
    pub default_port: u16,
}

impl CatalogEntry {
    /// The platform-neutral unit stem: `rusty-photon-<name>` names the
    /// systemd unit, the Windows service, and the brew formula alike.
    pub fn unit_name(&self) -> String {
        format!("rusty-photon-{}", self.name)
    }

    /// The config file name inside the config directory.
    pub fn config_file(&self) -> String {
        format!("{}.json", self.name)
    }
}

/// The embedded `pkg/doctor.toml` files, alphabetical by service.
static RAW: &[(&str, &str)] = &[
    (
        "calibrator-flats",
        include_str!("../../calibrator-flats/pkg/doctor.toml"),
    ),
    ("dsd-fp2", include_str!("../../dsd-fp2/pkg/doctor.toml")),
    (
        "filemonitor",
        include_str!("../../filemonitor/pkg/doctor.toml"),
    ),
    (
        "pa-falcon-rotator",
        include_str!("../../pa-falcon-rotator/pkg/doctor.toml"),
    ),
    (
        "pa-scops-oag",
        include_str!("../../pa-scops-oag/pkg/doctor.toml"),
    ),
    (
        "phd2-guider",
        include_str!("../../phd2-guider/pkg/doctor.toml"),
    ),
    (
        "plate-solver",
        include_str!("../../plate-solver/pkg/doctor.toml"),
    ),
    (
        "ppba-driver",
        include_str!("../../ppba-driver/pkg/doctor.toml"),
    ),
    (
        "qhy-camera",
        include_str!("../../qhy-camera/pkg/doctor.toml"),
    ),
    (
        "qhy-focuser",
        include_str!("../../qhy-focuser/pkg/doctor.toml"),
    ),
    ("rp", include_str!("../../rp/pkg/doctor.toml")),
    ("sentinel", include_str!("../../sentinel/pkg/doctor.toml")),
    (
        "sky-survey-camera",
        include_str!("../../sky-survey-camera/pkg/doctor.toml"),
    ),
    (
        "star-adventurer-gti",
        include_str!("../../star-adventurer-gti/pkg/doctor.toml"),
    ),
    ("ui-htmx", include_str!("../../ui-htmx/pkg/doctor.toml")),
    (
        "zwo-camera",
        include_str!("../../zwo-camera/pkg/doctor.toml"),
    ),
    (
        "zwo-focuser",
        include_str!("../../zwo-focuser/pkg/doctor.toml"),
    ),
];

static CATALOG: LazyLock<Vec<CatalogEntry>> = LazyLock::new(|| {
    RAW.iter()
        .map(|(name, content)| {
            let meta = doctor_toml::parse(content)
                .unwrap_or_else(|e| panic!("embedded {name}/pkg/doctor.toml is invalid: {e}"));
            CatalogEntry {
                name,
                class: meta.class,
                default_port: meta.port,
            }
        })
        .collect()
});

/// Every packaged service, alphabetical.
pub fn catalog() -> &'static [CatalogEntry] {
    &CATALOG
}

/// Look up a service by name.
pub fn entry(name: &str) -> Option<&'static CatalogEntry> {
    catalog().iter().find(|e| e.name == name)
}

/// Look up a service by its `rusty-photon-<name>` unit stem (with or
/// without a `.service` suffix).
pub fn entry_for_unit(unit: &str) -> Option<&'static CatalogEntry> {
    let stem = unit.strip_suffix(".service").unwrap_or(unit);
    let name = stem.strip_prefix("rusty-photon-")?;
    entry(name)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn test_catalog_parses_and_ports_are_unique() {
        let catalog = catalog();
        assert!(!catalog.is_empty());
        let ports: HashSet<u16> = catalog.iter().map(|e| e.default_port).collect();
        assert_eq!(ports.len(), catalog.len(), "default ports must be unique");
    }

    #[test]
    fn test_catalog_knows_both_classes() {
        assert_eq!(entry("qhy-focuser").unwrap().class, ServerClass::Alpaca);
        assert_eq!(entry("rp").unwrap().class, ServerClass::Core);
        assert_eq!(entry("qhy-focuser").unwrap().default_port, 11113);
    }

    #[test]
    fn test_unit_name_round_trips() {
        let entry = entry_for_unit("rusty-photon-qhy-focuser.service").unwrap();
        assert_eq!(entry.name, "qhy-focuser");
        assert_eq!(entry.unit_name(), "rusty-photon-qhy-focuser");
        assert_eq!(entry.config_file(), "qhy-focuser.json");
        assert!(entry_for_unit("ssh.service").is_none());
    }
}
