use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::error::{Result, RpError};

#[derive(Debug, Clone, Deserialize)]
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
    #[serde(default)]
    pub safety: Value,
    #[serde(default)]
    pub imaging: ImagingConfig,
    /// Optional plate-solver service. When `None`, the `plate_solve`
    /// MCP tool returns `plate solver not configured`. Mirrors the
    /// `Option<MountConfig>` pattern — the service is optional
    /// infrastructure, not part of the equipment surface.
    #[serde(default)]
    pub plate_solver: Option<PlateSolverConfig>,
    pub server: ServerConfig,
}

/// Observer site location. Validated at config-load time: latitude
/// must be in [-90, 90] and longitude in [-180, 180]. The IANA
/// timezone is derived from these coordinates at startup via
/// `rp-ephemeris`; elevation is intentionally omitted (see
/// `docs/services/rp.md` §"Site Configuration").
#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SiteConfig {
    pub latitude_degrees: f64,
    pub longitude_degrees: f64,
}

impl SiteConfig {
    /// Range-validate the site, returning a [`RpError::Config`] with a
    /// message naming the offending field on failure.
    pub fn validate(&self) -> Result<()> {
        if !(-90.0..=90.0).contains(&self.latitude_degrees) {
            return Err(RpError::Config(format!(
                "site.latitude_degrees must be in [-90, 90]; got {}",
                self.latitude_degrees
            )));
        }
        if !(-180.0..=180.0).contains(&self.longitude_degrees) {
            return Err(RpError::Config(format!(
                "site.longitude_degrees must be in [-180, 180]; got {}",
                self.longitude_degrees
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionConfig {
    pub data_directory: String,
    #[serde(default)]
    pub session_state_file: String,
    /// Optional template for capture filenames. `None` is the default and
    /// produces filenames of the form `<doc_uuid_8>.fits` plus a matching
    /// `.json` sidecar — fully self-identifying via the UUID-8 suffix that
    /// drives the disk-fallback resolution path. When set, the template is
    /// reserved for a future token resolver (planner/capture context feeding
    /// `{target}` / `{filter}` / etc.); until that lands `capture` ignores
    /// the value and writes `<doc_uuid_8>.fits` regardless. See
    /// `docs/services/rp.md` (Persistence) and Phase 7 of
    /// `docs/plans/image-evaluation-tools.md`.
    #[serde(default)]
    pub file_naming_pattern: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EquipmentConfig {
    #[serde(default)]
    pub cameras: Vec<CameraConfig>,
    #[serde(default)]
    pub mount: Option<MountConfig>,
    #[serde(default)]
    pub focusers: Vec<FocuserConfig>,
    #[serde(default)]
    pub filter_wheels: Vec<FilterWheelConfig>,
    #[serde(default)]
    pub cover_calibrators: Vec<CoverCalibratorConfig>,
    #[serde(default)]
    pub safety_monitors: Vec<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CameraConfig {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub alpaca_url: String,
    #[serde(default)]
    pub device_type: String,
    #[serde(default)]
    pub device_number: u32,
    #[serde(default)]
    pub cooler_target_c: Option<f64>,
    #[serde(default)]
    pub gain: Option<i32>,
    #[serde(default)]
    pub offset: Option<i32>,
    /// Effective focal length of the optical train feeding this camera,
    /// in millimetres. Used at capture time to derive pixel scale and FOV
    /// for the exposure document's `optics` block. The value lives in
    /// config because the optical train (telescope + reducer/extender) has
    /// no ASCOM Alpaca property — even the optional
    /// `Telescope.FocalLength` does not reflect anything screwed in front
    /// of the camera. Omitted → no `optics` block on captures from this
    /// camera. See `docs/services/rp.md` §"Core Fields" for the derivation
    /// and failure modes.
    #[serde(default)]
    pub focal_length_mm: Option<f64>,
    /// Optional HTTP Basic Auth credentials for connecting to auth-enabled Alpaca services
    #[serde(default)]
    pub auth: Option<rp_auth::config::ClientAuthConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FocuserConfig {
    pub id: String,
    #[serde(default)]
    pub camera_id: String,
    pub alpaca_url: String,
    #[serde(default)]
    pub device_number: u32,
    /// Operator-supplied lower bound for `move_focuser` validation. The
    /// device-reported `max_step` is the hardware ceiling; these fields
    /// let the operator enforce a tighter safe-travel range.
    #[serde(default)]
    pub min_position: Option<i32>,
    /// Operator-supplied upper bound for `move_focuser` validation.
    #[serde(default)]
    pub max_position: Option<i32>,
    /// Optional HTTP Basic Auth credentials for connecting to auth-enabled Alpaca services
    #[serde(default)]
    pub auth: Option<rp_auth::config::ClientAuthConfig>,
}

/// `rp` deployments have at most one mount — piggyback rigs share one
/// mount across multiple optical trains (multiple cameras / focusers /
/// filter wheels). Multi-mount support is in `rp.md` Future
/// Considerations. The singular `Option` reflects that contract in the
/// type; `None` is valid for camera-only / flats-rig configurations.
#[derive(Debug, Clone, Deserialize)]
pub struct MountConfig {
    pub alpaca_url: String,
    #[serde(default)]
    pub device_number: u32,
    /// Mechanical settle time applied after the mount reports
    /// `Slewing == false`, before `slew` returns. Set per-rig (gear
    /// backlash, mount mass, etc.) — defaults to zero. Per-call
    /// `settle_after` on `slew` overrides this value (including
    /// `"0s"` to skip).
    #[serde(default, with = "humantime_serde")]
    pub settle_after_slew: Option<Duration>,
    /// Optional HTTP Basic Auth credentials for connecting to auth-enabled Alpaca services
    #[serde(default)]
    pub auth: Option<rp_auth::config::ClientAuthConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FilterWheelConfig {
    pub id: String,
    #[serde(default)]
    pub camera_id: String,
    pub alpaca_url: String,
    #[serde(default)]
    pub device_number: u32,
    #[serde(default)]
    pub filters: Vec<String>,
    /// Optional HTTP Basic Auth credentials for connecting to auth-enabled Alpaca services
    #[serde(default)]
    pub auth: Option<rp_auth::config::ClientAuthConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CoverCalibratorConfig {
    pub id: String,
    pub alpaca_url: String,
    #[serde(default)]
    pub device_number: u32,
    /// Poll interval when waiting for cover/calibrator state changes (default `"3s"`)
    #[serde(
        default = "default_cover_calibrator_poll_interval",
        with = "humantime_serde"
    )]
    pub poll_interval: Duration,
    /// Optional HTTP Basic Auth credentials for connecting to auth-enabled Alpaca services
    #[serde(default)]
    pub auth: Option<rp_auth::config::ClientAuthConfig>,
}

fn default_cover_calibrator_poll_interval() -> Duration {
    Duration::from_secs(3)
}

/// Image cache + future analysis-tool tuning. Pi-5-friendly defaults.
#[derive(Debug, Clone, Deserialize)]
pub struct ImagingConfig {
    #[serde(default = "default_cache_max_mib")]
    pub cache_max_mib: usize,
    #[serde(default = "default_cache_max_images")]
    pub cache_max_images: usize,
}

impl Default for ImagingConfig {
    fn default() -> Self {
        Self {
            cache_max_mib: default_cache_max_mib(),
            cache_max_images: default_cache_max_images(),
        }
    }
}

fn default_cache_max_mib() -> usize {
    1024
}

fn default_cache_max_images() -> usize {
    8
}

/// HTTP-client connection to the `plate-solver` rp-managed service.
/// `timeout` is the connection-side outer timeout (the
/// belt-and-suspenders backstop per Tenet 1) — *not* the wrapper's
/// per-solve deadline, which is set by the `plate_solve` MCP tool's
/// per-call `timeout` parameter.
///
/// `default_search_radius_deg` is the operator-set radius applied
/// when the per-call MCP parameter is omitted; per-call overrides
/// for loaded-from-disk images where the configured rig default
/// may not match.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlateSolverConfig {
    pub url: String,
    #[serde(default = "default_plate_solver_timeout", with = "humantime_serde")]
    pub timeout: Duration,
    #[serde(default)]
    pub default_search_radius_deg: Option<f64>,
}

fn default_plate_solver_timeout() -> Duration {
    Duration::from_secs(60)
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_bind")]
    pub bind_address: String,
    #[serde(default)]
    pub tls: Option<rp_tls::config::TlsConfig>,
    #[serde(default)]
    pub auth: Option<rp_auth::config::AuthConfig>,
}

fn default_port() -> u16 {
    11115
}

fn default_bind() -> String {
    "127.0.0.1".to_string()
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
    if let Some(site) = config.site.as_ref() {
        site.validate()?;
    }
    for cam in &config.equipment.cameras {
        cam.validate()?;
    }
    Ok(config)
}

impl CameraConfig {
    /// Range-validate the camera, returning a [`RpError::Config`] with a
    /// message naming the offending field on failure. Today the only
    /// validated field is `focal_length_mm` — must be strictly positive
    /// when supplied — but the impl exists so future fields land in one
    /// canonical place.
    pub fn validate(&self) -> Result<()> {
        if let Some(f) = self.focal_length_mm {
            if !(f > 0.0 && f.is_finite()) {
                return Err(RpError::Config(format!(
                    "equipment.cameras['{}'].focal_length_mm must be a positive finite number; got {}",
                    self.id, f
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    const MINIMAL_CONFIG_JSON: &str = r#"{
        "session": {"data_directory": "/tmp/rp-test"},
        "equipment": {},
        "server": {}
    }"#;

    #[test]
    fn load_config_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, MINIMAL_CONFIG_JSON).unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(config.session.data_directory, "/tmp/rp-test");
        assert_eq!(config.server.port, 11115);
        assert_eq!(config.server.bind_address, "127.0.0.1");
        assert_eq!(config.imaging.cache_max_mib, 1024);
        assert_eq!(config.imaging.cache_max_images, 8);
    }

    #[test]
    fn imaging_config_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "imaging": {"cache_max_mib": 256, "cache_max_images": 4},
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(config.imaging.cache_max_mib, 256);
        assert_eq!(config.imaging.cache_max_images, 4);
    }

    #[test]
    fn load_config_missing_file() {
        let err = load_config(Path::new("/nonexistent/rp/config.json")).unwrap_err();
        assert!(err.to_string().contains("failed to read config file"));
    }

    #[test]
    fn file_naming_pattern_defaults_to_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, MINIMAL_CONFIG_JSON).unwrap();

        let config = load_config(&path).unwrap();
        assert!(
            config.session.file_naming_pattern.is_none(),
            "omitted file_naming_pattern must deserialize to None"
        );
    }

    #[test]
    fn file_naming_pattern_round_trips_when_set() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {
                    "data_directory": "/tmp/rp-test",
                    "file_naming_pattern": "{target}_{filter}"
                },
                "equipment": {},
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(
            config.session.file_naming_pattern.as_deref(),
            Some("{target}_{filter}")
        );
    }

    #[test]
    fn focuser_config_minimal_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "focusers": [
                        {
                            "id": "main-focuser",
                            "alpaca_url": "http://localhost:11113"
                        }
                    ]
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(config.equipment.focusers.len(), 1);
        let f = &config.equipment.focusers[0];
        assert_eq!(f.id, "main-focuser");
        assert_eq!(f.alpaca_url, "http://localhost:11113");
        assert_eq!(f.device_number, 0);
        assert!(f.min_position.is_none());
        assert!(f.max_position.is_none());
        assert!(f.auth.is_none());
    }

    #[test]
    fn focuser_config_with_bounds_and_auth() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "focusers": [
                        {
                            "id": "main-focuser",
                            "camera_id": "main-cam",
                            "alpaca_url": "http://localhost:11113",
                            "device_number": 2,
                            "min_position": 0,
                            "max_position": 100000,
                            "auth": {"username": "u", "password": "p"}
                        }
                    ]
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        let f = &config.equipment.focusers[0];
        assert_eq!(f.camera_id, "main-cam");
        assert_eq!(f.device_number, 2);
        assert_eq!(f.min_position, Some(0));
        assert_eq!(f.max_position, Some(100000));
        let auth = f.auth.as_ref().unwrap();
        assert_eq!(auth.username, "u");
        assert_eq!(auth.password, "p");
    }

    #[test]
    fn mount_config_minimal_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "mount": {
                        "alpaca_url": "http://localhost:11122"
                    }
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        let m = config.equipment.mount.as_ref().unwrap();
        assert_eq!(m.alpaca_url, "http://localhost:11122");
        assert_eq!(m.device_number, 0);
        assert!(m.settle_after_slew.is_none());
        assert!(m.auth.is_none());
    }

    #[test]
    fn mount_config_with_settle_and_auth() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "mount": {
                        "alpaca_url": "http://localhost:11122",
                        "device_number": 1,
                        "settle_after_slew": "3s",
                        "auth": {"username": "u", "password": "p"}
                    }
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        let m = config.equipment.mount.as_ref().unwrap();
        assert_eq!(m.device_number, 1);
        assert_eq!(m.settle_after_slew, Some(Duration::from_secs(3)));
        let auth = m.auth.as_ref().unwrap();
        assert_eq!(auth.username, "u");
        assert_eq!(auth.password, "p");
    }

    #[test]
    fn mount_config_omitted_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        assert!(config.equipment.mount.is_none());
    }

    #[test]
    fn site_config_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "site": {
                    "latitude_degrees": 47.6062,
                    "longitude_degrees": -122.3321
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        let site = config.site.unwrap();
        assert!((site.latitude_degrees - 47.6062).abs() < 1e-9);
        assert!((site.longitude_degrees - (-122.3321)).abs() < 1e-9);
    }

    #[test]
    fn site_config_omitted_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, MINIMAL_CONFIG_JSON).unwrap();
        let config = load_config(&path).unwrap();
        assert!(config.site.is_none());
    }

