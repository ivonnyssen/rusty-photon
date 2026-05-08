use serde::Deserialize;

use crate::error::{Result, RpError};

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
    use crate::config::load_config;

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
}
