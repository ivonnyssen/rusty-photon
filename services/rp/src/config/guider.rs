use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// HTTP-client connection to the guider rp-managed service (the
/// `phd2-guider` binary's `serve` mode), plus per-rig guiding
/// defaults.
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
pub struct GuiderConfig {
    pub url: String,
    #[serde(default = "default_guider_timeout", with = "humantime_serde")]
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
}

/// The guiding defaults carried onto `McpHandler` (parallel to
/// `plate_solver_default_search_radius_deg`): everything from
/// [`GuiderConfig`] except the connection fields, which live inside
/// the built client. `Default` (all `None`) is the not-configured
/// shape tests start from.
#[derive(Debug, Clone, Copy, Default)]
pub struct GuiderDefaults {
    pub settle_pixels: Option<f64>,
    pub settle_time: Option<Duration>,
    pub settle_timeout: Option<Duration>,
    pub dither_pixels: Option<f64>,
}

impl GuiderConfig {
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
fn default_guider_timeout() -> Duration {
    Duration::from_secs(90)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use std::time::Duration;

    use crate::config::load_config;
    use crate::config::test_support::MINIMAL_CONFIG_JSON;

    #[test]
    fn guider_block_omitted_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, MINIMAL_CONFIG_JSON).unwrap();

        let config = load_config(&path).unwrap();
        assert!(
            config.guider.is_none(),
            "expected guider to be None when omitted from config"
        );
    }

    #[test]
    fn guider_url_only_applies_default_timeout_and_no_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "guider": {"url": "http://127.0.0.1:11130"},
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        let g = config.guider.expect("guider should parse");
        assert_eq!(g.url, "http://127.0.0.1:11130");
        assert_eq!(g.timeout, Duration::from_secs(90));
        assert!(g.settle_pixels.is_none());
        assert!(g.settle_time.is_none());
        assert!(g.settle_timeout.is_none());
        assert!(g.dither_pixels.is_none());
    }

    #[test]
    fn guider_with_full_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "guider": {
                    "url": "http://127.0.0.1:11130",
                    "timeout": "2m",
                    "settle_pixels": 0.8,
                    "settle_time": "8s",
                    "settle_timeout": "40s",
                    "dither_pixels": 5.0
                },
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        let g = config.guider.expect("guider should parse");
        assert_eq!(g.timeout, Duration::from_secs(120));
        assert_eq!(g.settle_pixels, Some(0.8));
        assert_eq!(g.settle_time, Some(Duration::from_secs(8)));
        assert_eq!(g.settle_timeout, Some(Duration::from_secs(40)));
        assert_eq!(g.dither_pixels, Some(5.0));

        let defaults = g.defaults();
        assert_eq!(defaults.settle_pixels, Some(0.8));
        assert_eq!(defaults.settle_time, Some(Duration::from_secs(8)));
        assert_eq!(defaults.settle_timeout, Some(Duration::from_secs(40)));
        assert_eq!(defaults.dither_pixels, Some(5.0));
    }

    #[test]
    fn guider_rejects_unknown_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "guider": {
                    "url": "http://127.0.0.1:11130",
                    "dither_every_n_exposures": 3
                },
                "server": {}
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
}
