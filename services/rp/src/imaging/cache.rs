//! In-memory image cache.
//!
//! Holds the pixel buffer that `capture` already decoded so subsequent tools
//! (`measure_basic`, `auto_focus`, plugins) don't re-read and re-decode the
//! FITS file. The on-disk FITS file remains the durable source of truth — the
//! cache is strictly a hot-path optimization, with the file as fallback on
//! miss. See `docs/services/rp.md` (Image Cache) for the full design.
//!
//! Storage is `u16` for every consumer/prosumer astro camera (max_adu ≤
//! 65535); the `I32` variant is the hatch for future scientific cameras whose
//! `max_adu` exceeds 16-bit range.
//!
//! Eviction is LRU with two budgets — `cache_max_mib` and `cache_max_images`
//! — whichever trips first.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ndarray::Array2;
use tokio::sync::RwLock;
use tracing::debug;

use crate::document::ExposureDocument;

/// Pixel storage variant. The design intent is per-camera selection at
/// connect time (driven by the camera's `MaxADU`); the same camera always
/// reports the same `MaxADU`, so the variant is effectively per-camera. The
/// current implementation fetches `max_adu` per-frame in
/// `mcp.rs:capture` — see the "Phase 3 follow-up: stash `max_adu` on
/// `CameraEntry`" section in `docs/plans/image-evaluation-tools.md`.
pub enum CachedPixels {
    U16(Array2<u16>),
    I32(Array2<i32>),
}

/// Dispatch on a `&CachedPixels` and run the body once per pixel variant.
///
/// Generic image-analysis functions are statically dispatched, so the runtime
/// tag in `CachedPixels` has to be unwrapped somewhere. This macro hides the
/// two-arm match while still emitting a separate monomorphization per pixel
/// type — the body is textually duplicated by the macro expander, then the
/// compiler monomorphizes each arm with `T = u16` or `T = i32`.
///
/// Caveat: because the body is duplicated before expansion, any non-`Copy`
/// value moved by the body would only compile in one arm. Capture by reference
/// or by `Copy` value to keep both arms valid.
#[macro_export]
macro_rules! dispatch_pixels {
    ($pixels:expr, |$arr:ident| $body:expr) => {
        match $pixels {
            $crate::imaging::CachedPixels::U16(__a) => {
                let $arr = __a.view();
                $body
            }
            $crate::imaging::CachedPixels::I32(__a) => {
                let $arr = __a.view();
                $body
            }
        }
    };
}

impl CachedPixels {
    /// Memory footprint of this buffer, in bytes.
    pub fn nbytes(&self) -> usize {
        match self {
            CachedPixels::U16(a) => a.len() * std::mem::size_of::<u16>(),
            CachedPixels::I32(a) => a.len() * std::mem::size_of::<i32>(),
        }
    }
}

/// A cached image plus the metadata tools need to make sense of it.
///
/// The exposure document lives inline behind a per-entry async `RwLock` so
/// section updates (`measure_basic` writing `image_analysis`, plugins writing
/// their own sections) can mutate it without contending with any other entry.
/// `json_nbytes` is the *serialized* size of the document — the cache budget
/// includes it because `detect_stars` / `measure_stars` per-star arrays push
/// document JSON into the tens-of-KB range, which is non-negligible relative
/// to a small u16 thumbnail.
pub struct CachedImage {
    pub pixels: CachedPixels,
    pub width: u32,
    pub height: u32,
    pub fits_path: PathBuf,
    pub max_adu: u32,
    pub document: RwLock<ExposureDocument>,
    /// Bytes returned by `serde_json::to_vec(&document)` at the last update.
    /// Updated atomically by `put_section` (Step 4) so the cache mutex can
    /// read it during eviction without taking the per-entry lock.
    pub json_nbytes: AtomicUsize,
}

impl CachedImage {
    /// Construct an entry, computing `json_nbytes` from the document up
    /// front. If JSON serialization fails (which would also break sidecar
    /// writes), `json_nbytes` falls back to `0` — the cache will under-bill
    /// the entry rather than refuse to insert. Surface the error elsewhere
    /// (sidecar write) so the caller can decide.
    pub fn new(
        pixels: CachedPixels,
        width: u32,
        height: u32,
        fits_path: PathBuf,
        max_adu: u32,
        document: ExposureDocument,
    ) -> Self {
        let json_nbytes = serde_json::to_vec(&document).map(|v| v.len()).unwrap_or(0);
        Self {
            pixels,
            width,
            height,
            fits_path,
            max_adu,
            document: RwLock::new(document),
            json_nbytes: AtomicUsize::new(json_nbytes),
        }
    }

