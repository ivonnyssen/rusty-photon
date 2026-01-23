//! Tests for FITS file utilities

use base64::Engine;
use phd2_guider::{decode_base64_u16, write_grayscale_u16_fits};

#[test]
fn test_decode_base64_u16_valid() {
    // 4 pixels: 0x0001, 0x0002, 0x0003, 0x0004 in little-endian
    // Bytes: [0x01, 0x00, 0x02, 0x00, 0x03, 0x00, 0x04, 0x00]
    let bytes = vec![0x01, 0x00, 0x02, 0x00, 0x03, 0x00, 0x04, 0x00];
    let base64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);

    let pixels = decode_base64_u16(&base64_data).unwrap();

    assert_eq!(pixels, vec![1, 2, 3, 4]);
}

#[test]
fn test_decode_base64_u16_empty() {
    let base64_data = base64::engine::general_purpose::STANDARD.encode(&[]);

    let pixels = decode_base64_u16(&base64_data).unwrap();

    assert!(pixels.is_empty());
}

#[test]
fn test_decode_base64_u16_invalid_base64() {
    let result = decode_base64_u16("not valid base64!!!");

    assert!(result.is_err());
}

#[test]
fn test_decode_base64_u16_odd_bytes() {
    // 3 bytes - not a multiple of 2
    let bytes = vec![0x01, 0x00, 0x02];
    let base64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);

    let result = decode_base64_u16(&base64_data);

    assert!(result.is_err());
}

#[test]
fn test_decode_base64_u16_max_values() {
    // Test max u16 value: 0xFFFF
    let bytes = vec![0xFF, 0xFF, 0x00, 0x00];
    let base64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);

    let pixels = decode_base64_u16(&base64_data).unwrap();

    assert_eq!(pixels, vec![65535, 0]);
}

#[tokio::test]
async fn test_write_grayscale_u16_fits_dimension_mismatch() {
    let pixels = vec![1u16, 2, 3, 4];

    // 2x3 = 6 pixels expected, but we only have 4
    let result = write_grayscale_u16_fits("/tmp/test.fits", &pixels, 2, 3, None).await;

    assert!(result.is_err());
}
