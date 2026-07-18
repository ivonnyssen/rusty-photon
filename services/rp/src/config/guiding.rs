use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// HTTP-client connection to the guider rp-managed service (the
/// `phd2-guider` binary's `serve` mode), plus per-rig guiding
/// defaults. Lives at `equipment.mount.guiding` — guiding is
/// mount-scoped: the guider corrects and dithers by moving the mount,
/// which moves every optical train on it, so the schema makes a
/// guider without a mount unrepresentable.
///
/// `timeout` is the connection-side HTTP deadline for the quick
/// endpoints (stop, pause, resume, stats); the settle-blocking calls
/// (start, dither) stretch it past the resolved settle `timeout`
/// automatically (see `rp-guider`), so it only needs raising when the
/// *service's own* configured settle timeout exceeds ~75 s and rp is
/// not overriding it per call.
///
/// The `settle_*` fields are operator-set defaults forwarded on every
/// `start_guiding` / `dither` call unless the per-call MCP parameter
/// overrides them; fields left unset are omitted from the wire and the
/// guider service's own `settling` config applies. All thresholds are
/// **guide-camera pixels** — arcseconds would require a pixel scale
/// that only exists after PHD2 calibration.
///
/// `dither_pixels` is the default `dither` amount when the per-call
/// `pixels` parameter is omitted. How *often* to dither is workflow
/// policy (Tenet 7) and deliberately not configured here.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GuidingConfig {
    pub url: String,
    #[serde(default = "default_guiding_timeout", with = "humantime_serde")]
    #[schemars(with = "String")]
    pub timeout: Duration,
    #[serde(default)]
    pub settle_pixels: Option<f64>,
    #[serde(default, with = "humantime_serde::option")]
    #[schemars(with = "Option<String>")]
    pub settle_time: Option<Duration>,
    #[serde(default, with = "humantime_serde::option")]
    #[schemars(with = "Option<String>")]
    pub settle_timeout: Option<Duration>,
    #[serde(default)]
    pub dither_pixels: Option<f64>,
    /// Rotation threshold for the rotate-while-guiding ladder (rp.md
    /// § Optical Trains, plan phase T4): when rp rotates a
    /// guide-coupled train and PHD2 reports no connected rotator, a
    /// |Δθ| above this many degrees clears the PHD2 calibration;
    /// below it the cross-axis leak (sin Δθ) sits inside guiding's
    /// noise floor. Defaults to 5°.
    #[serde(default)]
    pub recalibrate_above_deg: RecalibrateAboveDeg,
}

/// Degrees of rotation above which the guiding calibration is cleared.
///
/// Validated at load (parse-don't-validate): must be a finite number
/// in `0..=180` — 0 means "always recalibrate", and a rotator delta
/// never exceeds 180° by shortest path. Serializes transparently as
/// the inner `f64`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "f64")]
pub struct RecalibrateAboveDeg(f64);

impl RecalibrateAboveDeg {
    /// The single validating constructor. Rejects non-finite or
    /// out-of-range thresholds, naming the field in the error.
    pub fn try_new(value: f64) -> Result<Self, String> {
        if !value.is_finite() || !(0.0..=180.0).contains(&value) {
            return Err(format!(
                "recalibrate_above_deg must be a finite number of degrees in 0..=180, got {value}"
            ));
        }
        Ok(Self(value))
    }

    /// The threshold in degrees.
    pub fn value(self) -> f64 {
        self.0
    }
}

impl Default for RecalibrateAboveDeg {
    fn default() -> Self {
        Self(5.0)
    }
}

impl TryFrom<f64> for RecalibrateAboveDeg {
    type Error = String;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

/// The guiding defaults carried onto `McpHandler` (parallel to
/// `plate_solver_default_search_radius_deg`): everything from
/// [`GuidingConfig`] except the connection fields, which live inside
/// the built client. `Default` (all `None`) is the not-configured
/// shape tests start from.
#[derive(Debug, Clone, Copy, Default)]
pub struct GuiderDefaults {
    pub settle_pixels: Option<f64>,
    pub settle_time: Option<Duration>,
    pub settle_timeout: Option<Duration>,
    pub dither_pixels: Option<f64>,
}

impl GuidingConfig {
    /// The per-call defaults to carry onto the MCP handler.
    pub fn defaults(&self) -> GuiderDefaults {
        GuiderDefaults {
            settle_pixels: self.settle_pixels,
            settle_time: self.settle_time,
            settle_timeout: self.settle_timeout,
            dither_pixels: self.dither_pixels,
        }
    }
}

/// Default quick-call HTTP deadline. Sized past the guider service's
/// default settle backstop (settle timeout 60 s + 10 s grace) so an
/// url-only config never cuts a legitimate settle wait.
fn default_guiding_timeout() -> Duration {
    Duration::from_secs(90)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use std::time::Duration;

    use super::RecalibrateAboveDeg;
    use crate::config::load_config;
    use crate::config::test_support::MINIMAL_CONFIG_JSON;

    #[test]
    fn guiding_block_omitted_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, MINIMAL_CONFIG_JSON).unwrap();

