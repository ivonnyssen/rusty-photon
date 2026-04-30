//! Exposure documents: the JSON record describing a captured image plus all
//! tool-produced sections written against it.
//!
//! `rp` owns the core document fields (id, captured_at, file_path, camera/
//! exposure metadata). Image-analysis tools and plugins contribute additional
//! data via named sections — see `docs/services/rp.md` (Exposure Document and
//! Plugin Sections).
//!
//! Persistence is a sidecar JSON file written next to each FITS file
//! (`<image>.json`). Writes are atomic: data is staged into a `.tmp` file and
//! `rename`d into place, so a crash mid-write cannot leave a torn document.
//!
//! As of Phase 7 (`docs/plans/image-evaluation-tools.md`), the document lives
//! inline on each `imaging::CachedImage` cache entry. Lookups go through
//! `ImageCache::get_document` and section updates through
//! `ImageCache::put_section`; the sidecar JSON pair on disk plus the disk-
//! fallback resolution path together provide the "live as long as the file
//! is on disk" contract.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{Result, RpError};

/// A captured exposure plus any sections tools have written against it.
///
/// `sections` is an open map keyed by tool/plugin name (`image_analysis`,
/// `flat_calibration`, `plate_solve`, etc.). Each section's shape is owned by
/// its writer; `rp` does not validate them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExposureDocument {
    pub id: String,
    /// RFC3339 timestamp of capture completion.
    pub captured_at: String,
    /// Absolute path to the FITS file on disk.
    pub file_path: String,
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "humantime_serde"
    )]
    pub duration: Option<Duration>,
    /// Camera's `MaxADU` at the time of capture. The sidecar carries it
    /// forward so a disk-fallback rehydration of the image cache can
    /// pick the correct `CachedPixels` variant without needing the
    /// originating camera to still be connected — see Phase 7
    /// (`docs/plans/image-evaluation-tools.md`) and the Image and
    /// Document Cache section of `docs/services/rp.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_adu: Option<u32>,
    #[serde(default)]
    pub sections: Map<String, Value>,
}

/// Sidecar JSON path for a given FITS file path (`/foo/<uuid8>.fits` →
/// `/foo/<uuid8>.json`).
pub fn sidecar_path(file_path: &str) -> PathBuf {
    let p = PathBuf::from(file_path);
    p.with_extension("json")
}

/// Read a sidecar JSON file from disk and deserialize it into an
/// `ExposureDocument`. Synchronous because sidecars are small (single-digit
/// KB even with measurement sections); the disk-fallback resolver runs the
/// whole scan on the blocking pool already.
pub fn read_sidecar_sync(path: &Path) -> Result<ExposureDocument> {
    let body = std::fs::read(path).map_err(|e| {
        RpError::Imaging(format!(
            "failed to read sidecar '{}': {}",
            path.display(),
            e
        ))
    })?;
    serde_json::from_slice(&body).map_err(|e| {
        RpError::Imaging(format!(
            "failed to parse sidecar '{}': {}",
            path.display(),
            e
        ))
    })
}

/// Atomically write `doc` to its sidecar JSON path
/// (`<doc.file_path>.with_extension("json")`).
///
/// Stages into a sibling temp file, fsyncs, renames into place, fsyncs the
/// parent directory (unix-only). Mirrors the FITS write treatment in
/// `imaging::fits::write_fits`.
pub async fn write_sidecar(doc: &ExposureDocument) -> Result<()> {
    let path = sidecar_path(&doc.file_path);
    write_sidecar_at(&path, doc).await
}

/// As `write_sidecar`, but writes to an explicit path. Used by tests that
/// want to verify sidecar I/O without round-tripping through the rest of
/// the pipeline.
pub async fn write_sidecar_at(path: &Path, doc: &ExposureDocument) -> Result<()> {
    let body = serde_json::to_vec_pretty(doc)?;
    let path = path.to_path_buf();
    // Run the whole stage-and-commit sequence on the blocking pool. Matches
    // the `imaging::fits::write_fits` pattern: one task spawn per write rather
    // than one per `tokio::fs::*` call, and lets us use sync-only crates like
    // `tempfile` for the staging file.
    tokio::task::spawn_blocking(move || write_sidecar_sync(&path, &body))
        .await
        .map_err(|e| RpError::Imaging(format!("sidecar write task join error: {e}")))?
}

