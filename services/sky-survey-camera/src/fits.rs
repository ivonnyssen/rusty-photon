//! Minimal FITS primary-HDU reader for sky-survey-camera.
//!
//! Supports the subset of the FITS standard the SkyView backend
//! (and the stub server in BDD tests) actually emits: a single image
//! HDU with `BITPIX = 8 | 16 | 32 | -32 | -64`, `NAXIS = 2`, optional
//! `BSCALE` / `BZERO`. Output is normalised to `i32` ADU values, which
//! is what ASCOM `ImageArray` expects.
//!
//! Anything more exotic (multiple HDUs, table extensions, complex
//! number types) is out of scope for v0 — see `docs/services/
//! sky-survey-camera.md` *Future Work*.

use thiserror::Error;

const BLOCK_SIZE: usize = 2880;
const RECORD_SIZE: usize = 80;

#[derive(Debug, Error)]
pub enum FitsError {
    #[error("FITS payload too short")]
    TooShort,
    #[error("missing required FITS keyword: {0}")]
    MissingKeyword(&'static str),
    #[error("unsupported FITS keyword value: {0}={1}")]
    UnsupportedValue(&'static str, String),
    #[error("malformed FITS keyword line")]
    MalformedRecord,
}

#[derive(Debug, Clone)]
pub struct FitsImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<i32>,
}

/// Parse the primary HDU of a FITS payload into a `(width, height,
/// Vec<i32>)`. Reads `BITPIX` to size the data buffer, applies optional
/// `BSCALE` / `BZERO`, and saturates to `i32::MIN..=i32::MAX`.
pub fn parse_primary_hdu(bytes: &[u8]) -> Result<FitsImage, FitsError> {
    if bytes.len() < BLOCK_SIZE {
        return Err(FitsError::TooShort);
    }

    let mut header_end = None;
    let mut bitpix: Option<i32> = None;
    let mut naxis: Option<u32> = None;
    let mut naxis1: Option<u32> = None;
    let mut naxis2: Option<u32> = None;
    let mut bscale: f64 = 1.0;
    let mut bzero: f64 = 0.0;

    let mut offset = 0;
    'outer: while offset + BLOCK_SIZE <= bytes.len() {
        let block = &bytes[offset..offset + BLOCK_SIZE];
        for record_idx in 0..(BLOCK_SIZE / RECORD_SIZE) {
            let start = record_idx * RECORD_SIZE;
            let record = &block[start..start + RECORD_SIZE];
            let record_str = std::str::from_utf8(record).map_err(|_| FitsError::MalformedRecord)?;
            let trimmed = record_str.trim_end();
            if trimmed.starts_with("END") {
                header_end = Some(offset + BLOCK_SIZE);
                break 'outer;
            }
            if let Some((key, raw_value)) = split_keyword(record_str) {
                match key {
                    "BITPIX" => bitpix = Some(parse_int(raw_value, "BITPIX")?),
                    "NAXIS" => naxis = Some(parse_int(raw_value, "NAXIS")? as u32),
                    "NAXIS1" => naxis1 = Some(parse_int(raw_value, "NAXIS1")? as u32),
                    "NAXIS2" => naxis2 = Some(parse_int(raw_value, "NAXIS2")? as u32),
                    "BSCALE" => bscale = parse_float(raw_value),
                    "BZERO" => bzero = parse_float(raw_value),
                    _ => {}
                }
            }
        }
        offset += BLOCK_SIZE;
    }

    let header_end = header_end.ok_or(FitsError::MissingKeyword("END"))?;
    let bitpix = bitpix.ok_or(FitsError::MissingKeyword("BITPIX"))?;
    let naxis = naxis.ok_or(FitsError::MissingKeyword("NAXIS"))?;
    let width = naxis1.ok_or(FitsError::MissingKeyword("NAXIS1"))?;
    let height = naxis2.ok_or(FitsError::MissingKeyword("NAXIS2"))?;

    if naxis != 2 {
        return Err(FitsError::UnsupportedValue("NAXIS", naxis.to_string()));
    }

    let bytes_per_pixel: usize = match bitpix {
        8 | 16 | 32 => (bitpix.unsigned_abs() as usize) / 8,
        -32 | -64 => (bitpix.unsigned_abs() as usize) / 8,
        other => {
            return Err(FitsError::UnsupportedValue("BITPIX", other.to_string()));
        }
    };
    let pixels =
        (width as usize)
            .checked_mul(height as usize)
            .ok_or(FitsError::UnsupportedValue(
                "NAXIS1*NAXIS2",
                format!("{width}*{height}"),
            ))?;
    let needed = pixels
        .checked_mul(bytes_per_pixel)
        .ok_or(FitsError::UnsupportedValue(
            "data size",
            format!("{pixels}*{bytes_per_pixel}"),
        ))?;
    if bytes.len() < header_end + needed {
        return Err(FitsError::TooShort);
    }
    let data_bytes = &bytes[header_end..header_end + needed];

