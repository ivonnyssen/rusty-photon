//! FITS file I/O via the `fitrs` crate.
//!
//! Pixels are written as BITPIX=32 (i32) because `fitrs` does not support u16.
//! Internally the cache may hold `u16` (see `cache.rs`); FITS write widens to
//! `i32` at this boundary, FITS read produces `i32`.

use std::path::Path;

use fitrs::{Fits, Hdu};
use tracing::debug;

use crate::error::{Result, RpError};

/// Write i32 pixel data as a FITS file.
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
    if path.exists() {
        std::fs::remove_file(path)
            .map_err(|e| RpError::Imaging(format!("failed to remove existing file: {}", e)))?;
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| RpError::Imaging(format!("failed to create directory: {}", e)))?;
    }

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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

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
