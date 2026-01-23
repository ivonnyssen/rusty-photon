//! FITS file utilities for saving image data
//!
//! This module provides utility functions for working with FITS files,
//! including decoding base64 image data and writing FITS images.

use std::path::Path;

use base64::Engine;
use fitsio::images::{ImageDescription, ImageType};
use fitsio::FitsFile;
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
    // FITS uses FORTRAN-style column-major ordering, so dimensions are [NAXIS1, NAXIS2]
    // where NAXIS1 is the fastest-varying dimension (columns/width)
    let description = ImageDescription {
        data_type: ImageType::UnsignedShort,
        dimensions: &[width as usize, height as usize],
    };

    // Remove existing file if present (fitsio requires this)
    if path.exists() {
        std::fs::remove_file(path).map_err(|e| {
            Phd2Error::InvalidState(format!("Failed to remove existing file: {}", e))
        })?;
    }

    let mut fptr = FitsFile::create(path)
        .with_custom_primary(&description)
        .open()
        .map_err(|e| Phd2Error::InvalidState(format!("Failed to create FITS file: {}", e)))?;

    let hdu = fptr
        .primary_hdu()
        .map_err(|e| Phd2Error::InvalidState(format!("Failed to get primary HDU: {}", e)))?;

    // Convert u16 to i64 for fitsio (it expects signed values for write_image)
    // Actually, fitsio's write_image can handle u16 directly for UnsignedShort type
    // But we need to convert to the appropriate type
    let pixels_i32: Vec<i32> = pixels.iter().map(|&p| p as i32).collect();

    hdu.write_image(&mut fptr, &pixels_i32)
        .map_err(|e| Phd2Error::InvalidState(format!("Failed to write image data: {}", e)))?;

    // Write optional headers
    if let Some(headers) = headers {
        for (key, value) in headers {
            hdu.write_key(&mut fptr, key, value.as_str()).map_err(|e| {
                Phd2Error::InvalidState(format!("Failed to write header {}: {}", key, e))
            })?;
        }
    }

    debug!("FITS file written successfully to {}", path.display());
    Ok(())
}
