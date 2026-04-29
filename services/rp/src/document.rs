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
//! The in-memory store is the runtime source of truth. Reload-on-restart from
//! the sidecar JSON files is a follow-up (Phase 5); current scope (Phase 4)
//! only persists writes — readers go through the in-memory map.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::RwLock;
use tracing::debug;

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
    #[serde(default)]
    pub sections: Map<String, Value>,
}

/// Process-wide store of exposure documents. Cheap to clone — internally
/// `Arc<RwLock<HashMap>>`.
#[derive(Clone)]
pub struct DocumentStore {
    inner: Arc<RwLock<HashMap<String, ExposureDocument>>>,
}

impl DocumentStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Insert a freshly-captured document. Writes the sidecar JSON atomically.
    /// Replaces any existing entry for the same id.
    pub async fn create(&self, doc: ExposureDocument) -> Result<()> {
        let sidecar_path = sidecar_path(&doc.file_path);
        write_sidecar(&sidecar_path, &doc).await?;
        let id = doc.id.clone();
        self.inner.write().await.insert(id.clone(), doc);
        debug!(document_id = %id, "DocumentStore created");
        Ok(())
    }

    /// Look up a document by id. `None` if not present in the in-memory store.
    pub async fn get(&self, id: &str) -> Option<ExposureDocument> {
        self.inner.read().await.get(id).cloned()
    }

    /// Write `value` into `sections[name]` on the document. Persists the
    /// updated sidecar JSON atomically before committing the change to the
    /// in-memory store, so a sidecar write failure leaves both on-disk and
    /// in-memory state unchanged. Returns an error if the document is not in
    /// the store.
    ///
    /// Concurrent `put_section` calls are serialized by holding the store's
    /// write lock across the sidecar write — this prevents a slower writer
    /// from overwriting the sidecar with an older snapshot after a faster
    /// concurrent writer already persisted a newer one.
    pub async fn put_section(&self, id: &str, name: &str, value: Value) -> Result<()> {
        let mut guard = self.inner.write().await;
        let mut updated = guard
            .get(id)
            .ok_or_else(|| RpError::Imaging(format!("document not found: {}", id)))?
            .clone();
        updated.sections.insert(name.to_string(), value);
        let sidecar_path = sidecar_path(&updated.file_path);
        write_sidecar(&sidecar_path, &updated).await?;
        guard.insert(id.to_string(), updated);
        debug!(document_id = %id, section = %name, "DocumentStore put_section");
        Ok(())
    }
}

impl Default for DocumentStore {
    fn default() -> Self {
        Self::new()
    }
}

fn sidecar_path(file_path: &str) -> PathBuf {
    let p = PathBuf::from(file_path);
    p.with_extension("json")
}

