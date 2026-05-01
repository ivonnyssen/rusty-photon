//! Thin reader facade over [`fitsrs`].
//!
//! Three entry points cover what the workspace's three FITS consumers
//! need:
//!
//! - [`read_primary`] returns the on-disk pixel type plus header
//!   metadata (`bscale`, `bzero`, `blank`). Callers that need exact
//!   unsigned-16 or other domain-specific scaling apply it themselves.
//! - [`read_primary_as_i32`] applies BSCALE/BZERO and saturates to
//!   `i32`, matching the `Vec<i32>` shape sky-survey-camera and rp's
//!   imaging pipeline expect.
//! - [`read_primary_keyword`] reads only the primary header — much
//!   cheaper than [`read_primary`] when the caller only needs one
//!   keyword (e.g. rp's `DOC_ID` lookup).
//!
//! BLANK handling: the raw integer sentinel value is surfaced via
//! [`FitsImage::blank`] and **not** filtered or replaced. Per
//! ADR-001 Amendment A, silently dropping pixels (the previous
//! `fitrs`-based path's behaviour) is a bug we're fixing here.

use std::fmt::Debug;
use std::io::{Read, Seek};

use fitsrs::card::Value as FitsValue;
use fitsrs::{Fits, Pixels as FitsPixels, HDU};

use crate::error::FitsError;
use crate::writer::KeywordValue;

/// Decoded primary HDU. `data` is the on-disk numeric type; consumers
/// that want a single typed `Vec<T>` should use [`read_primary_as_i32`]
/// or apply their own scaling.
#[derive(Debug, Clone)]
pub struct FitsImage {
    pub width: u32,
    pub height: u32,
    pub data: Pixels,
    /// FITS `BSCALE` (default 1.0). Multiplied into the raw pixel value.
    pub bscale: f64,
    /// FITS `BZERO` (default 0.0). Added to the BSCALE-multiplied pixel.
    pub bzero: f64,
    /// FITS `BLANK` sentinel for integer images, raw and unscaled.
    /// Surfaced as-is so callers can decide how to handle it.
    pub blank: Option<i64>,
}

#[derive(Debug, Clone)]
pub enum Pixels {
    U8(Vec<u8>),
    I16(Vec<i16>),
    I32(Vec<i32>),
    I64(Vec<i64>),
    F32(Vec<f32>),
    F64(Vec<f64>),
}

