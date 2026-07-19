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
    /// § Rotator Tool Details): when rp rotates a guide-coupled train
    /// and PHD2 reports no connected rotator, a |Δθ| above this many
    /// degrees clears the PHD2 calibration; below it the cross-axis
    /// leak (sin Δθ) sits inside guiding's noise floor. Defaults
    /// to 5°.
    #[serde(default)]
    pub recalibrate_above_deg: RecalibrateAboveDeg,
    /// The Guide Focus Watch (rp.md § Guide Focus Watch). Omitted →
    /// the watch is disabled.
    #[serde(default)]
    pub focus_watch: Option<FocusWatchConfig>,
}

/// The `focus_watch` sub-block: thresholds turning a degrading HFD
/// trend into `guide_focus_degraded` / `guide_focus_escalation`
/// events. Every field is optional; the block's presence enables the
/// watch.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FocusWatchConfig {
    /// Frames per median (baseline and trailing). Default 10.
    #[serde(default)]
    pub window: WatchWindow,
    /// Degradation threshold: trailing median > baseline × ratio.
    /// Default 1.25.
    #[serde(default)]
    pub degrade_ratio: DegradeRatio,
    /// Minimum spacing between `guide_focus_degraded` emissions.
    /// Default 10 minutes.
    #[serde(default = "default_watch_cooldown", with = "humantime_serde")]
    #[schemars(with = "String")]
    pub cooldown: Duration,
    /// How long a degradation episode may persist after the degraded
    /// event before `guide_focus_escalation` fires. Default
    /// 10 minutes.
    #[serde(default = "default_watch_escalation", with = "humantime_serde")]
    #[schemars(with = "String")]
    pub escalation_deadline: Duration,
    /// Metrics poll cadence while guiding is active. Default 5 s.
    #[serde(default = "default_watch_poll", with = "humantime_serde")]
    #[schemars(with = "String")]
    pub poll_interval: Duration,
}

impl Default for FocusWatchConfig {
    fn default() -> Self {
        Self {
            window: WatchWindow::default(),
            degrade_ratio: DegradeRatio::default(),
            cooldown: default_watch_cooldown(),
            escalation_deadline: default_watch_escalation(),
            poll_interval: default_watch_poll(),
        }
    }
}

fn default_watch_cooldown() -> Duration {
    Duration::from_secs(600)
}

fn default_watch_escalation() -> Duration {
    Duration::from_secs(600)
}

fn default_watch_poll() -> Duration {
    Duration::from_secs(5)
}

/// The focus watch's median window in frames. Parse-don't-validate:
/// a parabola of medians needs history — fewer than 3 frames is
/// noise, rejected at load. Defaults to 10.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "i64")]
pub struct WatchWindow(u32);

impl WatchWindow {
    pub fn value(self) -> usize {
        self.0 as usize
    }
}

impl Default for WatchWindow {
    fn default() -> Self {
        Self(10)
    }
}

impl TryFrom<i64> for WatchWindow {
    type Error = String;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        match u32::try_from(value) {
            Ok(v) if v >= 3 => Ok(Self(v)),
            _ => Err(format!(
                "focus_watch.window must be an integer >= 3, got {value}"
            )),
        }
    }
}

/// The focus watch's degradation ratio. Must be a finite number
/// strictly above 1.0 — at or below it the watch would fire on
/// noise. Defaults to 1.25.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "f64")]
pub struct DegradeRatio(f64);

impl DegradeRatio {
    pub fn value(self) -> f64 {
        self.0
    }
}

impl Default for DegradeRatio {
    fn default() -> Self {
        Self(1.25)
    }
}

impl TryFrom<f64> for DegradeRatio {
    type Error = String;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        if !value.is_finite() || value <= 1.0 {
            return Err(format!(
                "focus_watch.degrade_ratio must be a finite number > 1.0, got {value}"
            ));
        }
        Ok(Self(value))
    }
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
/// the built client. `Default` (all `None`, threshold at 5°) is the
/// not-configured shape tests start from.
#[derive(Debug, Clone, Copy)]
pub struct GuiderDefaults {
    pub settle_pixels: Option<f64>,
    pub settle_time: Option<Duration>,
    pub settle_timeout: Option<Duration>,
    pub dither_pixels: Option<f64>,
    /// The rotate-while-guiding ladder's recalibration threshold, in
    /// degrees (rp.md § Rotator Tool Details).
    pub recalibrate_above_deg: f64,
}

impl Default for GuiderDefaults {
    fn default() -> Self {
        Self {
            settle_pixels: None,
            settle_time: None,
            settle_timeout: None,
            dither_pixels: None,
            recalibrate_above_deg: RecalibrateAboveDeg::default().value(),
        }
    }
}

impl GuidingConfig {
    /// The per-call defaults to carry onto the MCP handler.
    pub fn defaults(&self) -> GuiderDefaults {
        GuiderDefaults {
            settle_pixels: self.settle_pixels,
            settle_time: self.settle_time,
            settle_timeout: self.settle_timeout,
            dither_pixels: self.dither_pixels,
            recalibrate_above_deg: self.recalibrate_above_deg.value(),
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
