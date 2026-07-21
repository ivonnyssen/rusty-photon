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
pub use optical_train::{FocalLengthMm, OpticalTrainConfig, TrainAutoFocusConfig, TrainPurpose};
pub use plate_solver::PlateSolverConfig;
pub use rotator::RotatorConfig;
pub use safety::SafetyConfig;
pub use safety_monitor::SafetyMonitorConfig;
pub use server::ServerConfig;
pub use session::SessionConfig;
pub use site::SiteConfig;
pub use switch::SwitchConfig;

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
    #[serde(default)]
    pub targets: Value,
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
/// `StateDirectory` (`/var/lib/rusty-photon/rp/`) on Unix,
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

/// The scaffold's platform-dependent `session.data_directory` default.
#[cfg(not(windows))]
fn default_data_directory() -> String {
    "/var/lib/rusty-photon/rp/data".to_string()
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
    errors
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
        #[cfg(not(windows))]
        assert_eq!(
            config.session.data_directory,
            "/var/lib/rusty-photon/rp/data"
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
            "targets": [{ "name": "M31", "ra_hours": 0.712, "dec_degrees": 41.27 }],
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
}
