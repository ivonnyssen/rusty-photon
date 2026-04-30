//! FITS file I/O via the `fitrs` crate.
//!
//! Pixels are written as BITPIX=32 (i32) because `fitrs` does not support u16.
//! Internally the cache may hold `u16` (see `cache.rs`); FITS write widens to
//! `i32` at this boundary, FITS read produces `i32`.
//!
//! Writes are atomic and durable: data is staged into a uniquely-named file in
//! the destination directory, fsynced, renamed into place, and the parent
//! directory is fsynced so the rename itself survives a crash. This mirrors
//! the sidecar JSON treatment in `document.rs`.

use std::path::Path;

use fitrs::{Fits, Hdu};
use tracing::debug;

use crate::error::{Result, RpError};

/// Write i32 pixel data as a FITS file.
///
/// Atomic: data is staged to a sibling temp file, fsynced, then renamed onto
/// `path`, which overwrites any existing destination. A crash mid-write
/// cannot leave a torn file at `path` — readers either see the old contents
/// or the new ones, never a partial mix. The staging file is removed by a
/// Drop guard if anything before the rename fails.
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

    // Run the whole stage-and-commit sequence on the blocking pool: fitrs and
    // tempfile are sync-only, and one task spawn per write is cheaper than
    // one per fs syscall.
    tokio::task::spawn_blocking(move || write_fits_sync(&path, &pixels, width, height))
        .await
        .map_err(|e| RpError::Imaging(format!("task join error: {}", e)))?
}

fn write_fits_sync(path: &Path, pixels: &[i32], width: u32, height: u32) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| RpError::Imaging(format!("FITS path has no parent: {}", path.display())))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| RpError::Imaging(format!("failed to create directory: {}", e)))?;

    // `NamedTempFile::new_in(parent)` reserves an OS-generated unique filename
    // in the destination directory; `into_temp_path()` drops the file handle
    // but keeps a Drop guard that removes the staging file on panic or early
    // return. `fitrs::Fits::create` errors if the file already exists, so we
    // remove the empty placeholder before handing it the path. The Drop guard
    // still fires correctly afterwards (its `remove_file` is silent on ENOENT).
    let tmp_path = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| RpError::Imaging(format!("failed to create staging file: {}", e)))?
        .into_temp_path();
    std::fs::remove_file(&tmp_path)
        .map_err(|e| RpError::Imaging(format!("failed to clear staging file: {}", e)))?;

    let hdu = Hdu::new(&[width as usize, height as usize], pixels.to_vec());
    Fits::create(&tmp_path, hdu)
        .map_err(|e| RpError::Imaging(format!("failed to create FITS file: {}", e)))?;

    // fsync the FITS bytes so a crash after rename cannot surface a renamed-
    // but-zero-length file.
    std::fs::File::open(&tmp_path)
        .and_then(|f| f.sync_all())
        .map_err(|e| RpError::Imaging(format!("failed to fsync staging file: {}", e)))?;

    // Atomic rename. Overwrites any existing destination on POSIX and on
    // Windows (Rust's `rename` uses MoveFileExW with MOVEFILE_REPLACE_EXISTING).
    // `persist` consumes the TempPath, disarming the Drop guard.
    tmp_path
        .persist(path)
        .map_err(|e| RpError::Imaging(format!("failed to persist FITS file: {}", e.error)))?;

    // fsync the parent directory entry so the rename itself is durable.
    // Windows can't open a directory as a regular file handle, so unix-only.
    #[cfg(unix)]
    {
        std::fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| RpError::Imaging(format!("failed to fsync parent directory: {}", e)))?;
    }

    debug!(path = %path.display(), "FITS file written successfully");
    Ok(())
}

/// Read pixel data from a FITS file.
///
/// Returns `(pixels, width, height)`. The pixel vector is flat row-major;
/// `width` is the first FITS axis, `height` the second.
pub fn read_fits_pixels<P: AsRef<Path>>(path: P) -> Result<(Vec<i32>, u32, u32)> {
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

    // `fitrs` always returns `IntegersI32` for any BITPIX=32 (and BITPIX=16)
    // file — it does not inspect BZERO/BSCALE. Files written as "u32"
    // (BITPIX=32 + BZERO=2147483648) would arrive here with values shifted by
    // −2³¹; we don't support that encoding. Other variants
    // (`Characters`, `FloatingPoint32/64`) are surfaced as an error.
    match primary.read_data() {
        fitrs::FitsData::IntegersI32(array) => {
            let pixels: Vec<i32> = array.data.iter().filter_map(|v| *v).collect();
            let (width, height) = dims_from_shape(&array.shape)?;
            debug!(
                pixel_count = pixels.len(),
                width, height, "read FITS pixels"
            );
            Ok((pixels, width, height))
        }
        other => Err(RpError::Imaging(format!(
            "unsupported FITS data type (expected integer): {:?}",
            std::mem::discriminant(&other)
        ))),
    }
}

