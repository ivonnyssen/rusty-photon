use rp_auth::config::ClientAuthConfig;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::SkySurveyCameraError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub device: DeviceConfig,
    pub optics: OpticsConfig,
    pub pointing: PointingConfig,
    pub survey: SurveyConfig,
    pub server: ServerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub name: String,
    pub unique_id: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpticsConfig {
    pub focal_length_mm: f64,
    pub pixel_size_x_um: f64,
    pub pixel_size_y_um: f64,
    pub sensor_width_px: u32,
    pub sensor_height_px: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointingConfig {
    pub initial_ra_deg: f64,
    pub initial_dec_deg: f64,
    #[serde(default)]
    pub initial_rotation_deg: f64,
    /// When present, switches the camera into telescope-following mode:
    /// `StartExposure` reads RA/Dec from the configured ASCOM Telescope
    /// instead of from the cached `PointingState`. See the
    /// "Telescope follow mode" section of the service design doc and
    /// the F1–F6 contracts for behaviour.
    #[serde(default)]
    pub telescope: Option<TelescopeFollowConfig>,
}

/// Configuration for telescope-following mode. Absent in static mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelescopeFollowConfig {
    pub alpaca_url: String,
    #[serde(default)]
    pub device_number: u32,
    /// Constant offset added to mount RA before the SkyView request.
    /// Phase 2 keeps this at the default `0.0`; Phase 3 uses it.
    #[serde(default)]
    pub offset_ra_arcsec: f64,
    /// Constant offset added to mount Dec before the SkyView request.
    #[serde(default)]
    pub offset_dec_arcsec: f64,
    /// Per-read timeout on `right_ascension` / `declination` against
    /// the ASCOM Telescope. Bounds the latency a wedged mount can add
    /// to `StartExposure`.
    #[serde(
        default = "default_telescope_request_timeout",
        with = "humantime_serde"
    )]
    pub request_timeout: Duration,
    #[serde(default)]
    pub auth: Option<ClientAuthConfig>,
}

fn default_telescope_request_timeout() -> Duration {
    Duration::from_secs(2)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurveyConfig {
    pub name: String,
    #[serde(with = "humantime_serde")]
    pub request_timeout: Duration,
    pub cache_dir: PathBuf,
    /// Base URL the SurveyClient hits. Defaults to NASA SkyView; tests
    /// override it with a stub server.
    #[serde(default = "default_survey_endpoint")]
    pub endpoint: String,
}

fn default_survey_endpoint() -> String {
    "https://skyview.gsfc.nasa.gov/current/cgi/runquery.pl".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
}

pub async fn load_config(path: &Path) -> Result<Config, SkySurveyCameraError> {
    let bytes = tokio::fs::read(path).await?;
    let config: Config = serde_json::from_slice(&bytes)?;
    validate(&config)?;
    Ok(config)
}

fn validate(config: &Config) -> Result<(), SkySurveyCameraError> {
    if let Some(t) = &config.pointing.telescope {
        if !t.offset_ra_arcsec.is_finite() {
            return Err(SkySurveyCameraError::ConfigInvalid(
                "pointing.telescope.offset_ra_arcsec must be finite".into(),
            ));
        }
        if !t.offset_dec_arcsec.is_finite() {
            return Err(SkySurveyCameraError::ConfigInvalid(
                "pointing.telescope.offset_dec_arcsec must be finite".into(),
            ));
        }
        if t.request_timeout.is_zero() {
            return Err(SkySurveyCameraError::ConfigInvalid(
                "pointing.telescope.request_timeout must be > 0".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn base_config_with_telescope(telescope: Option<TelescopeFollowConfig>) -> Config {
        Config {
            device: DeviceConfig {
                name: "n".into(),
                unique_id: "u".into(),
                description: "d".into(),
            },
            optics: OpticsConfig {
                focal_length_mm: 1000.0,
                pixel_size_x_um: 3.76,
                pixel_size_y_um: 3.76,
                sensor_width_px: 100,
                sensor_height_px: 100,
            },
            pointing: PointingConfig {
                initial_ra_deg: 0.0,
                initial_dec_deg: 0.0,
                initial_rotation_deg: 0.0,
                telescope,
            },
            survey: SurveyConfig {
                name: "DSS2 Red".into(),
                request_timeout: Duration::from_secs(30),
                cache_dir: PathBuf::from("/tmp"),
                endpoint: default_survey_endpoint(),
            },
            server: ServerConfig { port: 0 },
        }
    }

    fn telescope_config() -> TelescopeFollowConfig {
        TelescopeFollowConfig {
            alpaca_url: "http://127.0.0.1:32323".into(),
            device_number: 0,
            offset_ra_arcsec: 0.0,
            offset_dec_arcsec: 0.0,
            request_timeout: Duration::from_secs(2),
            auth: None,
        }
    }

    #[test]
    fn validate_accepts_static_mode() {
        validate(&base_config_with_telescope(None)).unwrap();
    }

    #[test]
    fn validate_accepts_zero_offset() {
        validate(&base_config_with_telescope(Some(telescope_config()))).unwrap();
    }

    #[test]
    fn validate_rejects_nan_ra_offset() {
        let mut t = telescope_config();
        t.offset_ra_arcsec = f64::NAN;
        let err = validate(&base_config_with_telescope(Some(t))).unwrap_err();
        assert!(format!("{err}").contains("offset_ra_arcsec"));
    }

    #[test]
    fn validate_rejects_infinite_dec_offset() {
        let mut t = telescope_config();
        t.offset_dec_arcsec = f64::INFINITY;
        let err = validate(&base_config_with_telescope(Some(t))).unwrap_err();
        assert!(format!("{err}").contains("offset_dec_arcsec"));
    }

    #[test]
    fn validate_rejects_zero_request_timeout() {
        let mut t = telescope_config();
        t.request_timeout = Duration::ZERO;
        let err = validate(&base_config_with_telescope(Some(t))).unwrap_err();
        assert!(format!("{err}").contains("request_timeout"));
    }

    #[test]
    fn telescope_block_round_trips() {
        let json = r#"{
            "alpaca_url": "http://example/",
            "device_number": 1,
            "offset_ra_arcsec": 60.0,
            "offset_dec_arcsec": -45.0,
            "request_timeout": "5s"
        }"#;
        let cfg: TelescopeFollowConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.alpaca_url, "http://example/");
        assert_eq!(cfg.device_number, 1);
        assert_eq!(cfg.offset_ra_arcsec, 60.0);
        assert_eq!(cfg.offset_dec_arcsec, -45.0);
        assert_eq!(cfg.request_timeout, Duration::from_secs(5));
        assert!(cfg.auth.is_none());
    }

    #[test]
    fn telescope_defaults_when_optional_fields_omitted() {
        let json = r#"{ "alpaca_url": "http://x/" }"#;
        let cfg: TelescopeFollowConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.device_number, 0);
        assert_eq!(cfg.offset_ra_arcsec, 0.0);
        assert_eq!(cfg.offset_dec_arcsec, 0.0);
        assert_eq!(cfg.request_timeout, Duration::from_secs(2));
    }

    #[test]
    fn pointing_telescope_absent_by_default() {
        let json = r#"{
            "initial_ra_deg": 0.0,
            "initial_dec_deg": 0.0
        }"#;
        let cfg: PointingConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.telescope.is_none());
    }
}
