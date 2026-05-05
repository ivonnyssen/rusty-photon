//! ASTAP `.wcs` sidecar parser.
//!
//! ASTAP writes a `.wcs` sidecar next to each successfully solved FITS,
//! containing the World Coordinate System keywords as a FITS primary
//! HDU header (no data block — `NAXIS = 0` or all `NAXISn = 0`). Parsing
//! goes through [`fitsrs`] for the FITS-card layer and
//! [`wcs::WCSParams`] for the keyword-to-field mapping.
//!
//! The only divergence from a vanilla `Fits::from_reader` call is a
//! defensive pre-processor that pads short inputs out to the next
//! 2880-byte FITS block boundary. ASTAP's own output is already padded;
//! the pre-processor exists so test fixtures (and any future solver
//! emitting just enough cards to satisfy the contract) parse without
//! a separate code path.

use super::SolveOutcome;
use fitsrs::card::Card;
use fitsrs::hdu::HDU;
use fitsrs::Fits;
use serde::de::IntoDeserializer;
use serde::Deserialize;
use std::io::Cursor;
use std::path::Path;
use thiserror::Error;
use wcs::WCSParams;

const FITS_BLOCK: usize = 2880;

#[derive(Debug, Error)]
pub enum WcsParseError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("required keyword `{0}` not found in .wcs sidecar")]
    MissingKeyword(&'static str),

    #[error("`{key}` is non-numeric: {value}")]
    NonNumeric { key: &'static str, value: String },

    #[error("malformed FITS header in .wcs sidecar: {0}")]
    Malformed(String),
}

/// Parse a `.wcs` sidecar file and return the four fields the HTTP
/// contract surfaces, plus a solver banner string read from the file's
/// HISTORY / COMMENT cards.
pub fn read_wcs_sidecar(path: &Path) -> Result<SolveOutcome, WcsParseError> {
    let bytes = std::fs::read(path)?;
    parse_wcs_bytes(&bytes)
}

/// Pure-function variant for unit testing.
pub fn parse_wcs_bytes(bytes: &[u8]) -> Result<SolveOutcome, WcsParseError> {
    let normalized = pad_to_fits_block(bytes);
    let mut hdu_list = Fits::from_reader(Cursor::new(&*normalized));
    let hdu = hdu_list
        .next()
        .ok_or_else(|| WcsParseError::Malformed("empty header".into()))?
        .map_err(|e| WcsParseError::Malformed(format!("primary HDU parse failed: {e}")))?;

    let HDU::Primary(primary) = hdu else {
        return Err(WcsParseError::Malformed(
            "expected primary HDU in .wcs sidecar".into(),
        ));
    };
    let header = primary.get_header();

    let params = WCSParams::deserialize(header.into_deserializer())
        .map_err(|e| WcsParseError::Malformed(format!("WCS deserialization failed: {e}")))?;

    let crval1 = params
        .crval1
        .ok_or(WcsParseError::MissingKeyword("CRVAL1"))?;
    let crval2 = params
        .crval2
        .ok_or(WcsParseError::MissingKeyword("CRVAL2"))?;
    let cdelt1 = params
        .cdelt1
        .ok_or(WcsParseError::MissingKeyword("CDELT1"))?;
    let crota2 = params.crota2.unwrap_or(0.0);

    if !crval1.is_finite() {
        return Err(WcsParseError::NonNumeric {
            key: "CRVAL1",
            value: crval1.to_string(),
        });
    }
    if !crval2.is_finite() {
        return Err(WcsParseError::NonNumeric {
            key: "CRVAL2",
            value: crval2.to_string(),
        });
    }
    if !cdelt1.is_finite() {
        return Err(WcsParseError::NonNumeric {
            key: "CDELT1",
            value: cdelt1.to_string(),
        });
    }

    let solver = find_banner(header).unwrap_or_else(|| "astap-cli".to_string());

    Ok(SolveOutcome {
        ra_center: crval1,
        dec_center: crval2,
        pixel_scale_arcsec: cdelt1.abs() * 3600.0,
        rotation_deg: crota2,
        solver,
    })
}

/// Pad `bytes` (zero-copy when already aligned) to the next 2880-byte
/// FITS block boundary by appending ASCII spaces. Real ASTAP output is
/// already aligned; the pad keeps test fixtures and minimal sidecars
/// parseable without a divergent code path.
fn pad_to_fits_block(bytes: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let len = bytes.len();
    let remainder = len % FITS_BLOCK;
    if remainder == 0 && len > 0 {
        return std::borrow::Cow::Borrowed(bytes);
    }
    let target = if len == 0 {
        FITS_BLOCK
    } else {
        len + (FITS_BLOCK - remainder)
    };
    let mut padded = Vec::with_capacity(target);
    padded.extend_from_slice(bytes);
    padded.resize(target, b' ');
    std::borrow::Cow::Owned(padded)
}

/// Walk the header's cards looking for a HISTORY or COMMENT mentioning
/// "ASTAP". The banner is informational; the HTTP contract falls back
/// to a default when none is found.
fn find_banner<X>(header: &fitsrs::hdu::header::Header<X>) -> Option<String>
where
    X: fitsrs::hdu::header::Xtension + std::fmt::Debug,
{
    for card in header.cards() {
        let text = match card {
            Card::History(s) | Card::Comment(s) => s.as_str(),
            _ => continue,
        };
        if text.to_ascii_uppercase().contains("ASTAP") {
            return Some(text.trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a complete FITS primary HDU `.wcs` byte stream from a list
    /// of `(keyword, value)` pairs. Always prepends `SIMPLE = T`,
    /// `BITPIX = 16`, `NAXIS = 2`, `NAXIS1 = 1024`, `NAXIS2 = 1024`,
    /// `CTYPE1 = 'RA---TAN'`, `CTYPE2 = 'DEC--TAN'` so [`WCSParams`]'s
    /// mandatory fields are present. Caller's pairs append after the
    /// preamble; pad to a 2880-byte FITS block.
    fn build_wcs(pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut cards: Vec<String> = vec![
            card("SIMPLE", "T"),
            card("BITPIX", "16"),
            card("NAXIS", "2"),
            card("NAXIS1", "1024"),
            card("NAXIS2", "1024"),
            card("CTYPE1", "'RA---TAN'"),
            card("CTYPE2", "'DEC--TAN'"),
        ];
        for (k, v) in pairs {
            cards.push(card(k, v));
        }
        cards.push(format!("{:<80}", "END"));
        let mut out: Vec<u8> = cards.into_iter().flat_map(String::into_bytes).collect();
        let pad = FITS_BLOCK - (out.len() % FITS_BLOCK);
        if pad < FITS_BLOCK {
            out.resize(out.len() + pad, b' ');
        }
        out
    }

    fn card(key: &str, value: &str) -> String {
        let body = if value.starts_with('\'') {
            format!("{key:<8}= {value}")
        } else {
            format!("{key:<8}= {value:>20}")
        };
        format!("{body:<80}")
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
    fn comment_card_with_astap_becomes_solver_banner() {
        let bytes = build_wcs(&[
            ("CRVAL1", "10.6848"),
            ("CRVAL2", "41.2690"),
            ("CDELT1", "-0.000291667"),
            // COMMENT cards have a free-form value column; build_wcs's
            // generic shape works because fitsrs treats anything past
            // the 8-byte keyword column as the comment text.
        ]);
        // Splice a COMMENT card before END.
        let mut spliced = bytes;
        // Find END card by walking 80-byte cards.
        let end_idx = (0..spliced.len())
            .step_by(80)
            .find(|i| &spliced[*i..*i + 8] == b"END     ")
            .expect("END card not found");
        let comment = format!("{:<80}", "COMMENT ASTAP solver version CLI-2026.04.28");
        spliced.splice(end_idx..end_idx, comment.into_bytes());
        // Re-pad: splicing pushed the END further; ensure the buffer is
        // still a 2880 multiple by padding more spaces if needed.
        let pad = FITS_BLOCK - (spliced.len() % FITS_BLOCK);
        if pad < FITS_BLOCK {
            spliced.resize(spliced.len() + pad, b' ');
        }
        let out = parse_wcs_bytes(&spliced).unwrap();
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

    #[test]
    fn unpadded_input_is_padded_defensively() {
        // Build a header that ends right after the END card, with no
        // trailing 2880 padding. The pre-processor must pad it before
        // handing it to fitsrs.
        let cards = [
            card("SIMPLE", "T"),
            card("BITPIX", "16"),
            card("NAXIS", "2"),
            card("NAXIS1", "1024"),
            card("NAXIS2", "1024"),
            card("CTYPE1", "'RA---TAN'"),
            card("CTYPE2", "'DEC--TAN'"),
            card("CRVAL1", "10.6848"),
            card("CRVAL2", "41.2690"),
            card("CDELT1", "-0.000291667"),
            format!("{:<80}", "END"),
        ];
        let unpadded: Vec<u8> = cards.into_iter().flat_map(String::into_bytes).collect();
        assert_ne!(unpadded.len() % FITS_BLOCK, 0, "test setup precondition");
        let out = parse_wcs_bytes(&unpadded).unwrap();
        assert!((out.ra_center - 10.6848).abs() < 1e-6);
    }

    #[test]
    fn missing_simple_card_returns_malformed() {
        // Build a buffer that lacks SIMPLE = T at the top.
        let cards = [
            card("BITPIX", "16"),
            card("NAXIS", "0"),
            format!("{:<80}", "END"),
        ];
        let bytes: Vec<u8> = cards.into_iter().flat_map(String::into_bytes).collect();
        let err = parse_wcs_bytes(&bytes).unwrap_err();
        match err {
            WcsParseError::Malformed(_) => {}
            other => panic!("expected Malformed, got {other:?}"),
        }
    }
}