async fn write_sidecar(path: &PathBuf, doc: &ExposureDocument) -> Result<()> {
    let body = serde_json::to_vec_pretty(doc)?;
    let tmp = path.with_extension("json.tmp");
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&tmp, &body).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use serde_json::json;

    fn doc_with_path(id: &str, file_path: &str) -> ExposureDocument {
        ExposureDocument {
            id: id.to_string(),
            captured_at: "2026-04-28T12:00:00Z".to_string(),
            file_path: file_path.to_string(),
            width: 16,
            height: 16,
            camera_id: Some("cam".to_string()),
            duration: Some(Duration::from_secs(1)),
            sections: Map::new(),
        }
    }

    #[tokio::test]
    async fn create_and_get_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let fits_path = dir.path().join("img.fits").to_string_lossy().into_owned();
        let store = DocumentStore::new();

        store
            .create(doc_with_path("doc-1", &fits_path))
            .await
            .unwrap();

        let got = store.get("doc-1").await.unwrap();
        assert_eq!(got.id, "doc-1");
        assert_eq!(got.file_path, fits_path);
        assert_eq!(got.width, 16);
        assert!(got.sections.is_empty());
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let store = DocumentStore::new();
        assert!(store.get("nope").await.is_none());
    }

    #[tokio::test]
    async fn create_writes_sidecar_json() {
        let dir = tempfile::tempdir().unwrap();
        let fits_path = dir.path().join("img.fits").to_string_lossy().into_owned();
        let sidecar = dir.path().join("img.json");
        let store = DocumentStore::new();

        store
            .create(doc_with_path("doc-1", &fits_path))
            .await
            .unwrap();

        assert!(
            sidecar.exists(),
            "sidecar JSON should exist at {:?}",
            sidecar
        );
        let body = tokio::fs::read_to_string(&sidecar).await.unwrap();
        let parsed: ExposureDocument = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed.id, "doc-1");
    }

    #[tokio::test]
    async fn put_section_persists_to_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let fits_path = dir.path().join("img.fits").to_string_lossy().into_owned();
        let sidecar = dir.path().join("img.json");
        let store = DocumentStore::new();

        store
            .create(doc_with_path("doc-1", &fits_path))
            .await
            .unwrap();
        store
            .put_section(
                "doc-1",
                "image_analysis",
                json!({"hfr": 2.5, "star_count": 7}),
            )
            .await
            .unwrap();

        let got = store.get("doc-1").await.unwrap();
        assert_eq!(got.sections["image_analysis"]["hfr"], 2.5);
        assert_eq!(got.sections["image_analysis"]["star_count"], 7);

        let body = tokio::fs::read_to_string(&sidecar).await.unwrap();
        let parsed: ExposureDocument = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed.sections["image_analysis"]["star_count"], 7);
    }

    #[tokio::test]
    async fn put_section_unknown_id_errors() {
        let store = DocumentStore::new();
        let err = store
            .put_section("missing", "image_analysis", json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("document not found"), "{}", err);
    }

    #[tokio::test]
    async fn put_section_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let fits_path = dir.path().join("img.fits").to_string_lossy().into_owned();
        let store = DocumentStore::new();

        store
            .create(doc_with_path("doc-1", &fits_path))
            .await
            .unwrap();
        store
            .put_section("doc-1", "image_analysis", json!({"v": 1}))
            .await
            .unwrap();
        store
            .put_section("doc-1", "image_analysis", json!({"v": 2}))
            .await
            .unwrap();

        let got = store.get("doc-1").await.unwrap();
        assert_eq!(got.sections["image_analysis"]["v"], 2);
    }

    #[tokio::test]
    async fn put_section_rolls_back_on_write_failure() {
        // If the sidecar write fails, neither in-memory state nor on-disk
        // state should reflect the failed update. We force a write failure
        // by replacing the sidecar file with a directory of the same name —
        // `rename(tmp_file, sidecar_dir)` is rejected on Linux and Windows.
        let dir = tempfile::tempdir().unwrap();
        let fits_path = dir.path().join("img.fits").to_string_lossy().into_owned();
        let sidecar = dir.path().join("img.json");
        let store = DocumentStore::new();

        store
            .create(doc_with_path("doc-1", &fits_path))
            .await
            .unwrap();
        store
            .put_section("doc-1", "image_analysis", json!({"v": 1}))
            .await
            .unwrap();

        tokio::fs::remove_file(&sidecar).await.unwrap();
        tokio::fs::create_dir(&sidecar).await.unwrap();

        let err = store
            .put_section("doc-1", "image_analysis", json!({"v": 2}))
            .await
            .unwrap_err();
        assert!(
            !err.to_string().is_empty(),
            "expected non-empty write failure error"
        );

        let got = store.get("doc-1").await.unwrap();
        assert_eq!(
            got.sections["image_analysis"]["v"], 1,
            "in-memory section must roll back to the previous value when the sidecar write fails"
        );
    }

    #[tokio::test]
    async fn create_replaces_same_id() {
        let dir = tempfile::tempdir().unwrap();
        let fits_path = dir.path().join("img.fits").to_string_lossy().into_owned();
        let store = DocumentStore::new();

        let mut first = doc_with_path("doc-1", &fits_path);
        first.duration = Some(Duration::from_secs(1));
        store.create(first).await.unwrap();

        let mut second = doc_with_path("doc-1", &fits_path);
        second.duration = Some(Duration::from_secs(2));
        store.create(second).await.unwrap();

        let got = store.get("doc-1").await.unwrap();
        assert_eq!(got.duration, Some(Duration::from_secs(2)));
    }
}
