//! Reading the config directory (docs/services/doctor.md §Config parsing).
//!
//! For each catalog service, doctor reads `<svc>.json` the way the service
//! itself would: the file must be valid JSON, and the top-level `server`
//! block must parse under the catalog-declared shared shape, including
//! `deny_unknown_fields`. Everything else in every file stays opaque
//! `serde_json::Value` except the known cross-reference blocks, which are
//! read **leniently** (doctor knows only the fields it joins across, never
//! the whole shape).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusty_photon_server_config::doctor_toml::ServerClass;
use rusty_photon_server_config::{AlpacaServerConfig, ServerConfig};
use serde::Deserialize;
use serde_json::Value;

use crate::catalog::CatalogEntry;

/// Why a config file's contents are unavailable — two different operator
/// problems, diagnosed under two different check names.
#[derive(Debug)]
pub enum ReadError {
    /// The file exists but could not be read (permissions, I/O).
    Unreadable(String),
    /// The file was read but is not valid JSON.
    InvalidJson(String),
}

/// The `server` block, parsed under the catalog-declared shape.
#[derive(Debug)]
pub enum ServerBlock {
    /// No config file — the service will self-create defaults on first run.
    FileAbsent,
    /// No parseable `server` block to judge: the file has no `server` key
    /// (the service applies its defaults), or the file itself is unreadable
    /// or invalid JSON — the read-level checks own that diagnosis, and the
    /// effective port falls back to the catalog default either way.
    BlockAbsent,
    /// Parsed. `discovery_port` is `None` for core services.
    Parsed {
        server: ServerConfig,
        discovery_port: Option<u16>,
    },
    /// The block would make the service refuse to start.
    Invalid(String),
}

/// One catalog service's on-disk state.
#[derive(Debug)]
pub struct ServiceScan {
    pub entry: &'static CatalogEntry,
    pub config_path: PathBuf,
    /// `None` — file absent; `Err` — unreadable or not valid JSON.
    pub raw: Option<Result<Value, ReadError>>,
    pub server: ServerBlock,
}

impl ServiceScan {
    /// Whether this service takes part in diagnosis: its unit is installed
    /// or its config file exists.
    pub fn config_present(&self) -> bool {
        self.raw.is_some()
    }

    /// The parsed config `Value`, when the file exists and is valid JSON.
    pub fn value(&self) -> Option<&Value> {
        self.raw.as_ref().and_then(|r| r.as_ref().ok())
    }

    /// The port this service will actually use: the configured
    /// `server.port`, else the catalog default (also while the server block
    /// is unparseable — the collision picture should not go blind because
    /// one file has a typo).
    pub fn effective_port(&self) -> u16 {
        match &self.server {
            ServerBlock::Parsed { server, .. } => server.port,
            _ => self.entry.default_port,
        }
    }

    /// The enabled discovery port, when the config sets one.
    pub fn discovery_port(&self) -> Option<u16> {
        match &self.server {
            ServerBlock::Parsed { discovery_port, .. } => *discovery_port,
            _ => None,
        }
    }

    /// The parsed TLS/auth view of the server block, when parsed.
    pub fn server(&self) -> Option<&ServerConfig> {
        match &self.server {
            ServerBlock::Parsed { server, .. } => Some(server),
            _ => None,
        }
    }
}

