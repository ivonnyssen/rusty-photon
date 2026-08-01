//! Configuration types and JSON loader.
//!
//! Top-level [`Config`] is built by [`load_config`] from a JSON file.
//! Each domain-specific block lives in a sibling module:
//! [`session`], [`site`], [`equipment`] (plus the per-device-type
//! configs [`camera`], [`focuser`], [`mount`], [`filter_wheel`],
//! [`cover_calibrator`], the [`optical_train`] light-path lists, and
//! the mount-scoped [`guiding`] service block), [`imaging`],
//! [`plate_solver`], [`server`].
//! The submodules' public types are re-exported here so existing
//! `crate::config::CameraConfig` callsites keep working unchanged.

pub mod camera;
pub mod centering;
pub mod cooling;
pub mod cover_calibrator;
pub mod dome;
pub mod equipment;
pub mod filter_wheel;
pub mod focuser;
pub mod guiding;
pub mod imaging;
pub mod mount;
pub mod naming_template;
pub mod observing_conditions;
pub mod optical_train;
pub mod plate_solver;
pub mod rotator;
pub mod safety;
pub mod safety_monitor;
pub mod server;
pub mod session;
pub mod site;
pub mod switch;
pub mod target_store;

pub use camera::CameraConfig;
pub use centering::CenteringConfig;
pub use cooling::CoolingConfig;
pub use cover_calibrator::CoverCalibratorConfig;
pub use dome::DomeConfig;
pub use equipment::EquipmentConfig;
pub use filter_wheel::FilterWheelConfig;
pub use focuser::FocuserConfig;
pub use guiding::{FocusWatchConfig, GuiderDefaults, GuidingConfig};
pub use imaging::ImagingConfig;
pub use mount::MountConfig;
pub use observing_conditions::ObservingConditionsConfig;
pub use optical_train::{
    FocalLengthMm, OpticalTrainConfig, PositionAngleDegrees, TrainAutoFocusConfig, TrainPurpose,
};
pub use plate_solver::PlateSolverConfig;
pub use rotator::RotatorConfig;
pub use safety::SafetyConfig;
pub use safety_monitor::SafetyMonitorConfig;
pub use server::{AdvertisedUrl, ServerConfig};
pub use session::SessionConfig;
pub use site::SiteConfig;
pub use switch::SwitchConfig;
pub use target_store::{TargetStoreConfig, TargetStoreConfigWire};

use std::path::Path;

use rusty_photon_config::actions::FieldError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Result, RpError};

/// `deny_unknown_fields` so typoed or removed top-level keys fail loudly at
/// load instead of being silently ignored.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub session: SessionConfig,
    pub equipment: EquipmentConfig,
    /// Observer site (lat/lon). Required for ephemeris features
    /// (`compute_alt_az`, `get_target_status`, etc.); optional otherwise.
    /// When `Some` and a mount is configured, `rp` validates the
    /// configured lat/lon against the mount's `SiteLatitude` /
    /// `SiteLongitude` on connect — see `docs/services/rp.md`
    /// §"Site Validation Against the ASCOM Mount".
    #[serde(default)]
    pub site: Option<SiteConfig>,
    #[serde(default)]
    pub plugins: Vec<Value>,
    /// Target-store settings (`db_path`, `default_goals`,
    /// `default_scheduling`). Targets themselves live in the redb store
    /// (added via `add_target`), not in config — the legacy `targets[]`
    /// planner array was retired. A stray `targets` key or a leftover
    /// array shape here fails loudly at load (`deny_unknown_fields` +
    /// the typed field).
    #[serde(default)]
    pub target_store: TargetStoreConfigWire,
    #[serde(default)]
    pub planner: Value,
    /// Safety-enforcement knobs (rp.md § Safety); the monitors
    /// themselves live under `equipment.safety_monitors`.
    #[serde(default)]
    pub safety: SafetyConfig,
    #[serde(default)]
    pub imaging: ImagingConfig,
    /// Per-rig estimates that size the advisory `center_on_target`
    /// deadline carried on `centering_started` (§2.5). Always present;
    /// an omitted block uses [`CenteringConfig`]'s defaults.
    #[serde(default)]
    pub centering: CenteringConfig,
    /// Camera-cooling controller tuning (rp.md § Camera Cooling).
    /// Always present; an omitted block uses [`CoolingConfig`]'s
    /// defaults. The per-camera setpoint ladders live under
    /// `equipment.cameras[].cooler_targets_c`.
    #[serde(default)]
    pub cooling: CoolingConfig,
    /// Optional plate-solver service. When `None`, the `plate_solve`
    /// MCP tool returns `plate solver not configured`. Mirrors the
    /// `Option<MountConfig>` pattern — the service is optional
    /// infrastructure, not part of the equipment surface.
    #[serde(default)]
    pub plate_solver: Option<PlateSolverConfig>,
    #[serde(default = "server::default_server")]
    pub server: ServerConfig,
    /// PEM CA certificate `rp` trusts for every outbound HTTPS connection
    /// it makes as a client: Alpaca devices (`equipment.*[].alpaca_url`),
    /// the plate-solver service, and the guider service. An observatory
    /// runs one CA (rusty_photon_tls), so this is a single rp-level
    /// setting rather than per-target — matching the `ca_cert` field
    /// doctor already wires into sentinel, session-runner, and
    /// calibrator-flats (`services/doctor/src/provision/mod.rs`
    /// `CLIENT_WIRING_SERVICES`). `Some` becomes the client's **only**
    /// trusted root (`tls_certs_only`, ADR-002) — it replaces, not adds
    /// to, the platform trust store, so a public-CA `https://` target
    /// becomes unreachable alongside the observatory CA. `None` (the
    /// default) uses the platform trust store, so an https target signed
    /// by the observatory's self-signed CA fails certificate verification.
    #[serde(default)]
    pub ca_cert: Option<String>,
}

