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
    let base64_data = base64::engine::general_purpose::STANDARD.encode([]);

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

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri can't call fitsio FFI functions
async fn test_write_grayscale_u16_fits_success() {
    let pixels = vec![1u16, 2, 3, 4];
    let temp_file = std::env::temp_dir().join("test_fits_write.fits");

    // Clean up any existing file
    let _ = std::fs::remove_file(&temp_file);

    let result = write_grayscale_u16_fits(&temp_file, &pixels, 2, 2, None).await;
    assert!(result.is_ok(), "Failed to write FITS file: {:?}", result);

    // Verify file exists
    assert!(temp_file.exists());

    // Clean up
    let _ = std::fs::remove_file(&temp_file);
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri can't call fitsio FFI functions
async fn test_write_grayscale_u16_fits_with_headers() {
    let pixels = vec![100u16, 200, 300, 400, 500, 600, 700, 800, 900];
    let temp_file = std::env::temp_dir().join("test_fits_headers.fits");

    // Clean up any existing file
    let _ = std::fs::remove_file(&temp_file);

    let headers = vec![("TELESCOP", "Test Telescope"), ("OBSERVER", "Test User")];
    let result = write_grayscale_u16_fits(&temp_file, &pixels, 3, 3, Some(&headers)).await;
    assert!(result.is_ok(), "Failed to write FITS file: {:?}", result);

    // Verify file exists
    assert!(temp_file.exists());

    // Clean up
    let _ = std::fs::remove_file(&temp_file);
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri can't call fitsio FFI functions
async fn test_write_grayscale_u16_fits_single_pixel() {
    let pixels = vec![42u16];
    let temp_file = std::env::temp_dir().join("test_fits_single.fits");

    let _ = std::fs::remove_file(&temp_file);

    let result = write_grayscale_u16_fits(&temp_file, &pixels, 1, 1, None).await;
    assert!(result.is_ok());

    assert!(temp_file.exists());
    let _ = std::fs::remove_file(&temp_file);
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Miri can't call fitsio FFI functions
async fn test_write_grayscale_u16_fits_larger_image() {
    // 32x32 image
    let pixels: Vec<u16> = (0..1024).map(|i| i as u16).collect();
    let temp_file = std::env::temp_dir().join("test_fits_large.fits");

    let _ = std::fs::remove_file(&temp_file);

    let result = write_grayscale_u16_fits(&temp_file, &pixels, 32, 32, None).await;
    assert!(result.is_ok());

    assert!(temp_file.exists());
    let _ = std::fs::remove_file(&temp_file);
}

#[test]
fn test_decode_base64_u16_typical_star_image() {
    // Simulate a small star image with center brightness peak
    let pixels: Vec<u16> = vec![
        100, 100, 100, 100, 100, 200, 500, 200, 100, 500, 1000, 500, 100, 200, 500, 200, 100, 100,
        100, 100,
    ];

    // Convert to bytes (little-endian)
    let bytes: Vec<u8> = pixels.iter().flat_map(|&p| p.to_le_bytes()).collect();
    let base64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);

    let decoded = decode_base64_u16(&base64_data).unwrap();
    assert_eq!(decoded, pixels);
}

#[test]
fn test_decode_base64_u16_all_zeros() {
    let bytes = vec![0u8; 8]; // 4 pixels of value 0
    let base64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);

    let pixels = decode_base64_u16(&base64_data).unwrap();
    assert_eq!(pixels, vec![0, 0, 0, 0]);
}

#[test]
fn test_decode_base64_u16_alternating() {
    // Alternating 0 and 65535
    let bytes = vec![0x00, 0x00, 0xFF, 0xFF, 0x00, 0x00, 0xFF, 0xFF];
    let base64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);

    let pixels = decode_base64_u16(&base64_data).unwrap();
    assert_eq!(pixels, vec![0, 65535, 0, 65535]);
}
