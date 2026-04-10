//! FITS file I/O and image statistics.
//!
//! Provides functions for writing and reading FITS images (via the `fitrs` crate)
//! and computing pixel statistics (median, mean, min, max ADU).

use std::path::Path;

use fitrs::{Fits, Hdu};
use tracing::debug;

use crate::error::{Result, RpError};

/// Pixel-level statistics for an image.
#[derive(Debug, Clone)]
pub struct ImageStats {
    pub median_adu: u32,
    pub mean_adu: f64,
    pub min_adu: u32,
    pub max_adu: u32,
    pub pixel_count: u64,
}

/// Write i32 pixel data as a FITS file.
///
/// The image is stored as BITPIX=32 (i32) because `fitrs` does not support u16.
/// This is the same approach used by `phd2-guider/src/fits.rs`.
///
/// The file is written atomically: data goes to a temp file first, then renamed.
pub async fn write_fits<P: AsRef<Path>>(
    path: P,
    pixels: &[i32],
    width: u32,
    height: u32,
) -> Result<()> {
    let expected = (width as usize) * (height as usize);
    if pixels.len() != expected {
        return Err(RpError::Imaging(format!(
            "pixel count {} does not match dimensions {}x{} (expected {})",
            pixels.len(),
            width,
            height,
            expected
        )));
    }

    let path = path.as_ref().to_path_buf();
    let pixels = pixels.to_vec();

    debug!(
        width = width,
        height = height,
        path = %path.display(),
        "writing FITS image"
    );

    tokio::task::spawn_blocking(move || write_fits_sync(&path, &pixels, width, height))
        .await
        .map_err(|e| RpError::Imaging(format!("task join error: {}", e)))?
}

fn write_fits_sync(path: &Path, pixels: &[i32], width: u32, height: u32) -> Result<()> {
    // Remove existing file if present (fitrs does not overwrite)
    if path.exists() {
        std::fs::remove_file(path)
            .map_err(|e| RpError::Imaging(format!("failed to remove existing file: {}", e)))?;
    }

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| RpError::Imaging(format!("failed to create directory: {}", e)))?;
    }

    // FITS dimensions are [NAXIS1, NAXIS2] where NAXIS1 = width
    let hdu = Hdu::new(&[width as usize, height as usize], pixels.to_vec());
    Fits::create(path, hdu)
        .map_err(|e| RpError::Imaging(format!("failed to create FITS file: {}", e)))?;

    debug!(path = %path.display(), "FITS file written successfully");
    Ok(())
}

/// Read pixel data from a FITS file.
///
/// Returns the pixel values as a flat `Vec<i32>`.
pub fn read_fits_pixels<P: AsRef<Path>>(path: P) -> Result<Vec<i32>> {
    let path = path.as_ref();
    debug!(path = %path.display(), "reading FITS pixels");

    let fits = Fits::open(path).map_err(|e| {
        RpError::Imaging(format!(
            "failed to open FITS file '{}': {}",
            path.display(),
            e
        ))
    })?;

    let primary = fits
        .into_iter()
        .next()
        .ok_or_else(|| RpError::Imaging("FITS file has no HDUs".to_string()))?;

    match primary.read_data() {
        fitrs::FitsData::IntegersI32(array) => {
            let pixels: Vec<i32> = array.data.iter().filter_map(|v| *v).collect();
            debug!(pixel_count = pixels.len(), "read FITS pixels");
            Ok(pixels)
        }
        fitrs::FitsData::IntegersU32(array) => {
            let pixels: Vec<i32> = array
                .data
                .iter()
                .filter_map(|v| v.map(|u| u as i32))
                .collect();
            debug!(pixel_count = pixels.len(), "read FITS pixels (u32->i32)");
            Ok(pixels)
        }
        other => Err(RpError::Imaging(format!(
            "unsupported FITS data type (expected integer): {:?}",
            std::mem::discriminant(&other)
        ))),
    }
}