impl Config {
    /// [`Config::ca_cert`] as a `Path`, for `rusty_photon_tls::client`.
    pub fn ca_cert_path(&self) -> Option<&Path> {
        self.ca_cert.as_deref().map(Path::new)
    }
}

/// Minimal runnable scaffold `rp` writes on first start when no config
/// exists at the platform default path: no equipment, default server,
/// session data under a platform-dependent directory — the packaged unit's
/// `StateDirectory` (`/var/lib/rusty-photon/rp/`) on Linux,
/// `~/Library/Application Support/rusty-photon/rp/` on macOS,
/// `%PROGRAMDATA%\rusty-photon\rp\` on Windows (ADR-015). Must stay
/// deserializable into [`Config`] — the packaged first-start contract
/// depends on it.
pub fn default_scaffold() -> serde_json::Value {
    serde_json::json!({
        "session": { "data_directory": default_data_directory() },
        "equipment": {},
        "server": { "port": 11115, "bind_address": "0.0.0.0" }
    })
}

/// The Linux state path, provisioned and owned by the packaged unit's
/// systemd `StateDirectory=`; also the macOS last-resort fallback.
#[cfg(any(not(windows), test))]
const LINUX_STATE_DATA_DIR: &str = "/var/lib/rusty-photon/rp/data";

/// The scaffold's platform-dependent `session.data_directory` default.
///
/// The startup target-store open creates this directory (rp.md § Target
/// Store), so the default has to be writable by whatever account the
/// packaged service runs as. Linux gets that from systemd `StateDirectory=`
/// and Windows from `LocalSystem`'s access to `%PROGRAMDATA%`; macOS has no
/// equivalent — `brew services` runs as the invoking user, who cannot write
/// `/var/lib` — so macOS puts session data beside the config, mirroring the
/// Windows layout under its own platform root.
#[cfg(not(any(windows, target_os = "macos")))]
fn default_data_directory() -> String {
    LINUX_STATE_DATA_DIR.to_string()
}
#[cfg(target_os = "macos")]
fn default_data_directory() -> String {
    macos_data_directory(rusty_photon_config::default_config_dir().ok())
}
#[cfg(windows)]
fn default_data_directory() -> String {
    program_data_root(std::env::var_os("ProgramData"))
        .join("rusty-photon")
        .join("rp")
        .join("data")
        .to_string_lossy()
        .into_owned()
}

/// Pure resolution of the macOS `session.data_directory` default from the
/// resolved platform config directory (`~/Library/Application Support/
/// rusty-photon`, what `rusty-photon-config` puts `rp.json` in): session
/// data lands in `rp/data` beneath it. Falls back to
/// [`LINUX_STATE_DATA_DIR`] when no home directory resolves at all — a
/// machine with no home is no better served by either path, and that one
/// is at least documented. Parameterized over the resolved directory, and
/// compiled on macOS and in test builds on every platform, so the logic is
/// unit-testable on non-macOS hosts.
#[cfg(any(target_os = "macos", test))]
fn macos_data_directory(config_dir: Option<std::path::PathBuf>) -> String {
    match config_dir {
        Some(dir) => dir.join("rp").join("data").to_string_lossy().into_owned(),
        None => LINUX_STATE_DATA_DIR.to_string(),
    }
}

/// Pure resolution of the Windows `ProgramData` root from the value of the
/// `ProgramData` environment variable: the value verbatim when present and
/// non-empty, else the fixed `C:\ProgramData` fallback. A private copy of the
/// same rule `rusty-photon-config` applies to the config path (each crate
/// keeps its own — see the W2 note in `docs/plans/windows-packaging.md`);
/// compiled on Windows and in test builds on every platform, so the logic
/// is unit-testable on non-Windows hosts.
#[cfg(any(windows, test))]
fn program_data_root(program_data: Option<std::ffi::OsString>) -> std::path::PathBuf {
    match program_data {
        Some(v) if !v.is_empty() => std::path::PathBuf::from(v),
        _ => std::path::PathBuf::from(r"C:\ProgramData"),
    }
}