/// Scan one service's config file.
pub fn scan_service(config_dir: &Path, entry: &'static CatalogEntry) -> ServiceScan {
    let config_path = config_dir.join(entry.config_file());
    let raw = match std::fs::read_to_string(&config_path) {
        Ok(content) => Some(
            serde_json::from_str::<Value>(&content)
                .map_err(|e| ReadError::InvalidJson(e.to_string())),
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => Some(Err(ReadError::Unreadable(e.to_string()))),
    };
    let server = match &raw {
        None => ServerBlock::FileAbsent,
        Some(Err(_)) => ServerBlock::BlockAbsent,
        Some(Ok(value)) => match value.get("server") {
            None => ServerBlock::BlockAbsent,
            Some(block) => parse_server_block(block, entry.class),
        },
    };
    ServiceScan {
        entry,
        config_path,
        raw,
        server,
    }
}

fn parse_server_block(block: &Value, class: ServerClass) -> ServerBlock {
    match class {
        ServerClass::Alpaca => match AlpacaServerConfig::deserialize(block) {
            Ok(config) => ServerBlock::Parsed {
                discovery_port: config.discovery_port,
                server: config.core(),
            },
            Err(e) => ServerBlock::Invalid(e.to_string()),
        },
        ServerClass::Core => match ServerConfig::deserialize(block) {
            Ok(server) => ServerBlock::Parsed {
                server,
                discovery_port: None,
            },
            Err(e) => ServerBlock::Invalid(e.to_string()),
        },
    }
}

/// The `.json` files in the config dir that belong to no catalog service —
/// candidates for the unknown-config warning. Known non-service files are
/// exempt: `acme.json` (rp-tls ACME state) lives beside the configs.
pub fn unknown_config_files(config_dir: &Path, known: &[String]) -> Vec<String> {
    const NON_SERVICE_FILES: &[&str] = &["acme.json"];
    let Ok(entries) = std::fs::read_dir(config_dir) else {
        return Vec::new();
    };
    let mut unknown: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| {
            name.ends_with(".json")
                && !NON_SERVICE_FILES.contains(&name.as_str())
                && !known.contains(name)
        })
        .collect();
    unknown.sort();
    unknown
}

// ---- Lenient views of the known cross-reference blocks ----
//
// No `deny_unknown_fields` anywhere below: these are *doctor's* partial
// views of other services' shapes, and must keep working as those shapes
// grow fields doctor does not join across.

/// ui-htmx: one `drivers` map entry.
#[derive(Debug, Deserialize, Default)]
pub struct UiDriverView {
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub sentinel_service: Option<String>,
}

/// ui-htmx: the blocks doctor joins across.
#[derive(Debug, Deserialize, Default)]
pub struct UiHtmxView {
    #[serde(default)]
    pub drivers: BTreeMap<String, UiDriverView>,
    #[serde(default)]
    pub sentinel: Option<Value>,
}

/// sentinel: one `services` map entry.
#[derive(Debug, Deserialize, Default)]
pub struct SentinelServiceView {
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub restart_command: Option<String>,
}

/// sentinel: one watchdog operation family.
#[derive(Debug, Deserialize, Default)]
pub struct WatchdogOperationView {
    #[serde(default)]
    pub service: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct WatchdogView {
    #[serde(default)]
    pub operations: BTreeMap<String, WatchdogOperationView>,
}

/// sentinel: the blocks doctor joins across.
#[derive(Debug, Deserialize, Default)]
pub struct SentinelView {
    #[serde(default)]
    pub services: BTreeMap<String, SentinelServiceView>,
    #[serde(default)]
    pub operation_watchdog: Option<WatchdogView>,
}

/// rp: the session block field doctor checks.
#[derive(Debug, Deserialize, Default)]
pub struct RpSessionView {
    #[serde(default)]
    pub data_directory: Option<String>,
}

/// rp: the blocks doctor reads. `equipment` stays a `Value` — device usage
/// is opaque; only each entry's `alpaca_url` is extracted.
#[derive(Debug, Deserialize, Default)]
pub struct RpView {
    #[serde(default)]
    pub equipment: Option<Value>,
    #[serde(default)]
    pub session: Option<RpSessionView>,
}

impl RpView {
    /// Every `alpaca_url` in the equipment block, wherever the entry lives
    /// (`equipment.<kind>[].alpaca_url`).
    pub fn alpaca_urls(&self) -> Vec<String> {
        let mut urls = Vec::new();
        if let Some(Value::Object(kinds)) = &self.equipment {
            for entries in kinds.values() {
                if let Value::Array(entries) = entries {
                    for entry in entries {
                        if let Some(url) = entry.get("alpaca_url").and_then(Value::as_str) {
                            urls.push(url.to_string());
                        }
                    }
                }
            }
        }
        urls
    }
}

/// Parse a lenient view out of a scanned config, distinguishing "view not
/// applicable" (file absent / invalid JSON, `None`) from "the known block
/// itself does not parse" (`Some(Err)`).
pub fn view<T: for<'de> Deserialize<'de>>(scan: &ServiceScan) -> Option<Result<T, String>> {
    let value = scan.value()?;
    Some(T::deserialize(value.clone()).map_err(|e| e.to_string()))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::catalog;

    fn write(dir: &Path, name: &str, content: &str) {
        std::fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn test_scan_distinguishes_absent_invalid_and_parsed() {
        let dir = tempfile::tempdir().unwrap();
        let entry = catalog::entry("qhy-focuser").unwrap();

        let scan = scan_service(dir.path(), entry);
        assert!(matches!(scan.server, ServerBlock::FileAbsent));
        assert!(!scan.config_present());
        assert_eq!(scan.effective_port(), 11113);

        write(dir.path(), "qhy-focuser.json", "{ not json");
        let scan = scan_service(dir.path(), entry);
        assert!(scan.raw.as_ref().unwrap().is_err());
        assert_eq!(scan.effective_port(), 11113);

        write(
            dir.path(),
            "qhy-focuser.json",
            r#"{ "server": { "port": 4711, "discovery_port": 32227 } }"#,
        );
        let scan = scan_service(dir.path(), entry);
        assert_eq!(scan.effective_port(), 4711);
        assert_eq!(scan.discovery_port(), Some(32227));
    }

    #[cfg(unix)]
    #[test]
    fn test_unreadable_config_is_distinguished_from_invalid_json() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "qhy-focuser.json", "{}");
        let path = dir.path().join("qhy-focuser.json");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let scan = scan_service(dir.path(), catalog::entry("qhy-focuser").unwrap());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        match scan.raw {
            Some(Err(ReadError::Unreadable(_))) => {}
            Some(Ok(_)) => {} // running privileged — mode 000 still reads
            other => unreachable!("expected Unreadable, got {other:?}"),
        }
    }

