//! NASA SkyView HTTP backend + thin disk cache for the v0 simulator.
//!
//! The cache key is a `DefaultHasher`-derived hex string over the
//! `SurveyRequest` fields; that is non-cryptographic and not stable
//! across Rust versions, but the cache is local-only and operators
//! clear `cache_dir` manually (see Behavioral Contract S3).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::{debug, warn};

use crate::config::SurveyConfig;

#[derive(Debug, Error)]
pub enum SurveyError {
    #[error("survey HTTP request failed: {0}")]
    Http(String),
    #[error("survey returned non-success status {0}")]
    NonSuccess(u16),
    #[error("survey request timed out")]
    Timeout,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SurveyRequest {
    pub survey: String,
    pub ra_deg: f64,
    pub dec_deg: f64,
    pub rotation_deg: f64,
    pub pixels_x: u32,
    pub pixels_y: u32,
    pub size_x_deg: f64,
    pub size_y_deg: f64,
}

impl SurveyRequest {
    /// Stable cache key: hex of a `DefaultHasher` over rounded values
    /// (rounding masks fp drift as called out in the design doc).
    pub fn cache_key(&self) -> String {
        let mut h = DefaultHasher::new();
        self.survey.hash(&mut h);
        round_to(self.ra_deg, 1e-4).to_bits().hash(&mut h);
        round_to(self.dec_deg, 1e-4).to_bits().hash(&mut h);
        round_to(self.rotation_deg, 1e-4).to_bits().hash(&mut h);
        self.pixels_x.hash(&mut h);
        self.pixels_y.hash(&mut h);
        round_to(self.size_x_deg, 1e-6).to_bits().hash(&mut h);
        round_to(self.size_y_deg, 1e-6).to_bits().hash(&mut h);
        format!("{:016x}", h.finish())
    }
}

fn round_to(value: f64, step: f64) -> f64 {
    (value / step).round() * step
}

#[derive(Debug)]
pub struct SkyViewClient {
    http: reqwest::Client,
    endpoint: String,
}

impl SkyViewClient {
    pub fn new(config: &SurveyConfig) -> Result<Self, SurveyError> {
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|e| SurveyError::Http(e.to_string()))?;
        Ok(Self {
            http,
            endpoint: config.endpoint.clone(),
        })
    }

    pub async fn fetch(&self, req: &SurveyRequest) -> Result<Vec<u8>, SurveyError> {
        // SkyView's runquery.pl accepts these query parameters; see
        // https://skyview.gsfc.nasa.gov/current/help/fields.html.
        let pixels = format!("{},{}", req.pixels_x, req.pixels_y);
        let size = format!("{},{}", req.size_x_deg, req.size_y_deg);
        let position = format!("{},{}", req.ra_deg, req.dec_deg);
        let rotation = format!("{}", req.rotation_deg);
        let query = [
            ("Survey", req.survey.as_str()),
            ("Position", position.as_str()),
            ("Pixels", pixels.as_str()),
            ("Size", size.as_str()),
            ("Rotation", rotation.as_str()),
            ("Coordinates", "J2000"),
            ("Return", "FITS"),
        ];
        let response = self
            .http
            .get(&self.endpoint)
            .query(&query)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    SurveyError::Timeout
                } else {
                    SurveyError::Http(e.to_string())
                }
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(SurveyError::NonSuccess(status.as_u16()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| SurveyError::Http(e.to_string()))?;
        Ok(bytes.to_vec())
    }
}

/// Return cached FITS bytes if `<cache_dir>/<key>.fits` exists.
/// Errors are logged at `warn!` and treated as "no cache hit"; the
/// caller continues with a network fetch (S6).
pub fn try_cache_load(cache_dir: &Path, key: &str) -> Option<Vec<u8>> {
    let path = cache_dir.join(format!("{key}.fits"));
    match std::fs::read(&path) {
        Ok(bytes) => {
            debug!(?path, "cache hit");
            Some(bytes)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            warn!(?path, error = %e, "cache read failed; falling through to network");
            None
        }
    }
}

/// Best-effort write — failures don't fail the exposure (contract S6).
pub fn try_cache_store(cache_dir: &Path, key: &str, bytes: &[u8]) {
    let path: PathBuf = cache_dir.join(format!("{key}.fits"));
    if let Err(e) = std::fs::write(&path, bytes) {
        warn!(?path, error = %e, "cache write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_stable_across_calls() {
        let req = SurveyRequest {
            survey: "DSS2 Red".into(),
            ra_deg: 83.8221,
            dec_deg: -5.3911,
            rotation_deg: 0.0,
            pixels_x: 640,
            pixels_y: 480,
            size_x_deg: 0.5,
            size_y_deg: 0.5 * 480.0 / 640.0,
        };
        assert_eq!(req.cache_key(), req.cache_key());
    }

    #[test]
    fn cache_key_changes_with_position() {
        let mut a = SurveyRequest {
            survey: "DSS2 Red".into(),
            ra_deg: 0.0,
            dec_deg: 0.0,
            rotation_deg: 0.0,
            pixels_x: 100,
            pixels_y: 100,
            size_x_deg: 0.1,
            size_y_deg: 0.1,
        };
        let key_a = a.cache_key();
        a.ra_deg = 1.0;
        assert_ne!(key_a, a.cache_key());
    }

    #[test]
    fn cache_key_rounds_below_tolerance() {
        let mut a = SurveyRequest {
            survey: "DSS2 Red".into(),
            ra_deg: 0.0,
            dec_deg: 0.0,
            rotation_deg: 0.0,
            pixels_x: 100,
            pixels_y: 100,
            size_x_deg: 0.1,
            size_y_deg: 0.1,
        };
        let key_a = a.cache_key();
        a.ra_deg = 1e-7; // way below the 1e-4 tolerance
        assert_eq!(key_a, a.cache_key());
    }
}