/// Domain validation shared by startup ([`load_config`]) and the REST
/// `PUT /api/config` endpoint (via [`crate::config_actions::RpConfigDriver`]).
/// Empty result means valid. Paths are dotted with array indices
/// (`equipment.cameras.0.focal_length_mm`) so a UI can render each error
/// next to its field; messages name the device id where one exists.
pub fn validate_config(config: &Config) -> Vec<FieldError> {
    let mut errors = Vec::new();
    if let Some(site) = config.site.as_ref() {
        errors.extend(site.field_errors());
    }
    if let Some(pattern) = config.session.file_naming_pattern.as_deref() {
        if let Err(msg) = naming_template::validate_pattern(pattern) {
            errors.push(FieldError {
                path: "session.file_naming_pattern".to_string(),
                msg,
            });
        }
    }
    if let Some(pattern) = config.session.directory_pattern.as_deref() {
        if let Err(msg) = naming_template::validate_directory_pattern(pattern) {
            errors.push(FieldError {
                path: "session.directory_pattern".to_string(),
                msg,
            });
        }
    }
    for (index, cam) in config.equipment.cameras.iter().enumerate() {
        errors.extend(cam.field_errors(index));
    }
    // The optical-train graph rules (roster existence, terminal camera,
    // order consistency, the one-guiding-train rule) live with the
    // derived model so validation and derivation cannot drift apart.
    if let Err(train_errors) =
        crate::equipment::trains::TrainModel::try_from_equipment(&config.equipment)
    {
        errors.extend(train_errors);
    }
    errors.extend(plugin_registration_errors(&config.plugins));
    errors
}

/// A plugin registration stays an opaque `Value` — it is a plugin-author
/// surface, so unknown keys are legal there in a way they are nowhere else
/// in this config. The registrations rp itself *dials* are the exception,
/// because on those rp reads two fields: the callback URL it POSTs to —
/// the orchestrator's `invoke_url` (a session start) or an event plugin's
/// `webhook_url` (an emitted event) — and `auth`, the credential it
/// presents there (rp.md § Orchestrator Registration, § Delivery:
/// Webhooks). Both are permanent configuration faults when malformed — a
/// half-written credential would be read as "no credential" and 401 every
/// delivery; a bad URL would fail every attempt — so both fail at load
/// rather than at first use, which means `load_config`,
/// `PUT /api/config`, and `rp doctor` all reject them identically instead
/// of leaving the first session of the night to discover it.
///
/// Scoped to those two types exactly because the surface is otherwise
/// opaque: rp dials no other registration, so a tool-provider plugin
/// carrying its own differently-shaped `auth` key (a bearer token, say)
/// is that author's business and must not fail rp's config load. The same
/// scope decides which registration doctor offers to wire a credential
/// into (`RpView::plugin_targets`).
fn plugin_registration_errors(plugins: &[Value]) -> Vec<FieldError> {
    plugins
        .iter()
        .enumerate()
        .filter_map(|(index, plugin)| Some((index, plugin, dialed_url_field(plugin)?)))
        .flat_map(|(index, plugin, url_field)| {
            let mut errors = Vec::new();
            if let Some(auth) = plugin.get("auth").filter(|v| !v.is_null()) {
                if let Err(e) =
                    serde_json::from_value::<rp_auth::config::ClientAuthConfig>(auth.clone())
                {
                    errors.push(FieldError {
                        path: format!("plugins.{index}.auth"),
                        msg: e.to_string(),
                    });
                }
            }
            if let Some(url) = plugin.get(url_field).filter(|v| !v.is_null()) {
                if let Err(msg) = validate_callback_url(url) {
                    errors.push(FieldError {
                        path: format!("plugins.{index}.{url_field}"),
                        msg,
                    });
                }
            }
            errors
        })
        .collect()
}

/// The registration field naming the endpoint rp POSTs a session start
/// to (rp.md § Orchestrator Registration). Read by
/// [`crate::session::SessionManager`]'s registration lookup and validated
/// by [`plugin_registration_errors`] through this one name, so the two
/// cannot drift.
pub const ORCHESTRATOR_URL_FIELD: &str = "invoke_url";

/// The registration field naming the endpoint rp POSTs each subscribed
/// event to (rp.md § Delivery: Webhooks). Read by
/// [`crate::events::EventBus`]'s registration lookup and validated by
/// [`plugin_registration_errors`] through this one name.
pub const EVENT_URL_FIELD: &str = "webhook_url";

