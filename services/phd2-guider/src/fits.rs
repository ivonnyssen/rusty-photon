//! FITS file utilities for saving image data
//!
//! This module provides utility functions for working with FITS files,
//! including decoding base64 image data and writing FITS images.

use std::path::Path;

use base64::Engine;
use fitrs::{Fits, Hdu};
use tracing::debug;

use crate::error::{Phd2Error, Result};

/// Decode base64-encoded image data to u16 pixel values
///
/// PHD2 returns image data as base64-encoded bytes where each pixel
/// is a 16-bit unsigned integer in little-endian format.
///
/// # Arguments
/// * `base64_data` - The base64-encoded pixel data
///
/// # Returns
/// A vector of u16 pixel values
///
/// # Errors
/// Returns an error if the base64 data is invalid or the byte count
/// is not a multiple of 2.
pub fn decode_base64_u16(base64_data: &str) -> Result<Vec<u16>> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_data)
        .map_err(|e| Phd2Error::InvalidState(format!("Invalid base64 data: {}", e)))?;

    if bytes.len() % 2 != 0 {
        return Err(Phd2Error::InvalidState(format!(
            "Invalid pixel data: byte count {} is not a multiple of 2",
            bytes.len()
        )));
    }

    let pixels: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

    debug!("Decoded {} pixels from base64 data", pixels.len());
    Ok(pixels)
}

/// Write a 16-bit grayscale image to a FITS file
///
/// Creates a new FITS file with the given pixel data. The file will be
/// overwritten if it already exists.
///
/// Pixel values are stored as 32-bit integers (BITPIX=32) because the
/// `fitrs` crate does not support unsigned 16-bit. This is acceptable
/// for guide star thumbnails where file size is not a concern.
///
/// # Arguments
/// * `path` - Path where the FITS file will be written
/// * `pixels` - The pixel data as u16 values in row-major order
/// * `width` - Image width in pixels
/// * `height` - Image height in pixels
/// * `headers` - Optional additional FITS headers to include
///
/// # Errors
/// Returns an error if the file cannot be created or written.
///
/// # Example
/// ```ignore
/// let pixels = decode_base64_u16(&image.pixels)?;
/// write_grayscale_u16_fits(
///     "output.fits",
///     &pixels,
///     image.width,
///     image.height,
///     Some(&[("OBJECT", "Guide Star"), ("ORIGIN", "PHD2")]),
/// ).await?;
/// ```
pub async fn write_grayscale_u16_fits<P: AsRef<Path>>(
    path: P,
    pixels: &[u16],
    width: u32,
    height: u32,
    headers: Option<&[(&str, &str)]>,
) -> Result<()> {
    let expected_size = (width as usize) * (height as usize);
    if pixels.len() != expected_size {
        return Err(Phd2Error::InvalidState(format!(
            "Pixel count {} does not match dimensions {}x{} (expected {})",
            pixels.len(),
            width,
            height,
            expected_size
        )));
    }

    let path = path.as_ref().to_path_buf();
    let pixels = pixels.to_vec();
    let headers: Option<Vec<(String, String)>> = headers.map(|h| {
        h.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    });

    debug!(
        "Writing {}x{} FITS image to {}",
        width,
        height,
        path.display()
    );

    tokio::task::spawn_blocking(move || {
        write_fits_sync(&path, &pixels, width, height, headers.as_deref())
    })
    .await
    .map_err(|e| Phd2Error::InvalidState(format!("Task join error: {}", e)))?
}

/// Synchronous FITS writing implementation
fn write_fits_sync(
    path: &Path,
    pixels: &[u16],
    width: u32,
    height: u32,
    headers: Option<&[(String, String)]>,
) -> Result<()> {
    // Remove existing file if present (fitrs does not overwrite)
    if path.exists() {
        std::fs::remove_file(path).map_err(|e| {
            Phd2Error::InvalidState(format!("Failed to remove existing file: {}", e))
        })?;
    }

    // Convert u16 to i32 — fitrs does not support u16 directly.
    // FITS dimensions are [NAXIS1, NAXIS2] where NAXIS1 = width.
    let pixels_i32: Vec<i32> = pixels.iter().map(|&p| p as i32).collect();

    let mut hdu = Hdu::new(&[width as usize, height as usize], pixels_i32);

    if let Some(headers) = headers {
        for (key, value) in headers {
            hdu.insert(key.as_str(), value.as_str());
        }
    }

    Fits::create(path, hdu)
        .map_err(|e| Phd2Error::InvalidState(format!("Failed to create FITS file: {}", e)))?;

    debug!("FITS file written successfully to {}", path.display());
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    use base64::Engine;

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
            100, 100, 100, 100, 100, 200, 500, 200, 100, 500, 1000, 500, 100, 200, 500, 200, 100,
            100, 100, 100,
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
}