    let data: Vec<i32> = match bitpix {
        8 => data_bytes
            .iter()
            .map(|b| scale(*b as f64, bscale, bzero))
            .collect(),
        16 => data_bytes
            .chunks_exact(2)
            .map(|c| scale(i16::from_be_bytes([c[0], c[1]]) as f64, bscale, bzero))
            .collect(),
        32 => data_bytes
            .chunks_exact(4)
            .map(|c| {
                scale(
                    i32::from_be_bytes([c[0], c[1], c[2], c[3]]) as f64,
                    bscale,
                    bzero,
                )
            })
            .collect(),
        -32 => data_bytes
            .chunks_exact(4)
            .map(|c| {
                scale(
                    f32::from_be_bytes([c[0], c[1], c[2], c[3]]) as f64,
                    bscale,
                    bzero,
                )
            })
            .collect(),
        -64 => data_bytes
            .chunks_exact(8)
            .map(|c| {
                scale(
                    f64::from_be_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]),
                    bscale,
                    bzero,
                )
            })
            .collect(),
        _ => unreachable!(),
    };

    Ok(FitsImage {
        width,
        height,
        data,
    })
}

fn split_keyword(record: &str) -> Option<(&str, &str)> {
    let key_end = record.find('=')?;
    if key_end > 8 {
        return None;
    }
    let key = record[..key_end].trim();
    let after = &record[key_end + 1..];
    // Strip inline comment after `/`
    let value = match after.find('/') {
        Some(slash_pos) => &after[..slash_pos],
        None => after,
    };
    Some((key, value.trim()))
}

fn parse_int(raw: &str, name: &'static str) -> Result<i32, FitsError> {
    raw.trim()
        .parse::<i32>()
        .map_err(|_| FitsError::UnsupportedValue(name, raw.to_string()))
}

fn parse_float(raw: &str) -> f64 {
    raw.trim().parse::<f64>().unwrap_or(0.0)
}