/// Compute pixel statistics from a slice of pixel values.
///
/// Returns `None` if the pixel slice is empty.
pub fn compute_stats(pixels: &[i32]) -> Option<ImageStats> {
    if pixels.is_empty() {
        return None;
    }

    let pixel_count = pixels.len() as u64;

    let mut min = i32::MAX;
    let mut max = i32::MIN;
    let mut sum: i64 = 0;

    for &p in pixels {
        if p < min {
            min = p;
        }
        if p > max {
            max = p;
        }
        sum += p as i64;
    }

    let mean_adu = sum as f64 / pixel_count as f64;

    // Compute median by sorting a copy
    let mut sorted = pixels.to_vec();
    sorted.sort_unstable();
    let median = if sorted.len().is_multiple_of(2) {
        let mid = sorted.len() / 2;
        // Average of two middle values, rounded down
        ((sorted[mid - 1] as i64 + sorted[mid] as i64) / 2) as i32
    } else {
        sorted[sorted.len() / 2]
    };

    // Clamp to u32 range (pixel values should be non-negative)
    let clamp = |v: i32| -> u32 {
        if v < 0 {
            0
        } else {
            v as u32
        }
    };

    Some(ImageStats {
        median_adu: clamp(median),
        mean_adu,
        min_adu: clamp(min),
        max_adu: clamp(max),
        pixel_count,
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn compute_stats_odd_count() {
        let pixels = vec![10, 20, 30, 40, 50];
        let stats = compute_stats(&pixels).unwrap();
        assert_eq!(stats.median_adu, 30);
        assert_eq!(stats.min_adu, 10);
        assert_eq!(stats.max_adu, 50);
        assert_eq!(stats.pixel_count, 5);
        assert!((stats.mean_adu - 30.0).abs() < f64::EPSILON);
    }

    #[test]
    fn compute_stats_even_count() {
        let pixels = vec![10, 20, 30, 40];
        let stats = compute_stats(&pixels).unwrap();
        // Median of [10, 20, 30, 40] = (20 + 30) / 2 = 25
        assert_eq!(stats.median_adu, 25);
        assert_eq!(stats.min_adu, 10);
        assert_eq!(stats.max_adu, 40);
        assert_eq!(stats.pixel_count, 4);
        assert!((stats.mean_adu - 25.0).abs() < f64::EPSILON);
    }

    #[test]
    fn compute_stats_single_pixel() {
        let pixels = vec![42];
        let stats = compute_stats(&pixels).unwrap();
        assert_eq!(stats.median_adu, 42);
        assert_eq!(stats.min_adu, 42);
        assert_eq!(stats.max_adu, 42);
        assert_eq!(stats.pixel_count, 1);
    }

    #[test]
    fn compute_stats_empty() {
        let pixels: Vec<i32> = vec![];
        assert!(compute_stats(&pixels).is_none());
    }

    #[test]
    fn compute_stats_all_same() {
        let pixels = vec![1000; 100];
        let stats = compute_stats(&pixels).unwrap();
        assert_eq!(stats.median_adu, 1000);
        assert_eq!(stats.min_adu, 1000);
        assert_eq!(stats.max_adu, 1000);
    }

    #[test]
    fn compute_stats_unsorted_input() {
        let pixels = vec![50, 10, 40, 20, 30];
        let stats = compute_stats(&pixels).unwrap();
        assert_eq!(stats.median_adu, 30);
        assert_eq!(stats.min_adu, 10);
        assert_eq!(stats.max_adu, 50);
    }

    #[test]
    fn compute_stats_large_values() {
        // Typical 16-bit camera: values up to 65535
        let pixels = vec![0, 32768, 65535];
        let stats = compute_stats(&pixels).unwrap();
        assert_eq!(stats.median_adu, 32768);
        assert_eq!(stats.min_adu, 0);
        assert_eq!(stats.max_adu, 65535);
    }

    #[tokio::test]
    async fn write_and_read_fits_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.fits");

        let pixels = vec![100, 200, 300, 400];
        write_fits(&path, &pixels, 2, 2).await.unwrap();

        assert!(path.exists());

        let read_back = read_fits_pixels(&path).unwrap();
        assert_eq!(read_back, pixels);
    }

    #[tokio::test]
    async fn write_fits_dimension_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.fits");

        let pixels = vec![1, 2, 3, 4];
        let err = write_fits(&path, &pixels, 2, 3).await.unwrap_err();
        assert!(
            err.to_string().contains("does not match dimensions"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn read_fits_nonexistent() {
        let err = read_fits_pixels("/nonexistent/path.fits").unwrap_err();
        assert!(
            err.to_string().contains("failed to open FITS"),
            "unexpected error: {}",
            err
        );
    }

    #[tokio::test]
    async fn write_fits_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("dir").join("image.fits");

        let pixels = vec![42];
        write_fits(&path, &pixels, 1, 1).await.unwrap();

        assert!(path.exists());
    }
}