/// The callback field rp POSTs to, for the two plugin types rp dials;
/// `None` for every other type, whose keys rp never reads. The single
/// place that mapping is written. doctor's `RpView::plugin_targets` has
/// to restate it — it reads rp's config as opaque JSON from another
/// crate — so the two are kept in step by their tests, not by the
/// compiler.
fn dialed_url_field(plugin: &Value) -> Option<&'static str> {
    if is_orchestrator(plugin) {
        Some(ORCHESTRATOR_URL_FIELD)
    } else if is_event_plugin(plugin) {
        Some(EVENT_URL_FIELD)
    } else {
        None
    }
}

/// A callback URL must be an `http://` or `https://` URL rp can actually
/// POST to. Rejected at load for the same reason `server.advertised_url`
/// is: a bad scheme or a non-URL is a permanent configuration fault, and
/// left to first use it would only surface as a failed delivery in the
/// middle of the night. Mirrors [`server::AdvertisedUrl`]'s rule rather
/// than inventing a second one, plus one rule of its own — see the
/// userinfo branch.
fn validate_callback_url(value: &Value) -> std::result::Result<(), String> {
    let Some(url) = value.as_str() else {
        return Err(format!("must be a string, got {value}"));
    };
    // Every branch that echoes the URL back redacts it first: a
    // `FieldError` is rendered by the UI and by `rp doctor`, and these
    // two run *before* the userinfo check below — on inputs that may not
    // even parse — so a malformed credential-bearing URL would otherwise
    // leak its password on the way to being rejected.
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(format!(
            "must be an http:// or https:// URL, got {:?}",
            redact_userinfo(url)
        ));
    }
    // The scheme is already known to be http(s), for which the URL
    // grammar requires a host — so a successful parse here is a URL rp
    // can post to, and the host-less case arrives as a parse error
    // ("empty host") rather than needing a branch of its own.
    let parsed = reqwest::Url::parse(url)
        .map_err(|e| format!("is not a valid URL ({e}): {:?}", redact_userinfo(url)))?;
    // Embedded credentials are rejected rather than honored: rp logs this
    // URL on every delivery attempt, so a password in it is a password in
    // the night's logs, and the sibling `auth` block — which rp applies
    // per-request, marked sensitive — would silently win over it anyway.
    // This message omits the URL entirely rather than redacting it: there
    // is nothing left to point at that the field path does not say.
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(
            "must not embed credentials in the URL; put them in the sibling `auth` block"
                .to_string(),
        );
    }
    Ok(())
}

/// `url` with any embedded userinfo replaced by `***`, for the messages
/// that echo a rejected URL back to an operator.
///
/// Deliberately textual rather than going through [`reqwest::Url`]: the
/// callers run on input that has already failed a scheme check or failed
/// to parse at all, so there is no parsed URL to read a username off.
/// Scans only the authority — up to the first `/`, `?`, or `#` — so a
/// later `@` in a path or query is left alone.
fn redact_userinfo(url: &str) -> std::borrow::Cow<'_, str> {
    let authority_start = url.find("://").map_or(0, |i| i + 3);
    let authority_end = url[authority_start..]
        .find(['/', '?', '#'])
        .map_or(url.len(), |i| authority_start + i);
    // The last `@` in the authority is the delimiter; an earlier one
    // would be inside the userinfo itself.
    match url[authority_start..authority_end].rfind('@') {
        Some(at) => std::borrow::Cow::Owned(format!(
            "{}***@{}",
            &url[..authority_start],
            &url[authority_start + at + 1..]
        )),
        None => std::borrow::Cow::Borrowed(url),
    }
}

/// Whether a plugin registration is the orchestrator kind — the single
/// place that rule is written, shared by [`plugin_registration_errors`]
/// and [`crate::session::SessionManager`]'s registration lookup.
pub fn is_orchestrator(plugin: &Value) -> bool {
    plugin.get("type").and_then(Value::as_str) == Some("orchestrator")
}

/// Whether a plugin registration is the event-webhook kind — the single
/// place that rule is written, shared by [`plugin_registration_errors`]
/// and [`crate::events::EventBus`]'s registration lookup.
pub fn is_event_plugin(plugin: &Value) -> bool {
    plugin.get("type").and_then(Value::as_str) == Some("event")
}