fn write_sidecar_sync(final_path: &Path, body: &[u8]) -> Result<()> {
    let parent = final_path.parent().ok_or_else(|| {
        RpError::Imaging(format!(
            "sidecar path has no parent: {}",
            final_path.display()
        ))
    })?;
    std::fs::create_dir_all(parent)?;

    // `NamedTempFile::new_in(parent)` gives us an OS-generated unique name
    // (so two concurrent writers can't collide on the staging path) and a
    // `Drop` guard that removes the staging file on panic or early return.
    // `persist` disarms the guard on success.
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(body)?;
    // fsync the file data so a crash after rename cannot surface a renamed-
    // but-zero-length sidecar.
    tmp.as_file().sync_all()?;
    tmp.persist(final_path).map_err(|e| RpError::Io(e.error))?;
    // fsync the parent directory so the rename itself is durable. Windows
    // can't open a directory as a regular file handle, so this is unix-only.
    #[cfg(unix)]
    {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn doc_with_path(id: &str, file_path: &str) -> ExposureDocument {
        ExposureDocument {
            id: id.to_string(),
            captured_at: "2026-04-28T12:00:00Z".to_string(),
            file_path: file_path.to_string(),
            width: 16,
            height: 16,
            camera_id: Some("cam".to_string()),
            duration: Some(Duration::from_secs(1)),
            max_adu: Some(65535),
            sections: Map::new(),
        }
    }

    #[tokio::test]
    async fn write_sidecar_round_trips_through_json() {
        let dir = tempfile::tempdir().unwrap();
        let fits_path = dir.path().join("img.fits").to_string_lossy().into_owned();
        let sidecar = dir.path().join("img.json");

        let doc = doc_with_path("doc-1", &fits_path);
        write_sidecar(&doc).await.unwrap();

        let body = tokio::fs::read_to_string(&sidecar).await.unwrap();
        let parsed: ExposureDocument = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed.id, "doc-1");
        assert_eq!(parsed.file_path, fits_path);
        assert_eq!(parsed.max_adu, Some(65535));
    }

    #[tokio::test]
    async fn write_sidecar_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let fits_path = dir.path().join("img.fits").to_string_lossy().into_owned();
        let sidecar = dir.path().join("img.json");

        let mut doc = doc_with_path("doc-1", &fits_path);
        doc.duration = Some(Duration::from_secs(1));
        write_sidecar(&doc).await.unwrap();

        doc.duration = Some(Duration::from_secs(2));
        write_sidecar(&doc).await.unwrap();

        let body = tokio::fs::read_to_string(&sidecar).await.unwrap();
        let parsed: ExposureDocument = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed.duration, Some(Duration::from_secs(2)));
    }

    async fn entry_names(dir: &std::path::Path) -> Vec<String> {
        let mut entries = tokio::fs::read_dir(dir).await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        names
    }

    #[tokio::test]
    async fn successful_write_leaves_no_staging_files() {
        let dir = tempfile::tempdir().unwrap();
        let fits_path = dir.path().join("img.fits").to_string_lossy().into_owned();

        let doc = doc_with_path("doc-1", &fits_path);
        write_sidecar(&doc).await.unwrap();

        let names = entry_names(dir.path()).await;
        assert_eq!(
            names,
            vec!["img.json"],
            "directory should contain only the sidecar after a successful write"
        );
    }

    #[tokio::test]
    async fn failed_write_cleans_up_staging_file() {
        // Force the rename to fail by replacing the destination with a
        // directory — `rename(file, dir)` is rejected on Linux and Windows.
        let dir = tempfile::tempdir().unwrap();
        let fits_path = dir.path().join("img.fits").to_string_lossy().into_owned();
        let sidecar = dir.path().join("img.json");

        tokio::fs::create_dir(&sidecar).await.unwrap();

        let doc = doc_with_path("doc-1", &fits_path);
        let err = write_sidecar(&doc).await.unwrap_err();
        assert!(
            !err.to_string().is_empty(),
            "expected non-empty write failure error"
        );

        let names = entry_names(dir.path()).await;
        assert_eq!(
            names,
            vec!["img.json"],
            "failed write must not leave a staging file behind (only the directory we put in the way remains)"
        );
    }

    #[test]
    fn sidecar_path_swaps_extension() {
        assert_eq!(
            sidecar_path("/data/lights/550e8400.fits"),
            std::path::PathBuf::from("/data/lights/550e8400.json")
        );
    }

    #[test]
    fn read_sidecar_sync_errors_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does_not_exist.json");
        let err = read_sidecar_sync(&missing).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("failed to read sidecar"),
            "expected read-failure prefix, got: {msg}"
        );
        assert!(
            msg.contains(missing.to_string_lossy().as_ref()),
            "expected error to include the offending path, got: {msg}"
        );
    }

    #[test]
    fn read_sidecar_sync_errors_on_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("garbage.json");
        std::fs::write(&bad, b"{ not valid json").unwrap();
        let err = read_sidecar_sync(&bad).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("failed to parse sidecar"),
            "expected parse-failure prefix, got: {msg}"
        );
        assert!(
            msg.contains(bad.to_string_lossy().as_ref()),
            "expected error to include the offending path, got: {msg}"
        );
    }

    #[tokio::test]
    async fn write_sidecar_at_errors_when_path_has_no_parent() {
        // `Path::new("").parent()` is `None` cross-platform — exercises the
        // early-return guard in `write_sidecar_sync` before any filesystem
        // call is attempted.
        let doc = doc_with_path("doc-1", "/tmp/x.fits");
        let err = write_sidecar_at(Path::new(""), &doc).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("sidecar path has no parent"),
            "expected parent-missing message, got: {msg}"
        );
    }

    #[test]
    fn serialization_skips_none_max_adu() {
        let mut doc = doc_with_path("doc-1", "/tmp/x.fits");
        doc.max_adu = None;
        let body = serde_json::to_string(&doc).unwrap();
        assert!(
            !body.contains("max_adu"),
            "max_adu should be omitted when None, got: {}",
            body
        );
    }
}
