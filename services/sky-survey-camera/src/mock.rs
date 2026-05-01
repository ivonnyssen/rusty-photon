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
pub fn synthetic_fits(width: u32, height: u32) -> Vec<u8> {
    const BLOCK: usize = 2880;
    const RECORD: usize = 80;

    fn pad_record(line: &str) -> String {
        let mut padded = format!("{line:<80}");
        padded.truncate(RECORD);
        padded
    }

    let mut header = String::new();
    header.push_str(&pad_record("SIMPLE  =                    T"));
    header.push_str(&pad_record("BITPIX  =                   16"));
    header.push_str(&pad_record("NAXIS   =                    2"));
    header.push_str(&pad_record(&format!("NAXIS1  = {width:>20}")));
    header.push_str(&pad_record(&format!("NAXIS2  = {height:>20}")));
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
}
