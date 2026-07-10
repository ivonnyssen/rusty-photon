//! Configuration types and JSON loader.
//!
//! Top-level [`Config`] is built by [`load_config`] from a JSON file.
//! Each domain-specific block lives in a sibling module:
//! [`session`], [`site`], [`equipment`] (plus the per-device-type
//! configs [`camera`], [`focuser`], [`mount`], [`filter_wheel`],
//! [`cover_calibrator`]), [`imaging`], [`plate_solver`], [`guider`],
//! [`server`].
//! The submodules' public types are re-exported here so existing
//! `crate::config::CameraConfig` callsites keep working unchanged.

pub mod camera;
pub mod centering;
pub mod cover_calibrator;
pub mod equipment;
pub mod filter_wheel;
pub mod focuser;
pub mod guider;
pub mod imaging;
pub mod mount;
pub mod plate_solver;
pub mod safety;
pub mod safety_monitor;
pub mod server;
pub mod session;
pub mod site;

pub use camera::CameraConfig;
pub use centering::CenteringConfig;
pub use cover_calibrator::CoverCalibratorConfig;
pub use equipment::EquipmentConfig;
pub use filter_wheel::FilterWheelConfig;
pub use focuser::FocuserConfig;
pub use guider::{GuiderConfig, GuiderDefaults};
pub use imaging::ImagingConfig;
pub use mount::MountConfig;
pub use plate_solver::PlateSolverConfig;
pub use safety::SafetyConfig;
pub use safety_monitor::SafetyMonitorConfig;
pub use server::ServerConfig;
pub use session::SessionConfig;
pub use site::SiteConfig;

use std::path::Path;

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
    /// Optional plate-solver service. When `None`, the `plate_solve`
    /// MCP tool returns `plate solver not configured`. Mirrors the
    /// `Option<MountConfig>` pattern — the service is optional
    /// infrastructure, not part of the equipment surface.
    #[serde(default)]
    pub plate_solver: Option<PlateSolverConfig>,
    /// Optional guider service. When `None`, the guiding MCP tools
    /// (`start_guiding`, `stop_guiding`, `dither`, ...) return
    /// `guider not configured` and the safety enforcer skips its
    /// stop-guiding step. Same optional-infrastructure shape as
    /// `plate_solver`.
    #[serde(default)]
    pub guider: Option<GuiderConfig>,
    pub server: ServerConfig,
}

/// Minimal runnable scaffold `rp` writes on first start when no config
/// exists at the XDG default path: no equipment, default server, session
/// data under the packaged unit's `StateDirectory`
/// (`/var/lib/rusty-photon/rp/`). Must stay deserializable into
/// [`Config`] — the packaged first-start contract depends on it.
pub fn default_scaffold() -> serde_json::Value {
    serde_json::json!({
        "session": { "data_directory": "/var/lib/rusty-photon/rp/data" },
        "equipment": {},
        "server": {}
    })
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
pub(crate) mod test_support {
    pub const MINIMAL_CONFIG_JSON: &str = r#"{
        "session": {"data_directory": "/tmp/rp-test"},
        "equipment": {},
        "server": {}
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
        assert_eq!(
            config.session.data_directory,
            "/var/lib/rusty-photon/rp/data"
        );
        assert!(config.equipment.cameras.is_empty());
        assert!(config.site.is_none());
        assert_eq!(config.server.port, 11115);
    }

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
    fn load_config_missing_file() {
        let err = load_config(Path::new("/nonexistent/rp/config.json")).unwrap_err();
        assert!(err.to_string().contains("failed to read config file"));
    }

    #[test]
    fn load_config_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "not valid json").unwrap();

        let err = load_config(&path).unwrap_err();
        assert!(err.to_string().contains("failed to parse config file"));
    }
}