    /// Total memory footprint counted against the cache budget: pixel bytes
    /// plus the last-known serialized document size.
    pub fn nbytes(&self) -> usize {
        self.pixels.nbytes() + self.json_nbytes.load(Ordering::Relaxed)
    }
}

/// Process-wide image cache. Cheap to clone — internally `Arc<Mutex<…>>`.
#[derive(Clone)]
pub struct ImageCache {
    inner: Arc<Mutex<CacheInner>>,
    max_bytes: usize,
    max_images: usize,
}

struct CacheInner {
    images: HashMap<String, Arc<CachedImage>>,
    /// Recency: front = LRU (next to evict), back = most recently used.
    order: VecDeque<String>,
    bytes: usize,
}

impl ImageCache {
    /// `max_mib`: eviction budget in mebibytes. `max_images`: hard cap on
    /// entries — a safety net against pathological large-image streams.
    /// Whichever budget trips first triggers eviction.
    pub fn new(max_mib: usize, max_images: usize) -> Self {
        let max_bytes = max_mib.saturating_mul(1024 * 1024);
        debug!(
            max_mib = max_mib,
            max_images = max_images,
            "ImageCache constructed"
        );
        Self {
            inner: Arc::new(Mutex::new(CacheInner {
                images: HashMap::new(),
                order: VecDeque::new(),
                bytes: 0,
            })),
            max_bytes,
            max_images,
        }
    }

    /// Insert an image under `document_id`. Replaces any existing entry for
    /// that id. Evicts LRU entries until both budgets are satisfied.
    pub fn insert(&self, document_id: String, image: CachedImage) {
        let nbytes = image.nbytes();
        let mut inner = self.inner.lock().expect("ImageCache mutex poisoned");

        if let Some(prev) = inner.images.remove(&document_id) {
            inner.bytes = inner.bytes.saturating_sub(prev.nbytes());
            inner.order.retain(|k| k != &document_id);
        }

        inner.images.insert(document_id.clone(), Arc::new(image));
        inner.order.push_back(document_id.clone());
        inner.bytes += nbytes;

        debug!(
            document_id = %document_id,
            nbytes = nbytes,
            cache_bytes = inner.bytes,
            cache_count = inner.images.len(),
            "ImageCache inserted"
        );

        self.evict_locked(&mut inner);
    }

    /// Returns the cached image for `document_id` if present, marking it
    /// most-recently-used. `None` if not in cache — the caller is expected
    /// to fall back to reading the FITS file.
    pub fn get(&self, document_id: &str) -> Option<Arc<CachedImage>> {
        let mut inner = self.inner.lock().expect("ImageCache mutex poisoned");
        let image = inner.images.get(document_id).cloned()?;
        inner.order.retain(|k| k != document_id);
        inner.order.push_back(document_id.to_string());
        Some(image)
    }

    /// Clone the cached document out for callers that just want the JSON
    /// (HTTP `GET /api/documents/{id}`, the cache-miss measurement fallback).
    /// `None` when no entry is in the cache. Step 5 fills the on-disk
    /// fallback so this stops being the only resolution path.
    pub async fn get_document(&self, document_id: &str) -> Option<ExposureDocument> {
        let image = self.get(document_id)?;
        let cloned = image.document.read().await.clone();
        Some(cloned)
    }

    /// Write `value` into `sections[name]` on the cached document and
    /// persist the updated sidecar JSON atomically. The per-entry write
    /// lock is held across the sidecar write so concurrent updates
    /// serialize at the entry level.
    ///
    /// Failure semantics: when the sidecar write fails the in-memory
    /// section is rolled back to its prior value, mirroring the old
    /// `DocumentStore::put_section` contract. The cache budget is updated
    /// only after a successful write.
    pub async fn put_section(
        &self,
        document_id: &str,
        name: &str,
        value: serde_json::Value,
    ) -> crate::error::Result<()> {
        let image = self.get(document_id).ok_or_else(|| {
            crate::error::RpError::Imaging(format!("document not found: {}", document_id))
        })?;
        let mut doc = image.document.write().await;
        let prior = doc.sections.insert(name.to_string(), value);
        match crate::document::write_sidecar(&doc).await {
            Ok(()) => {
                let new_json_bytes = serde_json::to_vec(&*doc).map(|v| v.len()).unwrap_or(0);
                let old_json_bytes = image.json_nbytes.swap(new_json_bytes, Ordering::Relaxed);
                drop(doc);
                self.adjust_bytes(new_json_bytes as i64 - old_json_bytes as i64);
                debug!(
                    document_id = %document_id,
                    section = %name,
                    new_json_bytes,
                    old_json_bytes,
                    "ImageCache put_section"
                );
                Ok(())
            }
            Err(e) => {
                match prior {
                    Some(v) => {
                        doc.sections.insert(name.to_string(), v);
                    }
                    None => {
                        doc.sections.remove(name);
                    }
                }
                Err(e)
            }
        }
    }