        let config = load_config(&path).unwrap();
        assert!(
            config.equipment.mount.is_none(),
            "the minimal config has no mount, hence nowhere to hang guiding"
        );
    }

    #[test]
    fn guiding_url_only_applies_defaults_and_no_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "mount": {
                        "alpaca_url": "http://localhost:11122",
                        "guiding": {"url": "http://127.0.0.1:11130"}
                    }
                },
                "server": { "port": 0 }
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        let g = config
            .equipment
            .mount
            .as_ref()
            .unwrap()
            .guiding
            .as_ref()
            .expect("guiding should parse");
        assert_eq!(g.url, "http://127.0.0.1:11130");
        assert_eq!(g.timeout, Duration::from_secs(90));
        assert!(g.settle_pixels.is_none());
        assert!(g.settle_time.is_none());
        assert!(g.settle_timeout.is_none());
        assert!(g.dither_pixels.is_none());
        assert_eq!(g.recalibrate_above_deg.value(), 5.0);
    }

    #[test]
    fn guiding_with_full_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "mount": {
                        "alpaca_url": "http://localhost:11122",
                        "guiding": {
                            "url": "http://127.0.0.1:11130",
                            "timeout": "2m",
                            "settle_pixels": 0.8,
                            "settle_time": "8s",
                            "settle_timeout": "40s",
                            "dither_pixels": 5.0,
                            "recalibrate_above_deg": 10.0
                        }
                    }
                },
                "server": { "port": 0 }
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        let mount = config.equipment.mount.as_ref().unwrap();
        let g = mount.guiding.as_ref().expect("guiding should parse");
        assert_eq!(g.timeout, Duration::from_secs(120));
        assert_eq!(g.settle_pixels, Some(0.8));
        assert_eq!(g.settle_time, Some(Duration::from_secs(8)));
        assert_eq!(g.settle_timeout, Some(Duration::from_secs(40)));
        assert_eq!(g.dither_pixels, Some(5.0));
        assert_eq!(g.recalibrate_above_deg.value(), 10.0);

        let defaults = g.defaults();
        assert_eq!(defaults.settle_pixels, Some(0.8));
        assert_eq!(defaults.settle_time, Some(Duration::from_secs(8)));
        assert_eq!(defaults.settle_timeout, Some(Duration::from_secs(40)));
        assert_eq!(defaults.dither_pixels, Some(5.0));
    }

    #[test]
    fn guiding_rejects_out_of_range_recalibrate_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "mount": {
                        "alpaca_url": "http://localhost:11122",
                        "guiding": {
                            "url": "http://127.0.0.1:11130",
                            "recalibrate_above_deg": 181.0
                        }
                    }
                },
                "server": { "port": 0 }
            }"#,
        )
        .unwrap();

        let err = load_config(&path).unwrap_err().to_string();
        assert!(
            err.contains("recalibrate_above_deg must be a finite number of degrees in 0..=180"),
            "{err}"
        );
    }

    #[test]
    fn guiding_rejects_unknown_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {
                    "mount": {
                        "alpaca_url": "http://localhost:11122",
                        "guiding": {
                            "url": "http://127.0.0.1:11130",
                            "dither_every_n_exposures": 3
                        }
                    }
                },
                "server": { "port": 0 }
            }"#,
        )
        .unwrap();

        let err = load_config(&path).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("dither_every_n_exposures") || msg.contains("unknown field"),
            "expected unknown-field diagnostic, got: {msg}"
        );
    }

    #[test]
    fn recalibrate_above_deg_newtype_validation_boundaries() {
        assert_eq!(RecalibrateAboveDeg::default().value(), 5.0);
        assert_eq!(RecalibrateAboveDeg::try_new(0.0).unwrap().value(), 0.0);
        assert_eq!(RecalibrateAboveDeg::try_new(180.0).unwrap().value(), 180.0);
        assert!(RecalibrateAboveDeg::try_new(-0.1)
            .unwrap_err()
            .contains("recalibrate_above_deg"));
        assert!(RecalibrateAboveDeg::try_new(180.1).is_err());
        assert!(RecalibrateAboveDeg::try_new(f64::NAN).is_err());
        assert!(RecalibrateAboveDeg::try_new(f64::INFINITY).is_err());
    }
}