    #[test]
    fn site_config_rejects_latitude_out_of_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "site": {"latitude_degrees": 91.0, "longitude_degrees": 0.0},
                "server": {}
            }"#,
        )
        .unwrap();

        let err = load_config(&path).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("latitude_degrees") && msg.contains("[-90, 90]"),
            "expected latitude range diagnostic, got: {msg}"
        );
    }

    #[test]
    fn site_config_rejects_longitude_out_of_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "site": {"latitude_degrees": 0.0, "longitude_degrees": 181.0},
                "server": {}
            }"#,
        )
        .unwrap();

        let err = load_config(&path).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("longitude_degrees") && msg.contains("[-180, 180]"),
            "expected longitude range diagnostic, got: {msg}"
        );
    }

    #[test]
    fn site_config_rejects_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "site": {
                    "latitude_degrees": 0.0,
                    "longitude_degrees": 0.0,
                    "elevation_meters": 1000
                },
                "server": {}
            }"#,
        )
        .unwrap();

        // Elevation is explicitly out of v1 scope; surface a parse
        // error so an operator who's read the rp.md plan and added
        // an `elevation_meters` key gets a helpful failure rather
        // than silent ignoring.
        let err = load_config(&path).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("elevation_meters") || msg.contains("unknown field"),
            "expected unknown-field diagnostic, got: {msg}"
        );
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
    fn camera_config_focal_length_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "cameras": [
                        {
                            "id": "main-cam",
                            "alpaca_url": "http://localhost:11120",
                            "focal_length_mm": 540.0
                        }
                    ]
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        let cam = &config.equipment.cameras[0];
        assert_eq!(cam.focal_length_mm, Some(540.0));
    }

    #[test]
    fn camera_config_focal_length_defaults_to_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "cameras": [
                        {
                            "id": "main-cam",
                            "alpaca_url": "http://localhost:11120"
                        }
                    ]
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        assert!(
            config.equipment.cameras[0].focal_length_mm.is_none(),
            "omitted focal_length_mm must deserialize to None"
        );
    }

    #[test]
    fn camera_config_rejects_non_positive_focal_length() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "cameras": [
                        {
                            "id": "main-cam",
                            "alpaca_url": "http://localhost:11120",
                            "focal_length_mm": -100.0
                        }
                    ]
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let err = load_config(&path).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("focal_length_mm") && msg.contains("main-cam"),
            "expected focal_length diagnostic naming the camera, got: {msg}"
        );
    }

    #[test]
    fn plate_solver_block_omitted_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, MINIMAL_CONFIG_JSON).unwrap();

        let config = load_config(&path).unwrap();
        assert!(
            config.plate_solver.is_none(),
            "expected plate_solver to be None when omitted from config"
        );
    }

    #[test]
    fn plate_solver_url_only_applies_default_timeout_and_no_radius() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "plate_solver": {"url": "http://127.0.0.1:11131"},
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        let ps = config.plate_solver.expect("plate_solver should parse");
        assert_eq!(ps.url, "http://127.0.0.1:11131");
        assert_eq!(ps.timeout, Duration::from_secs(60));
        assert!(ps.default_search_radius_deg.is_none());
    }

    #[test]
    fn plate_solver_with_full_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "plate_solver": {
                    "url": "http://127.0.0.1:11131",
                    "timeout": "30s",
                    "default_search_radius_deg": 4.0
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        let ps = config.plate_solver.expect("plate_solver should parse");
        assert_eq!(ps.url, "http://127.0.0.1:11131");
        assert_eq!(ps.timeout, Duration::from_secs(30));
        assert_eq!(ps.default_search_radius_deg, Some(4.0));
    }

    #[test]
    fn plate_solver_rejects_unknown_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "plate_solver": {
                    "url": "http://127.0.0.1:11131",
                    "bogus_field": 1
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let err = load_config(&path).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("bogus_field") || msg.contains("unknown field"),
            "expected unknown-field diagnostic, got: {msg}"
        );
    }
}
