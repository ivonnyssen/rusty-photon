//! Library-only test helpers exposed by the `mock` feature. The
//! production binary always uses [`crate::survey::SkyViewClient`] —
//! enabling `mock` does not change runtime behaviour.
//!
//! [`MockSurveyClient`] returns a deterministic, valid FITS payload
//! sized to the binned sensor of the request. [`synthetic_fits`] is
//! the same generator exposed as a free function so the ConformU
//! integration test's in-process HTTP stub can reuse it. The data is
//! a small ramp keyed on `(pixels_x, pixels_y)` so cache hits remain
//! useful and downstream image-processing tools see something more
//! interesting than a zero-filled buffer. No network I/O, no hidden
//! timeouts — `health_check` and `fetch` both complete in
//! microseconds.

use crate::survey::{SurveyClient, SurveyError, SurveyRequest};

/// Synthetic [`SurveyClient`] that fabricates FITS bytes locally.
#[derive(Debug, Default)]
pub struct MockSurveyClient;

impl MockSurveyClient {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl SurveyClient for MockSurveyClient {
    async fn health_check(&self) -> Result<(), SurveyError> {
        Ok(())
    }

    async fn fetch(&self, request: &SurveyRequest) -> Result<Vec<u8>, SurveyError> {
        Ok(synthetic_fits(request.pixels_x, request.pixels_y))
    }
}

/// Build a minimal, valid `BITPIX = 16` FITS payload of `width × height`
/// pixels. The pixel data is a deterministic ramp that wraps at the
/// 16-bit boundary so the output is reproducible across runs.
///
/// No WCS is written. Callers that need a `(CRVAL1, CRVAL2)`-bearing
/// FITS — e.g. tests that pair the cutout with a plate-solver mock
/// configured to round-trip the embedded CRVAL — should use
/// [`synthetic_fits_with_wcs`] instead.
pub fn synthetic_fits(width: u32, height: u32) -> Vec<u8> {
    build_synthetic_fits(width, height, None)
}

/// Like [`synthetic_fits`] but writes a minimal `RA---TAN` / `DEC--TAN`
/// WCS keyed on the supplied field center and pixel scale. The output
/// is still a deterministic 16-bit ramp; only the header gains
/// `CTYPE1/2`, `CRVAL1/2`, `CRPIX1/2`, `CDELT1/2`. Used by tests that
/// need the FITS to advertise its own pointing so a downstream
/// "plate-solver" can read it back via `CRVAL1/2`.
pub fn synthetic_fits_with_wcs(
    width: u32,
    height: u32,
    ra_center_deg: f64,
    dec_center_deg: f64,
    pixel_scale_arcsec: f64,
) -> Vec<u8> {
    build_synthetic_fits(
        width,
        height,
        Some(WcsHeader {
            ra_center_deg,
            dec_center_deg,
            pixel_scale_arcsec,
        }),
    )
}

struct WcsHeader {
    ra_center_deg: f64,
    dec_center_deg: f64,
    pixel_scale_arcsec: f64,
}

fn build_synthetic_fits(width: u32, height: u32, wcs: Option<WcsHeader>) -> Vec<u8> {
    const BLOCK: usize = 2880;
    const RECORD: usize = 80;

    fn pad_record(line: &str) -> String {
        let mut padded = format!("{line:<80}");
        padded.truncate(RECORD);
        padded
    }

    fn float_record(key: &str, value: f64) -> String {
        // FITS-style fixed-format float: 20-character right-justified.
        let body = format!("{key:<8}= {value:>20.10E}");
        pad_record(&body)
    }

    fn string_record(key: &str, value: &str) -> String {
        // FITS string values are quoted and the open-quote sits at
        // column 11 (per the standard's fixed-format rule).
        let body = format!("{key:<8}= '{value:<8}'");
        pad_record(&body)
    }

    let mut header = String::new();
    header.push_str(&pad_record("SIMPLE  =                    T"));
    header.push_str(&pad_record("BITPIX  =                   16"));
    header.push_str(&pad_record("NAXIS   =                    2"));
    header.push_str(&pad_record(&format!("NAXIS1  = {width:>20}")));
    header.push_str(&pad_record(&format!("NAXIS2  = {height:>20}")));
    if let Some(w) = &wcs {
        let crpix1 = (width as f64) / 2.0 + 0.5;
        let crpix2 = (height as f64) / 2.0 + 0.5;
        let cdelt = w.pixel_scale_arcsec / 3600.0;
        header.push_str(&string_record("CTYPE1", "RA---TAN"));
        header.push_str(&string_record("CTYPE2", "DEC--TAN"));
        header.push_str(&float_record("CRVAL1", w.ra_center_deg));
        header.push_str(&float_record("CRVAL2", w.dec_center_deg));
        header.push_str(&float_record("CRPIX1", crpix1));
        header.push_str(&float_record("CRPIX2", crpix2));
        // CDELT1 negative — east decreases with pixel x in the
        // standard sky orientation. CDELT2 positive.
        header.push_str(&float_record("CDELT1", -cdelt));
        header.push_str(&float_record("CDELT2", cdelt));
    }
    header.push_str(&pad_record("END"));
    while !header.len().is_multiple_of(BLOCK) {
        header.push(' ');
    }

    let pixels = (width as usize) * (height as usize);
    let mut bytes = header.into_bytes();
    bytes.reserve(pixels * 2);
    for i in 0..pixels {
        let v = (i & 0xffff) as i16;
        bytes.extend_from_slice(&v.to_be_bytes());
    }
    while !bytes.len().is_multiple_of(BLOCK) {
        bytes.push(0);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fits::parse_primary_hdu;

    #[tokio::test]
    async fn health_check_is_instant_ok() {
        let client = MockSurveyClient::new();
        client.health_check().await.unwrap();
    }

    #[tokio::test]
    async fn fetch_returns_parseable_fits_with_requested_dimensions() {
        let client = MockSurveyClient::new();
        let req = SurveyRequest {
            survey: "Mock".into(),
            ra_deg: 0.0,
            dec_deg: 0.0,
            rotation_deg: 0.0,
            pixels_x: 32,
            pixels_y: 16,
            size_x_deg: 0.1,
            size_y_deg: 0.05,
        };
        let bytes = client.fetch(&req).await.unwrap();
        let img = parse_primary_hdu(&bytes).unwrap();
        assert_eq!(img.width, 32);
        assert_eq!(img.height, 16);
        assert_eq!(img.data.len(), 32 * 16);
    }

    #[tokio::test]
    async fn fetch_is_deterministic() {
        let client = MockSurveyClient::new();
        let req = SurveyRequest {
            survey: "Mock".into(),
            ra_deg: 1.0,
            dec_deg: 2.0,
            rotation_deg: 0.0,
            pixels_x: 8,
            pixels_y: 8,
            size_x_deg: 0.01,
            size_y_deg: 0.01,
        };
        assert_eq!(
            client.fetch(&req).await.unwrap(),
            client.fetch(&req).await.unwrap()
        );
    }

    #[test]
    fn synthetic_fits_with_wcs_round_trips_crval() {
        use rp_fits::reader::read_primary_keyword;
        use rp_fits::writer::KeywordValue;

        let bytes = synthetic_fits_with_wcs(64, 48, 83.8221, -5.3911, 1.05);
        let img = parse_primary_hdu(&bytes).unwrap();
        assert_eq!(img.width, 64);
        assert_eq!(img.height, 48);

        let crval1 = read_primary_keyword(std::io::Cursor::new(&bytes), "CRVAL1")
            .unwrap()
            .expect("CRVAL1 must be present");
        let crval2 = read_primary_keyword(std::io::Cursor::new(&bytes), "CRVAL2")
            .unwrap()
            .expect("CRVAL2 must be present");
        match (crval1, crval2) {
            (KeywordValue::Float(ra), KeywordValue::Float(dec)) => {
                assert!((ra - 83.8221).abs() < 1e-6, "CRVAL1 = {ra}");
                assert!((dec - -5.3911).abs() < 1e-6, "CRVAL2 = {dec}");
            }
            other => panic!("expected float CRVAL1/2, got {other:?}"),
        }
    }
}