fn dims_from_shape(shape: &[usize]) -> Result<(u32, u32)> {
    match shape {
        [w, h] => Ok((*w as u32, *h as u32)),
        [w, h, planes] if *planes == 1 => Ok((*w as u32, *h as u32)),
        other => Err(RpError::Imaging(format!(
            "unexpected FITS shape (expected 2D or 3D with 1 plane): {:?}",
            other
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

        let (read_back, w, h) = read_fits_pixels(&path).unwrap();
        assert_eq!(read_back, pixels);
        assert_eq!((w, h), (2, 2));
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

    #[test]
    fn dims_from_shape_2d() {
        assert_eq!(dims_from_shape(&[64, 48]).unwrap(), (64, 48));
    }

    #[test]
    fn dims_from_shape_3d_single_plane() {
        assert_eq!(dims_from_shape(&[64, 48, 1]).unwrap(), (64, 48));
    }

    #[test]
    fn dims_from_shape_rejects_multi_plane() {
        let err = dims_from_shape(&[64, 48, 3]).unwrap_err();
        assert!(
            err.to_string().contains("expected 2D or 3D with 1 plane"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn dims_from_shape_rejects_4d() {
        let err = dims_from_shape(&[64, 48, 1, 1]).unwrap_err();
        assert!(
            err.to_string().contains("expected 2D or 3D with 1 plane"),
            "unexpected error: {}",
            err
        );
    }

    fn entry_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn write_overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("img.fits");

        write_fits(&path, &[1, 2, 3, 4], 2, 2).await.unwrap();
        write_fits(&path, &[10, 20, 30, 40], 2, 2).await.unwrap();

        let (pixels, w, h) = read_fits_pixels(&path).unwrap();
        assert_eq!(pixels, vec![10, 20, 30, 40]);
        assert_eq!((w, h), (2, 2));
    }

    #[tokio::test]
    async fn successful_write_leaves_no_staging_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("img.fits");

        write_fits(&path, &[1, 2, 3, 4], 2, 2).await.unwrap();

        assert_eq!(
            entry_names(dir.path()),
            vec!["img.fits"],
            "directory should contain only the FITS file after a successful write"
        );
    }

    #[tokio::test]
    async fn failed_write_cleans_up_staging_file() {
        // Force the rename to fail by replacing the destination with a directory
        // — `rename(file, dir)` is rejected on Linux and Windows. Mirrors
        // document.rs's `failed_write_cleans_up_staging_file` test.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("img.fits");
        std::fs::create_dir(&path).unwrap();

        let err = write_fits(&path, &[1, 2, 3, 4], 2, 2).await.unwrap_err();
        assert!(
            err.to_string().contains("failed to persist FITS file"),
            "unexpected error: {}",
            err
        );

        assert_eq!(
            entry_names(dir.path()),
            vec!["img.fits"],
            "failed write must not leave a staging file behind (only the directory we put in the way remains)"
        );
    }

    /// After a successful first write, force a second write to the *same*
    /// path to fail (by removing write permission on the parent so staging
    /// file creation fails). Verify the original FITS contents are still
    /// intact — atomic rename means the destination is either the old file
    /// or the new file, never torn or missing.
    ///
    /// Unix-only because the chmod trick relies on POSIX-style write bits.
    #[cfg(unix)]
    #[tokio::test]
    async fn failed_write_preserves_prior_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("img.fits");

        write_fits(&path, &[1, 2, 3, 4], 2, 2).await.unwrap();

        let original_perms = std::fs::metadata(dir.path()).unwrap().permissions();
        let mut readonly = original_perms.clone();
        readonly.set_mode(0o555);
        std::fs::set_permissions(dir.path(), readonly).unwrap();

        let err = write_fits(&path, &[9, 9, 9, 9], 2, 2).await.unwrap_err();

        // Restore so tempdir can clean up regardless of assertion outcomes.
        std::fs::set_permissions(dir.path(), original_perms).unwrap();

        assert!(
            err.to_string().contains("failed to create staging file"),
            "unexpected error: {}",
            err
        );

        let (pixels, _, _) = read_fits_pixels(&path).unwrap();
        assert_eq!(
            pixels,
            vec![1, 2, 3, 4],
            "the original file must remain intact when a write to the same path fails"
        );
    }
}
