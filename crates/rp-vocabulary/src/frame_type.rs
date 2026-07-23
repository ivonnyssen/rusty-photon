//! Capture intent — `Light` / `Dark` / `Flat` / `Bias`.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A capture's intent — the `{frame_type}` naming token's value.
///
/// Only `Light` frames bucket against acquisition-goal quotas; `Dark` /
/// `Flat` / `Bias` live under their own directories (see
/// [`FrameType::calibration_slug`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, derive_more::Display)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum FrameType {
    /// A science frame of a sky target.
    #[display("Light")]
    Light,
    /// A dark-current calibration frame (shutter closed).
    #[display("Dark")]
    Dark,
    /// A flat-field calibration frame.
    #[display("Flat")]
    Flat,
    /// A bias/offset calibration frame (shortest exposure).
    #[display("Bias")]
    Bias,
}

impl FrameType {
    /// The reserved `{target}` bucket slug for a calibration frame with no
    /// explicit target — the lowercased frame type. `None` for `Light`,
    /// which always requires an explicit target.
    #[must_use]
    pub fn calibration_slug(self) -> Option<&'static str> {
        match self {
            FrameType::Light => None,
            FrameType::Dark => Some("dark"),
            FrameType::Flat => Some("flat"),
            FrameType::Bias => Some("bias"),
        }
    }
}

impl FromStr for FrameType {
    type Err = FrameTypeParseError;

    /// The exact, case-sensitive inverse of the derived `Display`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Light" => Ok(FrameType::Light),
            "Dark" => Ok(FrameType::Dark),
            "Flat" => Ok(FrameType::Flat),
            "Bias" => Ok(FrameType::Bias),
            _ => Err(FrameTypeParseError(s.to_string())),
        }
    }
}

/// The input was not one of `Light` / `Dark` / `Flat` / `Bias`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid frame type {0:?}: expected Light, Dark, Flat, or Bias")]
pub struct FrameTypeParseError(String);

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const ALL: [FrameType; 4] = [
        FrameType::Light,
        FrameType::Dark,
        FrameType::Flat,
        FrameType::Bias,
    ];

    #[test]
    fn display_from_str_round_trips_every_variant() {
        for ft in ALL {
            assert_eq!(FrameType::from_str(&ft.to_string()).unwrap(), ft);
        }
    }

    #[test]
    fn from_str_is_case_sensitive_and_rejects_unknown() {
        assert!("light".parse::<FrameType>().is_err());
        assert!("Foo".parse::<FrameType>().is_err());
    }

    #[test]
    fn calibration_slug_buckets_calibration_types_and_none_for_light() {
        assert_eq!(FrameType::Light.calibration_slug(), None);
        assert_eq!(FrameType::Dark.calibration_slug(), Some("dark"));
        assert_eq!(FrameType::Flat.calibration_slug(), Some("flat"));
        assert_eq!(FrameType::Bias.calibration_slug(), Some("bias"));
    }
}
