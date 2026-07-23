//! Per-frame exposure duration, owning both its string forms.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// A per-frame exposure duration.
///
/// Owns *both* of its string forms so they cannot drift: the value/wire
/// form (`"300s"`, seconds-exact — [`Serialize`]/[`Deserialize`]) and the
/// whole-second filename token (`"300sec"` —
/// [`Exposure::to_filename_token`]). Deserialization tolerates any
/// humantime spelling (`"5m"`, `"500ms"`) but always re-emits the
/// seconds-exact form, so a whole-second exposure round-trips byte-for-byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(try_from = "ExposureWire", into = "ExposureWire")]
pub struct Exposure(Duration);

impl Exposure {
    /// Wraps a non-zero [`Duration`].
    ///
    /// # Errors
    ///
    /// [`ExposureError::Zero`] for a zero duration.
    pub fn try_new(d: Duration) -> Result<Self, ExposureError> {
        if d.is_zero() {
            return Err(ExposureError::Zero);
        }
        Ok(Self(d))
    }

    /// The underlying duration.
    #[must_use]
    pub fn as_duration(&self) -> Duration {
        self.0
    }

    /// The whole-second filename token, e.g. `"300sec"`.
    ///
    /// # Errors
    ///
    /// [`ExposureError::SubSecond`] if the duration is not a whole number
    /// of seconds — filenames have no sub-second representation.
    pub fn to_filename_token(&self) -> Result<String, ExposureError> {
        if self.0.subsec_nanos() != 0 {
            return Err(ExposureError::SubSecond(self.0));
        }
        Ok(format!("{}sec", self.0.as_secs()))
    }

    /// Parses a `"300sec"` filename token back into an [`Exposure`].
    ///
    /// # Errors
    ///
    /// [`ExposureError::BadFilenameToken`] if `s` isn't `"<digits>sec"`, or
    /// [`ExposureError::Zero`] for `"0sec"`.
    pub fn from_filename_token(s: &str) -> Result<Self, ExposureError> {
        let secs = s
            .strip_suffix("sec")
            .and_then(|d| d.parse::<u64>().ok())
            .ok_or_else(|| ExposureError::BadFilenameToken(s.to_string()))?;
        Self::try_new(Duration::from_secs(secs))
    }

    /// The seconds-exact value form (`"300s"`); humantime's coarser
    /// spelling only for a sub-second duration.
    fn to_value_string(self) -> String {
        if self.0.subsec_nanos() == 0 {
            format!("{}s", self.0.as_secs())
        } else {
            humantime::format_duration(self.0).to_string()
        }
    }
}

/// The string wire form [`Exposure`] (de)serializes through.
#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
struct ExposureWire(String);

impl TryFrom<ExposureWire> for Exposure {
    type Error = ExposureError;

    fn try_from(w: ExposureWire) -> Result<Self, Self::Error> {
        let d = humantime::parse_duration(&w.0)
            .map_err(|_| ExposureError::BadValueString(w.0.clone()))?;
        Self::try_new(d)
    }
}

impl From<Exposure> for ExposureWire {
    fn from(e: Exposure) -> Self {
        Self(e.to_value_string())
    }
}

/// Errors constructing or formatting an [`Exposure`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExposureError {
    /// The duration was zero.
    #[error("exposure cannot be zero")]
    Zero,
    /// The value string was not a valid humantime duration.
    #[error("exposure value {0:?} is not a valid duration")]
    BadValueString(String),
    /// The filename token was not `"<seconds>sec"`.
    #[error("filename exposure token {0:?} is not \"<seconds>sec\"")]
    BadFilenameToken(String),
    /// A sub-second duration has no filename-token representation.
    #[error("exposure {0:?} is not a whole number of seconds; filenames have no sub-second form")]
    SubSecond(Duration),
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn try_new_rejects_zero() {
        assert_eq!(Exposure::try_new(Duration::ZERO), Err(ExposureError::Zero));
    }

    #[test]
    fn value_form_is_seconds_exact() {
        let e = Exposure::try_new(Duration::from_secs(300)).unwrap();
        assert_eq!(serde_json::to_value(e).unwrap(), serde_json::json!("300s"));
    }

    #[test]
    fn value_form_round_trips_and_tolerates_humantime_input() {
        let e: Exposure = serde_json::from_value(serde_json::json!("5m")).unwrap();
        assert_eq!(e.as_duration(), Duration::from_secs(300));
        assert_eq!(serde_json::to_value(e).unwrap(), serde_json::json!("300s"));
    }

    #[test]
    fn filename_token_round_trips_whole_seconds() {
        let e = Exposure::try_new(Duration::from_secs(120)).unwrap();
        assert_eq!(e.to_filename_token().unwrap(), "120sec");
        assert_eq!(Exposure::from_filename_token("120sec").unwrap(), e);
    }

    #[test]
    fn filename_token_rejects_sub_second() {
        let e = Exposure::try_new(Duration::from_millis(1500)).unwrap();
        assert!(e.to_filename_token().is_err());
    }

    #[test]
    fn from_filename_token_rejects_malformed_or_zero() {
        assert!(Exposure::from_filename_token("120").is_err());
        assert!(Exposure::from_filename_token("abcsec").is_err());
        assert!(Exposure::from_filename_token("0sec").is_err());
    }
}
