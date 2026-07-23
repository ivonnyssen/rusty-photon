//! Frame binning — the `(x, y)` factor pair, rendered `"2x2"`.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Frame binning, rendered as `"{x}x{y}"` (e.g. `"1x1"`, `"2x2"`).
///
/// The `(filter, binning, exposure)` triple is the acquisition quota key.
/// `x`/`y` stay public: any `u8 × u8` is shape-valid, so there is no bound
/// to protect. [`FromStr`] is the exact inverse of the derived `Display`,
/// so a rendered `{binning}` token round-trips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, derive_more::Display)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[display("{x}x{y}")]
pub struct Binning {
    /// Horizontal binning factor.
    pub x: u8,
    /// Vertical binning factor.
    pub y: u8,
}

impl FromStr for Binning {
    type Err = BinningParseError;

    /// Parses `"AxB"` (e.g. `"1x1"`, `"2x2"`) into a [`Binning`].
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (x, y) = s
            .split_once('x')
            .ok_or_else(|| BinningParseError(s.to_string()))?;
        let x = x
            .parse::<u8>()
            .map_err(|_| BinningParseError(s.to_string()))?;
        let y = y
            .parse::<u8>()
            .map_err(|_| BinningParseError(s.to_string()))?;
        Ok(Self { x, y })
    }
}

/// The input was not `"AxB"` with two `u8` factors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid binning {0:?}: expected \"AxB\", e.g. \"1x1\"")]
pub struct BinningParseError(String);

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn displays_as_wxh() {
        assert_eq!(Binning { x: 2, y: 2 }.to_string(), "2x2");
    }

    #[test]
    fn from_str_round_trips_display() {
        let b = Binning { x: 3, y: 1 };
        assert_eq!(Binning::from_str(&b.to_string()).unwrap(), b);
    }

    #[test]
    fn from_str_rejects_non_axb() {
        assert!("1".parse::<Binning>().is_err());
        assert!("1x".parse::<Binning>().is_err());
        assert!("x1".parse::<Binning>().is_err());
        assert!("1x1x1".parse::<Binning>().is_err());
    }

    #[test]
    fn from_str_rejects_out_of_u8_range() {
        assert!("999x1".parse::<Binning>().is_err());
    }
}