pub fn load_config(path: &Path) -> Result<Config> {
    let contents = std::fs::read_to_string(path).map_err(|e| {
        RpError::Config(format!(
            "failed to read config file '{}': {}",
            path.display(),
            e
        ))
    })?;
    let config: Config = serde_json::from_str(&contents).map_err(|e| {
        RpError::Config(format!(
            "failed to parse config file '{}': {}",
            path.display(),
            e
        ))
    })?;
    // Same field validation as `PUT /api/config`; startup keeps its
    // pre-REST behaviour of aborting on the first offending field.
    if let Some(err) = validate_config(&config).into_iter().next() {
        return Err(RpError::Config(format!("{} {}", err.path, err.msg)));
    }
    Ok(config)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
pub(crate) mod test_support {
    pub const MINIMAL_CONFIG_JSON: &str = r#"{
        "session": {"data_directory": "/tmp/rp-test"},
        "equipment": {}
    }"#;
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::test_support::MINIMAL_CONFIG_JSON;
    use super::*;

    #[test]
    fn default_scaffold_deserializes_into_config() {
        let config: Config = serde_json::from_value(default_scaffold()).unwrap();
        #[cfg(not(any(windows, target_os = "macos")))]
        assert_eq!(config.session.data_directory, LINUX_STATE_DATA_DIR);
        // Under the config root when a home resolves, else the fallback —
        // both end the same way, so this holds without a home on the runner.
        #[cfg(target_os = "macos")]
        assert!(
            config
                .session
                .data_directory
                .ends_with("/rusty-photon/rp/data"),
            "{}",
            config.session.data_directory
        );
        #[cfg(windows)]
        assert!(
            config
                .session
                .data_directory
                .ends_with(r"\rusty-photon\rp\data"),
            "{}",
            config.session.data_directory
        );
        assert!(config.equipment.cameras.is_empty());
        assert!(config.site.is_none());
        assert_eq!(config.server.port, 11115);
    }

    #[test]
    fn macos_data_directory_sits_under_the_config_root() {
        let root = "/Users/astro/Library/Application Support/rusty-photon";
        let dir = macos_data_directory(Some(std::path::PathBuf::from(root)));
        // Compared as `Path`s, not strings: `PathBuf::join` emits the *host's*
        // separator, and this macOS-only path is also built on the Windows and
        // Linux hosts that run the test. `Path` equality is component-wise, so
        // this still pins the tail exactly.
        let relative = std::path::Path::new(&dir)
            .strip_prefix(root)
            .unwrap_or_else(|_| panic!("{dir} is not under {root}"));
        assert_eq!(relative, std::path::Path::new("rp/data"));
    }

    #[test]
    fn macos_data_directory_falls_back_without_a_home() {
        assert_eq!(macos_data_directory(None), LINUX_STATE_DATA_DIR);
    }

    #[test]
    fn program_data_root_uses_env_value_verbatim() {
        let root = program_data_root(Some(std::ffi::OsString::from(r"D:\CustomData")));
        assert_eq!(root, std::path::PathBuf::from(r"D:\CustomData"));
    }

    #[test]
    fn program_data_root_falls_back_when_env_absent() {
        assert_eq!(
            program_data_root(None),
            std::path::PathBuf::from(r"C:\ProgramData")
        );
    }

    #[test]
    fn program_data_root_falls_back_when_env_empty() {
        assert_eq!(
            program_data_root(Some(std::ffi::OsString::new())),
            std::path::PathBuf::from(r"C:\ProgramData")
        );
    }

    #[test]
    fn load_config_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, MINIMAL_CONFIG_JSON).unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(config.session.data_directory, "/tmp/rp-test");
        assert_eq!(config.server.port, 11115);
        assert_eq!(config.server.bind_address.to_string(), "0.0.0.0");
        assert_eq!(config.imaging.cache_max_mib, 1024);
        assert_eq!(config.imaging.cache_max_images, 8);
    }

    #[test]
    fn ca_cert_omitted_defaults_to_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, MINIMAL_CONFIG_JSON).unwrap();

        let config = load_config(&path).unwrap();
        assert!(config.ca_cert.is_none());
        assert!(config.ca_cert_path().is_none());
    }

    #[test]
    fn ca_cert_path_reflects_the_configured_string() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "ca_cert": "/etc/rusty-photon/pki/ca.pem"
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(
            config.ca_cert_path(),
            Some(Path::new("/etc/rusty-photon/pki/ca.pem"))
        );
    }

    #[test]
    fn load_config_missing_file() {
        let err = load_config(Path::new("/nonexistent/rp/config.json")).unwrap_err();
        assert!(err.to_string().contains("failed to read config file"));
    }

    #[test]
    fn load_config_rejects_unknown_top_level_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "server": { "port": 0 },
                "workflows": []
            }"#,
        )
        .unwrap();

        let err = load_config(&path).unwrap_err().to_string();
        assert!(err.contains("workflows"), "{err}");
    }

    #[test]
    fn load_config_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "not valid json").unwrap();

        let err = load_config(&path).unwrap_err();
        assert!(err.to_string().contains("failed to parse config file"));
    }

    #[test]
    fn validate_config_flags_site_camera_and_train_fields_with_dotted_paths() {
        let mut config: Config = serde_json::from_value(default_scaffold()).unwrap();
        config.site = Some(crate::config::SiteConfig {
            latitude_degrees: 91.0,
            longitude_degrees: 181.0,
        });
        config.equipment.cameras = vec![
            serde_json::from_value(serde_json::json!({
                "id": "bad-cam",
                "alpaca_url": "http://localhost:11120",
                "cooler_targets_c": [-12]
            }))
            .unwrap(),
            serde_json::from_value(serde_json::json!({
                "id": "good-cam",
                "alpaca_url": "http://localhost:11121"
            }))
            .unwrap(),
        ];
        config.equipment.optical_trains = vec![serde_json::from_value(serde_json::json!({
            "id": "main",
            "devices": ["ghost-focuser", "good-cam"]
        }))
        .unwrap()];

        let errors = validate_config(&config);
        let paths: Vec<&str> = errors.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "site.latitude_degrees",
                "site.longitude_degrees",
                "equipment.cameras.0.cooler_targets_c",
                "equipment.optical_trains.0.devices.0",
            ]
        );
        assert!(
            errors[2].msg.contains("bad-cam"),
            "camera errors name the device id for humans: {:?}",
            errors[2]
        );
        assert!(
            errors[3].msg.contains("ghost-focuser"),
            "train errors name the offending device id: {:?}",
            errors[3]
        );
    }

    #[test]
    fn validate_config_accepts_scaffold() {
        let config: Config = serde_json::from_value(default_scaffold()).unwrap();
        assert_eq!(validate_config(&config), vec![]);
    }

    /// Serialize → deserialize → serialize must be a fixed point: the REST
    /// `PUT /api/config` path re-parses the serialized value it persists
    /// (`rusty_photon_config::actions::config_apply`), so any asymmetric
    /// field would corrupt the persisted config.
    fn assert_value_round_trips(config: &Config) -> serde_json::Value {
        let value = serde_json::to_value(config).unwrap();
        let back: Config = serde_json::from_value(value.clone()).unwrap();
        let again = serde_json::to_value(&back).unwrap();
        assert_eq!(again, value, "Config JSON round-trip must be stable");
        value
    }

    #[test]
    fn config_json_round_trips_default_scaffold() {
        let config: Config = serde_json::from_value(default_scaffold()).unwrap();
        assert_value_round_trips(&config);
    }

    #[test]
    fn config_json_round_trips_fully_populated_sample() {
        // Every block populated, including all humantime-serde Duration
        // fields (which serialize as humantime strings) and both secret
        // shapes (server auth hash + per-device client auth password).
        let sample = serde_json::json!({
            "session": {
                "data_directory": "/data/lights",
                "session_state_file": "/data/session_state.json",
                "file_naming_pattern": "{target}_{filter}"
            },
            "site": { "latitude_degrees": 47.6062, "longitude_degrees": -122.3321 },
            "equipment": {
                "cameras": [{
                    "id": "main-cam",
                    "name": "Main",
                    "alpaca_url": "https://localhost:11120",
                    "device_type": "camera",
                    "device_number": 0,
                    "cooler_targets_c": [-10, 5],
                    "gain": 100,
                    "offset": 50,
                    "readout_time_estimate": "8s",
                    "auth": { "username": "observatory", "password": "secret" }
                }],
                "optical_trains": [{
                    "id": "main",
                    "purpose": "imaging",
                    "focal_length_mm": 1000.0,
                    "devices": ["main-focuser", "main-fw", "main-cam"]
                }],
                "mount": {
                    "alpaca_url": "http://localhost:11122",
                    "device_number": 0,
                    "settle_after_slew": "3s",
                    "slew_rate_arcsec_per_sec": 7200.0,
                    "guiding": {
                        "url": "http://localhost:11130",
                        "timeout": "90s",
                        "settle_pixels": 0.8,
                        "settle_time": "10s",
                        "settle_timeout": "1m",
                        "dither_pixels": 5.0,
                        "recalibrate_above_deg": 5.0,
                        "auth": { "username": "observatory", "password": "secret" }
                    },
                    "auth": { "username": "observatory", "password": "secret" }
                },
                "focusers": [{
                    "id": "main-focuser",
                    "alpaca_url": "http://localhost:11113",
                    "device_number": 0,
                    "min_position": 0,
                    "max_position": 100000,
                    "steps_per_sec": 1200.0,
                    "auth": { "username": "observatory", "password": "secret" }
                }],
                "filter_wheels": [{
                    "id": "main-fw",
                    "alpaca_url": "http://localhost:11123",
                    "device_number": 0,
                    "filters": ["L", "R", "G", "B"],
                    "auth": { "username": "observatory", "password": "secret" }
                }],
                "cover_calibrators": [{
                    "id": "flat-panel",
                    "alpaca_url": "http://localhost:11125",
                    "device_number": 0,
                    "poll_interval": "3s",
                    "auth": { "username": "observatory", "password": "secret" }
                }],
                "safety_monitors": [{
                    "id": "weather-watcher",
                    "alpaca_url": "http://localhost:11111",
                    "device_number": 0,
                    "auth": { "username": "observatory", "password": "secret" }
                }]
            },
            "plugins": [{ "name": "image-analyzer", "type": "event" }],
            "target_store": {
                "db_path": "/data/targets.redb",
                "default_goals": [{ "filter": "L", "binning": "1x1", "exposure_duration": "5m", "desired_count": 20 }],
                "default_scheduling": { "min_altitude_degrees": 20.0 }
            },
            "planner": { "min_altitude_degrees": 20 },
            "safety": { "poll_interval": "10s" },
            "imaging": { "cache_max_mib": 1024, "cache_max_images": 8 },
            "centering": { "solve_time_estimate": "30s", "slew_overhead_estimate": "10s" },
            "cooling": {
                "poll_interval": "10s",
                "plateau_window": "2m",
                "plateau_threshold_c": 0.5,
                "tolerance_c": 1.0,
                "max_cooler_power_pct": 90.0,
                "regulation_margin_c": 3.0,
                "max_cooldown": "20m",
                "warmup_step_interval": "2m",
                "warm_target_c": 10.0
            },
            "plate_solver": { "url": "http://localhost:11131", "timeout": "1m", "default_search_radius_deg": 3.0,
                               "auth": { "username": "observatory", "password": "secret" } },
            "server": {
                "port": 11115,
                "bind_address": "127.0.0.1",
                "tls": { "cert": "/etc/pki/rp.pem", "key": "/etc/pki/rp-key.pem" },
                "auth": { "username": "observatory", "password_hash": "$argon2id$v=19$m=19456,t=2,p=1$abc" }
            }
        });

        let config: Config = serde_json::from_value(sample).unwrap();
        let value = assert_value_round_trips(&config);

        // humantime-serde fields serialize as humantime strings, not
        // `{secs, nanos}` objects — the wire shape the schema declares.
        for (pointer, expected) in [
            ("/equipment/cameras/0/readout_time_estimate", "8s"),
            ("/equipment/mount/settle_after_slew", "3s"),
            ("/equipment/cover_calibrators/0/poll_interval", "3s"),
            ("/safety/poll_interval", "10s"),
            ("/centering/solve_time_estimate", "30s"),
            ("/centering/slew_overhead_estimate", "10s"),
            ("/cooling/poll_interval", "10s"),
            ("/cooling/plateau_window", "2m"),
            ("/cooling/max_cooldown", "20m"),
            ("/cooling/warmup_step_interval", "2m"),
            ("/plate_solver/timeout", "1m"),
            ("/equipment/mount/guiding/timeout", "1m 30s"),
            ("/equipment/mount/guiding/settle_time", "10s"),
            ("/equipment/mount/guiding/settle_timeout", "1m"),
        ] {
            assert_eq!(
                value.pointer(pointer).and_then(Value::as_str),
                Some(expected),
                "expected humantime string at {pointer}, got {:?}",
                value.pointer(pointer)
            );
        }
    }

    /// A plugin registration is otherwise opaque, but on a registration rp
    /// dials the `auth` block is read as rp's client credential — a
    /// half-written one must fail at load (and so at `PUT /api/config` and
    /// `rp doctor`) rather than be silently read as "no credential" and
    /// 401 every delivery. Both dialed types are pinned: the two paths
    /// parse the same block through different call sites.
    #[test]
    fn a_malformed_plugin_auth_block_fails_to_load() {
        for (plugin_type, url_field, url) in [
            (
                "orchestrator",
                "invoke_url",
                "http://127.0.0.1:11170/invoke",
            ),
            ("event", "webhook_url", "http://127.0.0.1:11140/webhook"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.json");
            std::fs::write(
                &path,
                format!(
                    r#"{{
                        "session": {{"data_directory": "/tmp/rp-test"}},
                        "equipment": {{}},
                        "plugins": [{{
                            "name": "a-plugin",
                            "type": "{plugin_type}",
                            "{url_field}": "{url}",
                            "subscribes_to": ["exposure_complete"],
                            "auth": {{"username": "observatory"}}
                        }}],
                        "server": {{ "port": 0 }}
                    }}"#
                ),
            )
            .unwrap();

            let error = load_config(&path).unwrap_err().to_string();
            assert!(
                error.contains("plugins.0.auth") && error.contains("password"),
                "unexpected error for {plugin_type}: {error}"
            );
        }
    }

    /// The callback URL is the registration's primary field, so it fails
    /// at load on the same terms as `auth` beside it — a bad scheme is a
    /// permanent fault, and left to first use it would only surface as a
    /// failed delivery in the middle of the night.
    #[test]
    fn a_non_http_callback_url_fails_to_load() {
        for (plugin_type, url_field) in [("orchestrator", "invoke_url"), ("event", "webhook_url")] {
            for (url, expected) in [
                ("ftp://127.0.0.1:11170/invoke", "http://"),
                ("127.0.0.1:11170/invoke", "http://"),
                ("http://", "empty host"),
                // Embedded credentials: rp logs the callback URL on every
                // attempt, and the sibling `auth` block would win anyway.
                (
                    "https://observatory:s3cret@127.0.0.1:11170/invoke",
                    "must not embed credentials",
                ),
                ("https://observatory@127.0.0.1:11170/invoke", "credentials"),
                // A credential-bearing URL that is *also* malformed is
                // rejected by an earlier branch — one that echoes the URL
                // back. The shared secret-free assertion below is the
                // point of these two: rejection must not leak what it
                // rejected.
                ("ftp://observatory:s3cret@127.0.0.1:11170/invoke", "http://"),
                ("https://observatory:s3cret@", "empty host"),
            ] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("config.json");
                std::fs::write(
                    &path,
                    format!(
                        r#"{{
                            "session": {{"data_directory": "/tmp/rp-test"}},
                            "equipment": {{}},
                            "plugins": [{{
                                "name": "a-plugin",
                                "type": "{plugin_type}",
                                "subscribes_to": ["exposure_complete"],
                                "{url_field}": "{url}"
                            }}],
                            "server": {{ "port": 0 }}
                        }}"#
                    ),
                )
                .unwrap();

                let error = load_config(&path).unwrap_err().to_string();
                assert!(
                    error.contains(&format!("plugins.0.{url_field}")) && error.contains(expected),
                    "unexpected error for {plugin_type} {url:?}: {error}"
                );
                // The whole point of rejecting embedded credentials is to
                // keep them out of logs, so the rejection itself must not
                // echo one back: a `FieldError` is rendered by the UI and
                // by `rp doctor`.
                assert!(
                    !error.contains("s3cret"),
                    "the rejection leaked the embedded password: {error}"
                );
            }
        }
    }

    /// The redaction the rejection messages depend on, pinned directly:
    /// it runs on input that may not parse, so it cannot lean on
    /// `reqwest::Url` and has to get the authority boundaries right
    /// itself.
    #[test]
    fn redact_userinfo_strips_only_the_authoritys_credentials() {
        for (url, expected) in [
            // Nothing to redact — returned untouched.
            (
                "https://127.0.0.1:11170/invoke",
                "https://127.0.0.1:11170/invoke",
            ),
            ("127.0.0.1:11170/invoke", "127.0.0.1:11170/invoke"),
            // User and user:password forms.
            (
                "https://observatory:s3cret@127.0.0.1:11170/invoke",
                "https://***@127.0.0.1:11170/invoke",
            ),
            ("https://observatory@127.0.0.1/x", "https://***@127.0.0.1/x"),
            // Malformed but still credential-bearing: the case the parse
            // branch echoes.
            ("https://observatory:s3cret@", "https://***@"),
            // Scheme-less, so the authority starts at 0.
            ("observatory:s3cret@127.0.0.1/x", "***@127.0.0.1/x"),
            // An `@` past the authority is part of the path or query, not
            // a credential, and must survive.
            (
                "https://127.0.0.1/hook@v2?to=a@b",
                "https://127.0.0.1/hook@v2?to=a@b",
            ),
        ] {
            assert_eq!(redact_userinfo(url), expected, "for {url:?}");
        }
    }

    /// A registration rp never dials keeps its own `invoke_url`-shaped
    /// key, same as its `auth`.
    #[test]
    fn an_undialed_plugins_invoke_url_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "plugins": [{
                    "name": "some-tool-provider",
                    "type": "tool_provider",
                    "invoke_url": "ipc:///run/plugin.sock"
                }],
                "server": { "port": 0 }
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(config.plugins[0]["invoke_url"], "ipc:///run/plugin.sock");
    }

    /// The opaque half of the same rule: rp interprets `auth` on the
    /// registrations it dials alone, so a tool provider's
    /// differently-shaped `auth` key — it authenticates rp's MCP client
    /// its own way — is its author's business and must not fail rp's load.
    #[test]
    fn an_undialed_plugins_own_auth_shape_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "plugins": [{
                    "name": "ml-quality-classifier",
                    "type": "tool_provider",
                    "mcp_server_url": "http://127.0.0.1:11150/mcp",
                    "auth": {"bearer_token": "the-plugin-authors-own-shape"}
                }],
                "server": { "port": 0 }
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(
            config.plugins[0]["auth"]["bearer_token"],
            "the-plugin-authors-own-shape"
        );
    }

    /// The opaque half of the same rule: a registration's other keys stay
    /// the plugin author's business, and an absent `auth` is the ordinary
    /// no-credential case.
    #[test]
    fn a_plugin_registration_without_auth_loads_with_its_own_keys_intact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "plugins": [{
                    "name": "calibrator-flats",
                    "type": "orchestrator",
                    "invoke_url": "http://127.0.0.1:11170/invoke",
                    "config": {"camera_id": "main-cam"},
                    "some_plugin_specific_key": 42
                }],
                "server": { "port": 0 }
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(config.plugins.len(), 1);
        assert_eq!(config.plugins[0]["some_plugin_specific_key"], 42);
    }
}