impl Pixels {
    pub fn len(&self) -> usize {
        match self {
            Pixels::U8(v) => v.len(),
            Pixels::I16(v) => v.len(),
            Pixels::I32(v) => v.len(),
            Pixels::I64(v) => v.len(),
            Pixels::F32(v) => v.len(),
            Pixels::F64(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Read the primary HDU of a FITS stream. Returns the on-disk pixel
/// data plus BSCALE/BZERO/BLANK metadata. The reader must support
/// seeking — `Cursor<&[u8]>` and `BufReader<File>` both qualify.
pub fn read_primary<R: Read + Seek + Debug>(reader: R) -> Result<FitsImage, FitsError> {
    let mut hdu_list = Fits::from_reader(reader);
    let hdu = hdu_list
        .next()
        .ok_or_else(|| FitsError::Parse("FITS stream contains no HDUs".into()))?
        .map_err(|e| FitsError::Parse(format!("primary HDU parse failed: {e}")))?;

    let HDU::Primary(image_hdu) = hdu else {
        return Err(FitsError::Unsupported(
            "first HDU is not a primary image HDU".into(),
        ));
    };

    let xtension = image_hdu.get_header().get_xtension();
    let naxis = xtension.get_naxis();
    if naxis.len() != 2 {
        return Err(FitsError::Unsupported(format!(
            "only 2-D images are supported (NAXIS = {})",
            naxis.len()
        )));
    }
    let width = u32::try_from(naxis[0])
        .map_err(|_| FitsError::Parse(format!("NAXIS1 out of range: {}", naxis[0])))?;
    let height = u32::try_from(naxis[1])
        .map_err(|_| FitsError::Parse(format!("NAXIS2 out of range: {}", naxis[1])))?;

    let bscale = read_float_keyword(image_hdu.get_header(), "BSCALE").unwrap_or(1.0);
    let bzero = read_float_keyword(image_hdu.get_header(), "BZERO").unwrap_or(0.0);
    let blank = read_int_keyword(image_hdu.get_header(), "BLANK");

    let image = hdu_list.get_data(&image_hdu);
    let data = match image.pixels() {
        FitsPixels::U8(it) => Pixels::U8(it.collect()),
        FitsPixels::I16(it) => Pixels::I16(it.collect()),
        FitsPixels::I32(it) => Pixels::I32(it.collect()),
        FitsPixels::I64(it) => Pixels::I64(it.collect()),
        FitsPixels::F32(it) => Pixels::F32(it.collect()),
        FitsPixels::F64(it) => Pixels::F64(it.collect()),
    };

    Ok(FitsImage {
        width,
        height,
        data,
        bscale,
        bzero,
        blank,
    })
}

/// Read the primary HDU and return scaled-to-`i32` pixels in row-major
/// order. Applies BSCALE/BZERO and saturates to `i32::MIN..=i32::MAX`,
/// matching the legacy sky-survey-camera and rp behaviour.
pub fn read_primary_as_i32<R: Read + Seek + Debug>(
    reader: R,
) -> Result<(Vec<i32>, u32, u32), FitsError> {
    let img = read_primary(reader)?;
    let scale = |v: f64| -> i32 {
        let scaled = v * img.bscale + img.bzero;
        if scaled.is_nan() {
            0
        } else if scaled >= i32::MAX as f64 {
            i32::MAX
        } else if scaled <= i32::MIN as f64 {
            i32::MIN
        } else {
            scaled as i32
        }
    };
    let pixels: Vec<i32> = match img.data {
        Pixels::U8(v) => v.into_iter().map(|p| scale(p as f64)).collect(),
        Pixels::I16(v) => v.into_iter().map(|p| scale(p as f64)).collect(),
        Pixels::I32(v) => v.into_iter().map(|p| scale(p as f64)).collect(),
        Pixels::I64(v) => v.into_iter().map(|p| scale(p as f64)).collect(),
        Pixels::F32(v) => v.into_iter().map(|p| scale(p as f64)).collect(),
        Pixels::F64(v) => v.into_iter().map(scale).collect(),
    };
    Ok((pixels, img.width, img.height))
}

/// Read a single keyword from the primary HDU's header. Cheaper than
/// [`read_primary`] when the caller only needs metadata (e.g. rp's
/// `DOC_ID` resolver). Returns `Ok(None)` when the keyword is absent.
pub fn read_primary_keyword<R: Read + Seek + Debug>(
    reader: R,
    key: &str,
) -> Result<Option<KeywordValue>, FitsError> {
    let mut hdu_list = Fits::from_reader(reader);
    let hdu = hdu_list
        .next()
        .ok_or_else(|| FitsError::Parse("FITS stream contains no HDUs".into()))?
        .map_err(|e| FitsError::Parse(format!("primary HDU parse failed: {e}")))?;
    let HDU::Primary(image_hdu) = hdu else {
        return Err(FitsError::Unsupported(
            "first HDU is not a primary image HDU".into(),
        ));
    };
    let upper = key.to_ascii_uppercase();
    let value = image_hdu.get_header().get(upper.as_str());
    match value {
        None => Ok(None),
        Some(FitsValue::Integer { value, .. }) => Ok(Some(KeywordValue::Int(*value))),
        Some(FitsValue::Float { value, .. }) => Ok(Some(KeywordValue::Float(*value))),
        Some(FitsValue::Logical { value, .. }) => Ok(Some(KeywordValue::Bool(*value))),
        Some(FitsValue::String { value, .. }) => {
            Ok(Some(KeywordValue::Str(value.trim_end().to_owned())))
        }
        Some(FitsValue::Undefined) => Ok(None),
        Some(FitsValue::Invalid(s)) => Err(FitsError::Parse(format!(
            "keyword {key} has invalid value: {s}"
        ))),
    }
}

fn read_float_keyword<X>(header: &fitsrs::hdu::header::Header<X>, key: &str) -> Option<f64> {
    match header.get(key)? {
        FitsValue::Float { value, .. } => Some(*value),
        FitsValue::Integer { value, .. } => Some(*value as f64),
        _ => None,
    }
}

fn read_int_keyword<X>(header: &fitsrs::hdu::header::Header<X>, key: &str) -> Option<i64> {
    match header.get(key)? {
        FitsValue::Integer { value, .. } => Some(*value),
        _ => None,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::io::Cursor;

    use crate::writer::{write_i32_image, write_u16_image, write_u8_image, Keyword};

    #[test]
    fn round_trip_i32() {
        let pixels = vec![100i32, -200, 0, 1_000_000];
        let mut buf = Vec::new();
        write_i32_image(&mut buf, &pixels, 2, 2, &[]).unwrap();
        let img = read_primary(Cursor::new(&buf[..])).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        match img.data {
            Pixels::I32(v) => assert_eq!(v, pixels),
            other => panic!("expected I32, got {other:?}"),
        }
        assert_eq!(img.bscale, 1.0);
        assert_eq!(img.bzero, 0.0);
        assert!(img.blank.is_none());
    }

    #[test]
    fn round_trip_u8() {
        let mut buf = Vec::new();
        write_u8_image(&mut buf, &[1u8, 2, 3, 4, 5, 6], 3, 2, &[]).unwrap();
        let img = read_primary(Cursor::new(&buf[..])).unwrap();
        assert_eq!((img.width, img.height), (3, 2));
        match img.data {
            Pixels::U8(v) => assert_eq!(v, vec![1, 2, 3, 4, 5, 6]),
            other => panic!("expected U8, got {other:?}"),
        }
    }

    #[test]
    fn round_trip_u16_via_bzero_metadata() {
        // `read_primary` returns the raw i16 + bscale/bzero; caller
        // applies the inversion. Confirms the metadata is surfaced.
        let pixels = vec![0u16, 32768, 65535, 12345];
        let mut buf = Vec::new();
        write_u16_image(&mut buf, &pixels, 2, 2, &[]).unwrap();
        let img = read_primary(Cursor::new(&buf[..])).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.bscale, 1.0);
        assert_eq!(img.bzero, 32768.0);
        let raw = match img.data {
            Pixels::I16(v) => v,
            other => panic!("expected I16, got {other:?}"),
        };
        let recovered: Vec<u16> = raw.iter().map(|r| (*r as i32 + 32768) as u16).collect();
        assert_eq!(recovered, pixels);
    }

    #[test]
    fn read_primary_as_i32_applies_scaling() {
        // u16 image written via the BZERO=32768 path. `read_primary_as_i32`
        // should hand back the *physical* values (0..=65535), not the raw
        // i16 storage values.
        let pixels = vec![0u16, 32768, 65535, 12345];
        let mut buf = Vec::new();
        write_u16_image(&mut buf, &pixels, 2, 2, &[]).unwrap();
        let (got, w, h) = read_primary_as_i32(Cursor::new(&buf[..])).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(got, vec![0i32, 32768, 65535, 12345]);
    }

    #[test]
    fn read_primary_as_i32_passes_through_i32() {
        let pixels = vec![1i32, -1, 1_000_000, i32::MIN, i32::MAX];
        let mut buf = Vec::new();
        write_i32_image(&mut buf, &pixels, 5, 1, &[]).unwrap();
        let (got, _, _) = read_primary_as_i32(Cursor::new(&buf[..])).unwrap();
        assert_eq!(got, pixels);
    }

    #[test]
    fn read_primary_keyword_returns_string() {
        let mut buf = Vec::new();
        let kw = vec![Keyword::new("DOC_ID", KeywordValue::Str("uuid-here".into())).unwrap()];
        write_i32_image(&mut buf, &[0i32; 4], 2, 2, &kw).unwrap();
        let v = read_primary_keyword(Cursor::new(&buf[..]), "DOC_ID").unwrap();
        match v {
            Some(KeywordValue::Str(s)) => assert_eq!(s, "uuid-here"),
            other => panic!("expected string DOC_ID, got {other:?}"),
        }
    }

    #[test]
    fn read_primary_keyword_returns_none_when_absent() {
        let mut buf = Vec::new();
        write_i32_image(&mut buf, &[0i32; 4], 2, 2, &[]).unwrap();
        let v = read_primary_keyword(Cursor::new(&buf[..]), "DOC_ID").unwrap();
        assert!(v.is_none());
    }

    #[test]
    fn read_primary_keyword_is_case_insensitive() {
        let mut buf = Vec::new();
        let kw = vec![Keyword::new("EXPTIME", KeywordValue::Float(3.5)).unwrap()];
        write_i32_image(&mut buf, &[0i32; 4], 2, 2, &kw).unwrap();
        let v = read_primary_keyword(Cursor::new(&buf[..]), "exptime")
            .unwrap()
            .unwrap();
        match v {
            KeywordValue::Float(f) => assert!((f - 3.5).abs() < 1e-9),
            other => panic!("expected float, got {other:?}"),
        }
    }

    #[test]
    fn read_primary_rejects_empty_stream() {
        let err = read_primary(Cursor::new(&b""[..])).unwrap_err();
        assert!(matches!(err, FitsError::Parse(_)));
    }
}