    #[test]
    fn test_server_block_rejects_the_wrong_class_shape() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "ui-htmx.json",
            r#"{ "server": { "port": 11120, "discovery_port": 32227 } }"#,
        );
        let scan = scan_service(dir.path(), catalog::entry("ui-htmx").unwrap());
        let ServerBlock::Invalid(err) = &scan.server else {
            unreachable!("core shape must reject discovery_port: {:?}", scan.server);
        };
        assert!(err.contains("discovery_port"), "{err}");
    }

    #[test]
    fn test_unknown_config_files_exempt_known_non_service_files() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "qhy-focuser.json", "{}");
        write(dir.path(), "qhy-focusser.json", "{}");
        write(dir.path(), "acme.json", "{}");
        write(dir.path(), "notes.txt", "not json");
        let unknown = unknown_config_files(dir.path(), &["qhy-focuser.json".to_string()]);
        assert_eq!(unknown, vec!["qhy-focusser.json".to_string()]);
    }

    #[test]
    fn test_rp_view_collects_alpaca_urls_across_kinds() {
        let view: RpView = serde_json::from_str(
            r#"{ "equipment": {
                   "cameras": [ { "alpaca_url": "http://localhost:11121", "role": "guide" } ],
                   "mounts": [ { "alpaca_url": "http://localhost:11117" }, {} ] },
                 "session": { "data_directory": "/var/lib/x" },
                 "unknown_future_block": 1 }"#,
        )
        .unwrap();
        assert_eq!(
            view.alpaca_urls(),
            vec!["http://localhost:11121", "http://localhost:11117"]
        );
        assert_eq!(view.session.unwrap().data_directory.unwrap(), "/var/lib/x");
    }

    #[test]
    fn test_views_parse_leniently_but_report_shape_errors() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "sentinel.json",
            r#"{ "server": { "port": 11114 }, "services": ["not", "a", "map"] }"#,
        );
        let scan = scan_service(dir.path(), catalog::entry("sentinel").unwrap());
        let result: Result<SentinelView, String> = view(&scan).unwrap();
        result.unwrap_err();

        write(
            dir.path(),
            "sentinel.json",
            r#"{ "server": { "port": 11114 },
                 "services": { "cam": { "base_url": "http://x/api/v1", "future": 1 } },
                 "dashboard_extras": true }"#,
        );
        let scan = scan_service(dir.path(), catalog::entry("sentinel").unwrap());
        let sentinel: SentinelView = view(&scan).unwrap().unwrap();
        assert_eq!(
            sentinel.services["cam"].base_url.as_deref(),
            Some("http://x/api/v1")
        );
    }
}