    /// Apply a signed delta to the running cache `bytes` total under the
    /// cache mutex, then run eviction so the budget stays honored after
    /// document growth.
    fn adjust_bytes(&self, delta: i64) {
        let mut inner = self.inner.lock().expect("ImageCache mutex poisoned");
        if delta >= 0 {
            inner.bytes = inner.bytes.saturating_add(delta as usize);
        } else {
            inner.bytes = inner.bytes.saturating_sub((-delta) as usize);
        }
        self.evict_locked(&mut inner);
    }

    /// Number of entries currently in the cache.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("ImageCache mutex poisoned")
            .images
            .len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total bytes currently held.
    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.inner.lock().expect("ImageCache mutex poisoned").bytes
    }

    fn evict_locked(&self, inner: &mut CacheInner) {
        while (inner.bytes > self.max_bytes || inner.images.len() > self.max_images)
            && !inner.order.is_empty()
        {
            let key = inner.order.pop_front().expect("non-empty checked above");
            if let Some(evicted) = inner.images.remove(&key) {
                inner.bytes = inner.bytes.saturating_sub(evicted.nbytes());
                debug!(
                    document_id = %key,
                    cache_bytes = inner.bytes,
                    cache_count = inner.images.len(),
                    "ImageCache evicted"
                );
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use ndarray::Array2;
    use serde_json::Map;

    /// Minimal dummy document. Tests that exercise byte-budget accounting
    /// pair this with `json_nbytes = 0` so they stay focused on pixel
    /// bytes and don't depend on serde formatting choices.
    fn dummy_document(id: &str) -> ExposureDocument {
        ExposureDocument {
            id: id.to_string(),
            captured_at: "2026-04-30T00:00:00Z".to_string(),
            file_path: format!("/tmp/{}.fits", id),
            width: 0,
            height: 0,
            camera_id: None,
            duration: None,
            max_adu: None,
            sections: Map::new(),
        }
    }

    /// Pixel-only test fixture: builds a `CachedImage` with a dummy
    /// document but accounts zero JSON bytes, isolating byte-budget tests
    /// from the actual serialized doc size.
    fn u16_image(side: usize, fill: u16) -> CachedImage {
        let pixels = CachedPixels::U16(Array2::from_elem((side, side), fill));
        CachedImage {
            pixels,
            width: side as u32,
            height: side as u32,
            fits_path: PathBuf::from(format!("/tmp/{}.fits", side)),
            max_adu: 65535,
            document: RwLock::new(dummy_document("doc")),
            json_nbytes: AtomicUsize::new(0),
        }
    }

    fn i32_image(side: usize, fill: i32) -> CachedImage {
        let pixels = CachedPixels::I32(Array2::from_elem((side, side), fill));
        CachedImage {
            pixels,
            width: side as u32,
            height: side as u32,
            fits_path: PathBuf::from(format!("/tmp/{}.fits", side)),
            max_adu: 1 << 20,
            document: RwLock::new(dummy_document("doc")),
            json_nbytes: AtomicUsize::new(0),
        }
    }

    #[test]
    fn insert_and_get_round_trip() {
        let cache = ImageCache::new(100, 10);
        cache.insert("doc-1".to_string(), u16_image(4, 42));

        let got = cache.get("doc-1").unwrap();
        assert_eq!(got.width, 4);
        assert_eq!(got.height, 4);
        assert_eq!(got.max_adu, 65535);
        match &got.pixels {
            CachedPixels::U16(arr) => assert_eq!(arr[[0, 0]], 42),
            CachedPixels::I32(_) => panic!("expected u16 variant"),
        }
    }

    #[test]
    fn miss_returns_none() {
        let cache = ImageCache::new(100, 10);
        assert!(cache.get("nope").is_none());
    }

    #[test]
    fn is_empty_tracks_population() {
        let cache = ImageCache::new(100, 10);
        assert!(cache.is_empty());
        cache.insert("doc-1".to_string(), u16_image(4, 0));
        assert!(!cache.is_empty());
    }

    #[test]
    fn replacing_same_id_does_not_double_count_bytes() {
        let cache = ImageCache::new(100, 10);
        cache.insert("doc-1".to_string(), u16_image(4, 1));
        let bytes_after_first = cache.bytes();
        cache.insert("doc-1".to_string(), u16_image(4, 2));
        assert_eq!(cache.bytes(), bytes_after_first);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn evicts_when_image_count_exceeds_cap() {
        let cache = ImageCache::new(1024, 2);
        cache.insert("doc-1".to_string(), u16_image(2, 1));
        cache.insert("doc-2".to_string(), u16_image(2, 2));
        cache.insert("doc-3".to_string(), u16_image(2, 3));
        assert_eq!(cache.len(), 2);
        assert!(cache.get("doc-1").is_none());
        assert!(cache.get("doc-2").is_some());
        assert!(cache.get("doc-3").is_some());
    }

    #[test]
    fn evicts_when_byte_budget_exceeded() {
        // Budget: 1 MiB. Each 1024x1024 u16 = 2 MiB → only one fits.
        let cache = ImageCache::new(1, 100);
        cache.insert("doc-1".to_string(), u16_image(512, 1)); // 0.5 MiB
        cache.insert("doc-2".to_string(), u16_image(512, 2)); // 0.5 MiB total 1 MiB
        assert_eq!(cache.len(), 2);
        cache.insert("doc-3".to_string(), u16_image(512, 3)); // pushes over
        assert!(
            cache.get("doc-1").is_none(),
            "doc-1 should have been evicted"
        );
        assert!(cache.get("doc-2").is_some());
        assert!(cache.get("doc-3").is_some());
    }

    #[test]
    fn get_promotes_to_most_recently_used() {
        let cache = ImageCache::new(1024, 2);
        cache.insert("doc-1".to_string(), u16_image(2, 1));
        cache.insert("doc-2".to_string(), u16_image(2, 2));
        // Touch doc-1 to make it MRU.
        let _ = cache.get("doc-1");
        cache.insert("doc-3".to_string(), u16_image(2, 3));
        // doc-2 should now be the LRU and evicted.
        assert!(cache.get("doc-2").is_none());
        assert!(cache.get("doc-1").is_some());
        assert!(cache.get("doc-3").is_some());
    }

    #[test]
    fn i32_variant_round_trips() {
        let cache = ImageCache::new(100, 10);
        cache.insert("doc-i".to_string(), i32_image(4, 100_000));
        let got = cache.get("doc-i").unwrap();
        match &got.pixels {
            CachedPixels::I32(arr) => assert_eq!(arr[[0, 0]], 100_000),
            CachedPixels::U16(_) => panic!("expected i32 variant"),
        }
        assert_eq!(got.max_adu, 1 << 20);
    }

    #[test]
    fn nbytes_accounts_for_pixel_size() {
        let u16_img = u16_image(10, 0);
        let i32_img = i32_image(10, 0);
        assert_eq!(u16_img.pixels.nbytes(), 100 * 2);
        assert_eq!(i32_img.pixels.nbytes(), 100 * 4);
    }

    #[test]
    fn cached_image_new_includes_serialized_doc_in_nbytes() {
        // Pins the contract that `CachedImage::new` accounts for the
        // serialized document size. Step 4 will use this to keep the
        // running cache budget honest after `put_section` mutations.
        let pixels = CachedPixels::U16(Array2::from_elem((4, 4), 0u16));
        let pixel_bytes = pixels.nbytes();
        let doc = dummy_document("550e8400-e29b-41d4-a716-446655440000");
        let expected_json_bytes = serde_json::to_vec(&doc).unwrap().len();
        let img = CachedImage::new(pixels, 4, 4, PathBuf::from("/tmp/x.fits"), 65535, doc);
        assert_eq!(img.json_nbytes.load(Ordering::Relaxed), expected_json_bytes);
        assert_eq!(img.nbytes(), pixel_bytes + expected_json_bytes);
    }

    #[tokio::test]
    async fn cached_image_new_round_trips_document() {
        let doc = dummy_document("doc-1");
        let pixels = CachedPixels::U16(Array2::from_elem((2, 2), 0u16));
        let img = CachedImage::new(pixels, 2, 2, PathBuf::from("/tmp/x.fits"), 65535, doc);
        let read = img.document.read().await;
        assert_eq!(read.id, "doc-1");
    }
}
