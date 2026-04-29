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
use std::sync::{Arc, Mutex};

use ndarray::Array2;
use tracing::debug;

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
pub struct CachedImage {
    pub pixels: CachedPixels,
    pub width: u32,
    pub height: u32,
    pub fits_path: PathBuf,
    pub max_adu: u32,
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
        let nbytes = image.pixels.nbytes();
        let mut inner = self.inner.lock().expect("ImageCache mutex poisoned");

        if let Some(prev) = inner.images.remove(&document_id) {
            inner.bytes = inner.bytes.saturating_sub(prev.pixels.nbytes());
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
                inner.bytes = inner.bytes.saturating_sub(evicted.pixels.nbytes());
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

    fn u16_image(side: usize, fill: u16) -> CachedImage {
        let pixels = CachedPixels::U16(Array2::from_elem((side, side), fill));
        CachedImage {
            pixels,
            width: side as u32,
            height: side as u32,
            fits_path: PathBuf::from(format!("/tmp/{}.fits", side)),
            max_adu: 65535,
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
}
