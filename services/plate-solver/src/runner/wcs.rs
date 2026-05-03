//! ASTAP `.wcs` sidecar parser.
//!
//! ASTAP writes a `.wcs` sidecar next to each successfully solved FITS,
//! containing the World Coordinate System keywords as 80-character FITS
//! cards.
//!
//! ## Implementation note (Phase 2 spike)
//!
//! The plan calls for parsing via `fitsrs` + `wcs::WCSParams`. In practice
//! `.wcs` is a FITS *header only* (no data block), and `fitsrs::Fits::
//! from_reader` expects a full FITS file. Until a real ASTAP `.wcs` is
//! validated against `fitsrs` (Phase 2 spike — open question), we ship a
//! minimal hand-rolled card parser that only reads the four keywords the
//! HTTP contract returns. The parser is small (<100 lines) and exhaustively
//! tested. When the spike retires, the implementation swaps to
//! `WCSParams::deserialize` without changing the public surface
//! (`read_wcs_sidecar`).

use super::SolveOutcome;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WcsParseError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("required keyword `{0}` not found in .wcs sidecar")]
    MissingKeyword(&'static str),

    #[error("keyword `{key}` has non-numeric value: {value}")]
    NonNumeric { key: &'static str, value: String },
}

/// Parse a `.wcs` sidecar file and return the four fields the HTTP contract
/// surfaces, plus a solver banner string read from the file's history /
/// comment cards.
pub fn read_wcs_sidecar(path: &Path) -> Result<SolveOutcome, WcsParseError> {
    let bytes = std::fs::read(path)?;
    parse_wcs_bytes(&bytes)
}

/// Pure-function variant for unit testing.
pub fn parse_wcs_bytes(bytes: &[u8]) -> Result<SolveOutcome, WcsParseError> {
    let cards = split_cards(bytes);

    let crval1 = read_float(&cards, "CRVAL1")?;
    let crval2 = read_float(&cards, "CRVAL2")?;
    let cdelt1 = read_float(&cards, "CDELT1")?;
    let crota2 = read_float_optional(&cards, "CROTA2").unwrap_or(0.0);

    // Banner: pulled from a HISTORY card whose value contains "ASTAP" or
    // similar. Not strictly required by the HTTP contract today but kept
    // in the response for diagnostic value.
    let solver = find_banner(&cards).unwrap_or_else(|| "astap-cli".to_string());

    Ok(SolveOutcome {
        ra_center: crval1,
        dec_center: crval2,
        pixel_scale_arcsec: cdelt1.abs() * 3600.0,
        rotation_deg: crota2,
        solver,
    })
}

/// Split a FITS-style header byte stream into 80-character cards. Stops at
/// the END card (or EOF). Trailing 2880-byte block padding is ignored.
fn split_cards(bytes: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    for chunk in bytes.chunks(80) {
        if chunk.len() < 80 {
            break;
        }
        if chunk.starts_with(b"END     ") {
            break;
        }
        out.push(chunk);
    }
    out
}

/// Read the value column of the first card with a matching 8-byte keyword,
/// trimming the standard `KEYWORD = ...VALUE... / comment` shape.
fn card_value<'a>(cards: &'a [&'a [u8]], key: &str) -> Option<&'a str> {
    let key_padded = format!("{key:<8}");
    for card in cards {
        if !card.starts_with(key_padded.as_bytes()) {
            continue;
        }
        // After the 8-byte keyword, FITS uses "= " (cols 9-10) then the
        // value. Comment after " / ". We're lax: anything after column 10
        // up to the first " /" or EOL is the value.
        let after_eq = std::str::from_utf8(&card[10..]).ok()?;
        let value_str = match after_eq.find(" /") {
            Some(idx) => &after_eq[..idx],
            None => after_eq,
        };
        return Some(value_str.trim());
    }
    None
}

fn read_float(cards: &[&[u8]], key: &'static str) -> Result<f64, WcsParseError> {
    let raw = card_value(cards, key).ok_or(WcsParseError::MissingKeyword(key))?;
    raw.parse::<f64>().map_err(|_| WcsParseError::NonNumeric {
        key,
        value: raw.to_string(),
    })
}

fn read_float_optional(cards: &[&[u8]], key: &'static str) -> Option<f64> {
    card_value(cards, key).and_then(|raw| raw.parse::<f64>().ok())
}

fn find_banner(cards: &[&[u8]]) -> Option<String> {
    for card in cards {
        if card.starts_with(b"COMMENT") || card.starts_with(b"HISTORY") {
            let text = std::str::from_utf8(&card[8..]).ok()?.trim();
            if text.to_ascii_uppercase().contains("ASTAP") {
                return Some(text.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic `.wcs` byte stream from an array of `(key, value)`
    /// pairs. Pads each card to 80 columns and appends an END card.
    fn build_wcs(pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (k, v) in pairs {
            let body = if v.starts_with('"') {
                format!("{k:<8}= {v}")
            } else {
                format!("{k:<8}= {v:>20}")
            };
            let padded = format!("{body:<80}");
            out.extend_from_slice(padded.as_bytes());
        }
        let end = format!("{:<80}", "END");
        out.extend_from_slice(end.as_bytes());
        out
    }

    #[test]
    fn happy_path_extracts_four_fields() {
        let bytes = build_wcs(&[
            ("CRVAL1", "10.6848"),
            ("CRVAL2", "41.2690"),
            ("CDELT1", "-0.000291667"),
            ("CDELT2", "0.000291667"),
            ("CROTA2", "12.3"),
        ]);
        let out = parse_wcs_bytes(&bytes).unwrap();
        assert!((out.ra_center - 10.6848).abs() < 1e-6);
        assert!((out.dec_center - 41.2690).abs() < 1e-6);
        // |CDELT1| * 3600 = pixel scale in arcsec
        assert!((out.pixel_scale_arcsec - 1.05).abs() < 1e-3);
        assert!((out.rotation_deg - 12.3).abs() < 1e-6);
    }

    #[test]
    fn missing_crval1_is_named_error() {
        let bytes = build_wcs(&[("CRVAL2", "41.2690"), ("CDELT1", "-0.000291667")]);
        let err = parse_wcs_bytes(&bytes).unwrap_err();
        match err {
            WcsParseError::MissingKeyword(k) => assert_eq!(k, "CRVAL1"),
            other => panic!("expected MissingKeyword, got {other:?}"),
        }
    }

    #[test]
    fn missing_crval2_is_named_error() {
        let bytes = build_wcs(&[("CRVAL1", "10.6848"), ("CDELT1", "-0.000291667")]);
        let err = parse_wcs_bytes(&bytes).unwrap_err();
        match err {
            WcsParseError::MissingKeyword(k) => assert_eq!(k, "CRVAL2"),
            other => panic!("expected MissingKeyword, got {other:?}"),
        }
    }

    #[test]
    fn missing_cdelt1_is_named_error() {
        let bytes = build_wcs(&[("CRVAL1", "10.6848"), ("CRVAL2", "41.2690")]);
        let err = parse_wcs_bytes(&bytes).unwrap_err();
        match err {
            WcsParseError::MissingKeyword(k) => assert_eq!(k, "CDELT1"),
            other => panic!("expected MissingKeyword, got {other:?}"),
        }
    }

    #[test]
    fn missing_crota2_defaults_to_zero() {
        let bytes = build_wcs(&[
            ("CRVAL1", "10.6848"),
            ("CRVAL2", "41.2690"),
            ("CDELT1", "-0.000291667"),
        ]);
        let out = parse_wcs_bytes(&bytes).unwrap();
        assert_eq!(out.rotation_deg, 0.0);
    }

    #[test]
    fn non_numeric_crval1_is_named_error() {
        let bytes = build_wcs(&[
            ("CRVAL1", "not-a-number"),
            ("CRVAL2", "41.2690"),
            ("CDELT1", "-0.000291667"),
        ]);
        let err = parse_wcs_bytes(&bytes).unwrap_err();
        match err {
            WcsParseError::NonNumeric { key, .. } => assert_eq!(key, "CRVAL1"),
            other => panic!("expected NonNumeric, got {other:?}"),
        }
    }

    #[test]
    fn extra_keys_are_ignored() {
        let bytes = build_wcs(&[
            ("SIMPLE", "T"),
            ("BITPIX", "16"),
            ("NAXIS", "0"),
            ("CRVAL1", "10.6848"),
            ("CRVAL2", "41.2690"),
            ("CDELT1", "-0.000291667"),
        ]);
        let out = parse_wcs_bytes(&bytes).unwrap();
        assert!((out.ra_center - 10.6848).abs() < 1e-6);
    }

    #[test]
    fn comment_card_with_astap_becomes_solver_banner() {
        let mut bytes = build_wcs(&[
            ("CRVAL1", "10.6848"),
            ("CRVAL2", "41.2690"),
            ("CDELT1", "-0.000291667"),
        ]);
        // Replace the END with a COMMENT then END.
        bytes.truncate(bytes.len() - 80);
        let comment = format!("{:<80}", "COMMENT ASTAP solver version CLI-2026.04.28");
        bytes.extend_from_slice(comment.as_bytes());
        let end = format!("{:<80}", "END");
        bytes.extend_from_slice(end.as_bytes());
        let out = parse_wcs_bytes(&bytes).unwrap();
        assert!(out.solver.contains("ASTAP"), "got banner: {}", out.solver);
    }

    #[test]
    fn pixel_scale_uses_absolute_value_of_cdelt1() {
        // ASTAP writes negative CDELT1 because RA decreases left-to-right.
        let bytes = build_wcs(&[
            ("CRVAL1", "10.6848"),
            ("CRVAL2", "41.2690"),
            ("CDELT1", "-0.000291667"),
        ]);
        let out = parse_wcs_bytes(&bytes).unwrap();
        assert!(out.pixel_scale_arcsec > 0.0);
    }
}