fn scale(value: f64, bscale: f64, bzero: f64) -> i32 {
    let scaled = value * bscale + bzero;
    if scaled.is_nan() {
        0
    } else if scaled >= i32::MAX as f64 {
        i32::MAX
    } else if scaled <= i32::MIN as f64 {
        i32::MIN
    } else {
        scaled as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_minimal_fits(width: u32, height: u32, bitpix: i32) -> Vec<u8> {
        let mut header = String::new();
        let push = |h: &mut String, line: String| {
            let mut padded = format!("{line:<80}");
            padded.truncate(80);
            h.push_str(&padded);
        };
        push(&mut header, "SIMPLE  =                    T".to_string());
        push(&mut header, format!("BITPIX  = {bitpix:>20}"));
        push(&mut header, "NAXIS   =                    2".to_string());
        push(&mut header, format!("NAXIS1  = {width:>20}"));
        push(&mut header, format!("NAXIS2  = {height:>20}"));
        push(&mut header, "END".to_string());
        while !header.len().is_multiple_of(2880) {
            header.push(' ');
        }
        let mut bytes = header.into_bytes();
        let bpp = (bitpix.unsigned_abs() as usize) / 8;
        let data_len = (width as usize) * (height as usize) * bpp;
        bytes.extend(vec![0u8; data_len]);
        while !bytes.len().is_multiple_of(2880) {
            bytes.push(0);
        }
        bytes
    }

    #[test]
    fn parses_zeroed_bitpix32_image() {
        let bytes = build_minimal_fits(8, 4, 32);
        let img = parse_primary_hdu(&bytes).unwrap();
        assert_eq!(img.width, 8);
        assert_eq!(img.height, 4);
        assert_eq!(img.data.len(), 32);
        assert!(img.data.iter().all(|v| *v == 0));
    }

    #[test]
    fn parses_zeroed_bitpix16_image() {
        let bytes = build_minimal_fits(4, 4, 16);
        let img = parse_primary_hdu(&bytes).unwrap();
        assert_eq!(img.width, 4);
        assert_eq!(img.height, 4);
        assert_eq!(img.data.len(), 16);
    }

    #[test]
    fn rejects_truncated_payload() {
        let bytes = vec![0u8; 100];
        parse_primary_hdu(&bytes).unwrap_err();
    }

    #[test]
    fn rejects_non_fits_bytes() {
        let bytes = b"not a fits file at all -- random bytes here".repeat(100);
        parse_primary_hdu(&bytes).unwrap_err();
    }

    fn build_fits_with_data(width: u32, height: u32, bitpix: i32, data: Vec<u8>) -> Vec<u8> {
        let mut header = String::new();
        let push = |h: &mut String, line: String| {
            let mut padded = format!("{line:<80}");
            padded.truncate(80);
            h.push_str(&padded);
        };
        push(&mut header, "SIMPLE  =                    T".to_string());
        push(&mut header, format!("BITPIX  = {bitpix:>20}"));
        push(&mut header, "NAXIS   =                    2".to_string());
        push(&mut header, format!("NAXIS1  = {width:>20}"));
        push(&mut header, format!("NAXIS2  = {height:>20}"));
        push(&mut header, "END".to_string());
        while !header.len().is_multiple_of(2880) {
            header.push(' ');
        }
        let mut bytes = header.into_bytes();
        bytes.extend(data);
        while !bytes.len().is_multiple_of(2880) {
            bytes.push(0);
        }
        bytes
    }

    #[test]
    fn parses_bitpix32_signed_values() {
        // Two pixels: 7 and -3 in big-endian i32.
        let mut data = Vec::new();
        data.extend_from_slice(&7i32.to_be_bytes());
        data.extend_from_slice(&(-3i32).to_be_bytes());
        let bytes = build_fits_with_data(2, 1, 32, data);
        let img = parse_primary_hdu(&bytes).unwrap();
        assert_eq!(img.data, vec![7, -3]);
    }

    #[test]
    fn parses_bitpix_neg32_float_values() {
        // Two pixels: 1.5 and -2.25 in big-endian f32.
        let mut data = Vec::new();
        data.extend_from_slice(&1.5f32.to_be_bytes());
        data.extend_from_slice(&(-2.25f32).to_be_bytes());
        let bytes = build_fits_with_data(2, 1, -32, data);
        let img = parse_primary_hdu(&bytes).unwrap();
        // f32 → i32 saturating cast; small positive/negative values
        // round towards zero.
        assert_eq!(img.data, vec![1, -2]);
    }

    #[test]
    fn parses_bitpix_neg64_double_values() {
        let mut data = Vec::new();
        data.extend_from_slice(&100.5f64.to_be_bytes());
        data.extend_from_slice(&(-50.25f64).to_be_bytes());
        let bytes = build_fits_with_data(2, 1, -64, data);
        let img = parse_primary_hdu(&bytes).unwrap();
        assert_eq!(img.data, vec![100, -50]);
    }

    #[test]
    fn rejects_naxis_other_than_2() {
        let mut header = String::new();
        let push = |h: &mut String, line: String| {
            let mut padded = format!("{line:<80}");
            padded.truncate(80);
            h.push_str(&padded);
        };
        push(&mut header, "SIMPLE  =                    T".to_string());
        push(&mut header, "BITPIX  =                   32".to_string());
        push(&mut header, "NAXIS   =                    3".to_string());
        push(&mut header, "NAXIS1  =                    1".to_string());
        push(&mut header, "NAXIS2  =                    1".to_string());
        push(&mut header, "NAXIS3  =                    1".to_string());
        push(&mut header, "END".to_string());
        while !header.len().is_multiple_of(2880) {
            header.push(' ');
        }
        let mut bytes = header.into_bytes();
        bytes.extend(vec![0u8; 2880]);
        match parse_primary_hdu(&bytes).unwrap_err() {
            FitsError::UnsupportedValue("NAXIS", _) => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_bitpix() {
        let mut header = String::new();
        let push = |h: &mut String, line: String| {
            let mut padded = format!("{line:<80}");
            padded.truncate(80);
            h.push_str(&padded);
        };
        push(&mut header, "SIMPLE  =                    T".to_string());
        push(&mut header, "BITPIX  =                   24".to_string());
        push(&mut header, "NAXIS   =                    2".to_string());
        push(&mut header, "NAXIS1  =                    1".to_string());
        push(&mut header, "NAXIS2  =                    1".to_string());
        push(&mut header, "END".to_string());
        while !header.len().is_multiple_of(2880) {
            header.push(' ');
        }
        let mut bytes = header.into_bytes();
        bytes.extend(vec![0u8; 2880]);
        match parse_primary_hdu(&bytes).unwrap_err() {
            FitsError::UnsupportedValue("BITPIX", _) => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn scale_handles_extremes() {
        // Saturation: a value that overflows i32 from the upper side.
        assert_eq!(scale(1.0e20, 1.0, 0.0), i32::MAX);
        // Saturation: lower side.
        assert_eq!(scale(-1.0e20, 1.0, 0.0), i32::MIN);
        // NaN folds to zero.
        assert_eq!(scale(f64::NAN, 1.0, 0.0), 0);
        // BSCALE / BZERO are applied.
        assert_eq!(scale(10.0, 2.0, 5.0), 25);
    }
}
