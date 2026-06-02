use std::time::Duration;

use serde::Deserialize;

/// Conservative default GoTo slew rate: 2°/s = 7200 arcsec/s. Deliberately
/// slower than real GoTo mounts (3–4°/s) so the predicted slew duration
/// over-estimates and the deadline won't false-abort a healthy slew.
const DEFAULT_SLEW_RATE_ARCSEC_PER_SEC: f64 = 7200.0;

/// Assumed mount GoTo slew rate in arcsec/sec, feeding the predictive slew
/// deadline (`predicted = great-circle distance / rate + settle`). The
/// generic Alpaca `Telescope` trait exposes no GoTo-rate property, so this
/// config value is the rate source; set it per-rig for a tighter deadline.
///
/// Validated at load (parse-don't-validate): a non-finite or non-positive
/// rate is rejected during deserialization, so a bad config fails at
/// startup rather than at slew time.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(try_from = "f64")]
pub struct SlewRateArcsecPerSec(f64);

impl SlewRateArcsecPerSec {
    /// The single validating constructor. Rejects non-finite or
    /// non-positive rates, naming the field in the error.
    pub fn try_new(value: f64) -> Result<Self, String> {
        if !value.is_finite() || value <= 0.0 {
            return Err(format!(
                "slew_rate_arcsec_per_sec must be a finite positive number, got {value}"
            ));
        }
        Ok(Self(value))
    }

    /// The rate in arcsec/sec.
    pub fn value(self) -> f64 {
        self.0
    }
}

impl Default for SlewRateArcsecPerSec {
    fn default() -> Self {
        Self(DEFAULT_SLEW_RATE_ARCSEC_PER_SEC)
    }
}

impl TryFrom<f64> for SlewRateArcsecPerSec {
    type Error = String;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
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
    /// Assumed mount GoTo slew rate (arcsec/sec) used to size the
    /// predictive slew deadline. Defaults to 7200 (2°/s), a conservative
    /// slow-stepper rate; set per-rig for a tighter bound.
    #[serde(default)]
    pub slew_rate_arcsec_per_sec: SlewRateArcsecPerSec,
    /// Optional HTTP Basic Auth credentials for connecting to auth-enabled Alpaca services
    #[serde(default)]
    pub auth: Option<rp_auth::config::ClientAuthConfig>,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use std::time::Duration;

    use crate::config::load_config;

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
    fn mount_config_slew_rate_defaults_to_7200() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "mount": {"alpaca_url": "http://localhost:11122"}
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        let m = config.equipment.mount.as_ref().unwrap();
        assert_eq!(m.slew_rate_arcsec_per_sec.value(), 7200.0);
    }

    #[test]
    fn mount_config_slew_rate_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "mount": {
                        "alpaca_url": "http://localhost:11122",
                        "slew_rate_arcsec_per_sec": 3600
                    }
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        let m = config.equipment.mount.as_ref().unwrap();
        assert_eq!(m.slew_rate_arcsec_per_sec.value(), 3600.0);
    }

    #[test]
    fn mount_config_slew_rate_rejects_non_positive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "mount": {
                        "alpaca_url": "http://localhost:11122",
                        "slew_rate_arcsec_per_sec": -1.0
                    }
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let err = load_config(&path).unwrap_err().to_string();
        assert!(
            err.contains("slew_rate_arcsec_per_sec must be a finite positive number"),
            "expected the validation message, got: {err}"
        );
    }

    #[test]
    fn slew_rate_newtype_validation_boundaries() {
        use super::SlewRateArcsecPerSec;
        assert_eq!(SlewRateArcsecPerSec::default().value(), 7200.0);
        assert_eq!(
            SlewRateArcsecPerSec::try_new(3600.0).unwrap().value(),
            3600.0
        );
        // The `<= 0.0` edge and the non-finite branch (unreachable from
        // JSON, defensive-only) are rejected and name the field.
        assert!(SlewRateArcsecPerSec::try_new(0.0)
            .unwrap_err()
            .contains("slew_rate_arcsec_per_sec"));
        assert!(SlewRateArcsecPerSec::try_new(-1.0).is_err());
        assert!(SlewRateArcsecPerSec::try_new(f64::NAN).is_err());
        assert!(SlewRateArcsecPerSec::try_new(f64::INFINITY).is_err());
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
}
