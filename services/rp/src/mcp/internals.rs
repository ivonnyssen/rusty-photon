//! Cross-category helper methods on `McpHandler` that more than one
//! tool category needs, plus the small private types and free
//! functions they share. Kept in one file so changes that touch the
//! capture/measure pipeline land in one place.
//!
//! `pub(crate)` is the visibility we use for items called from sibling
//! `built_in/<category>.rs` files (e.g. `do_capture` is called from
//! both `built_in/camera.rs` and `built_in/auto_focus.rs`'s
//! `AutoFocusAdapter`). The `crate::mcp` module is private to the
//! crate, so `pub(crate)` does not widen the public API.

use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::camera::CameraState;
use tracing::debug;
use uuid::Uuid;

use crate::imaging::{self, BackgroundStats, DetectionParams, Star};
use crate::persistence::{self, CachedImage, CachedPixels, ExposureDocument};

use super::handler::McpHandler;

/// Backstop grace added to the requested exposure `duration` to bound
/// `do_capture`'s readout wait. An Alpaca camera can *fail* an exposure
/// — transition `CameraState` to `Error` and leave `ImageReady` false
/// (e.g. `sky-survey-camera` when its follow-mode mount read or survey
/// fetch times out) — or, more rarely, wedge in `Exposing`. The poll
/// loop treats `Error` as terminal; this grace caps the wait even when a
/// camera never reports either readiness or error. 120 s mirrors
/// `do_move_focuser_blocking`'s deadline and comfortably covers real
/// readout/download latency on top of the exposure itself.
const CAPTURE_READOUT_GRACE: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------------
// Private helper types shared across imaging tool bodies. All
// `pub(crate)` so individual category files can construct them.
// ---------------------------------------------------------------------------

/// `MeasureBasicParams` after schema-level optionals are validated by the
/// tool body. Pure data, no `Option`s — passed to the imaging composer.
pub(crate) struct ResolvedParams {
    pub(crate) threshold_sigma: f64,
    pub(crate) min_area: usize,
    pub(crate) max_area: usize,
}

/// `EstimateBackgroundParams` after sign/range validation. Same pattern as
/// `ResolvedParams`: schema-level optionals, validated in the tool body.
pub(crate) struct ResolvedClipParams {
    pub(crate) k: f64,
    pub(crate) max_iters: usize,
}

/// Background stats paired with the input pixel area (rows × cols). The
/// kernel's `BackgroundStats.n_pixels` is the *surviving* count after
/// sigma-clipping; `total_pixels` is what we report as `pixel_count` in
/// the tool's JSON contract — consistent with `measure_basic`.
pub(crate) struct BackgroundOutcome {
    pub(crate) stats: BackgroundStats,
    pub(crate) total_pixels: u64,
}

/// `DetectStarsParams` after schema-level optionals are validated by the
/// tool body. Pure data, no `Option`s — passed to the imaging composer.
pub(crate) struct ResolvedDetectParams {
    pub(crate) threshold_sigma: f64,
    pub(crate) min_area: usize,
    pub(crate) max_area: usize,
}

/// Detection outcome: the star list paired with the background stats used
/// to set the threshold. The tool's JSON contract surfaces both.
pub(crate) struct DetectStarsOutcome {
    pub(crate) stars: Vec<Star>,
    pub(crate) background: BackgroundStats,
}

/// `MeasureStarsParams` after schema-level optionals are validated by the
/// tool body.
pub(crate) struct ResolvedMeasureStarsParams {
    pub(crate) threshold_sigma: f64,
    pub(crate) min_area: usize,
    pub(crate) max_area: usize,
    pub(crate) stamp_half_size: usize,
}

// ---------------------------------------------------------------------------
// `McpHandler` helper-method impl. Methods are `pub(crate)` so they're
// callable from sibling category files.
// ---------------------------------------------------------------------------

impl McpHandler {
    pub(crate) async fn stats_via_document(
        &self,
        doc_id: &str,
    ) -> crate::error::Result<imaging::ImageStats> {
        if let Some(cached) = self.image_cache.resolve(doc_id).await {
            return crate::dispatch_pixels!(&cached.pixels, |arr| stats_outcome(arr));
        }

        debug!(document_id = %doc_id, "image cache miss, falling back to FITS");
        let doc = self
            .image_cache
            .resolve_document(doc_id)
            .await
            .ok_or_else(|| {
                crate::error::RpError::Imaging(format!("document not found: {}", doc_id))
            })?;
        self.stats_via_path(&doc.file_path).await
    }

    pub(crate) async fn stats_via_path(
        &self,
        path: &str,
    ) -> crate::error::Result<imaging::ImageStats> {
        let path_owned = path.to_string();
        tokio::task::spawn_blocking(move || {
            let (mut pixels, _w, _h) = persistence::read_fits_pixels(&path_owned)?;
            imaging::compute_stats(&mut pixels)
                .ok_or_else(|| crate::error::RpError::Imaging("image has no pixels".into()))
        })
        .await
        .map_err(|e| crate::error::RpError::Imaging(format!("task join error: {}", e)))?
    }

    pub(crate) async fn measure_via_document(
        &self,
        doc_id: &str,
        params: &ResolvedParams,
    ) -> crate::error::Result<imaging::MeasureBasicResult> {
        if let Some(cached) = self.image_cache.resolve(doc_id).await {
            let max_adu = Some(cached.max_adu);
            return crate::dispatch_pixels!(&cached.pixels, |arr| imaging::measure_basic(
                arr,
                params.threshold_sigma,
                params.min_area,
                params.max_area,
                max_adu,
            ));
        }

        debug!(document_id = %doc_id, "image cache miss, falling back to FITS");
        let doc = self
            .image_cache
            .resolve_document(doc_id)
            .await
            .ok_or_else(|| {
                crate::error::RpError::Imaging(format!("document not found: {}", doc_id))
            })?;
        // No camera context here, so we can't reliably know max_adu — pass None
        // (saturation flagging is best-effort; not a correctness issue).
        self.measure_via_path(&doc.file_path, params).await
    }

    pub(crate) async fn measure_via_path(
        &self,
        path: &str,
        params: &ResolvedParams,
    ) -> crate::error::Result<imaging::MeasureBasicResult> {
        let path_owned = path.to_string();
        let threshold = params.threshold_sigma;
        let min_a = params.min_area;
        let max_a = params.max_area;
        tokio::task::spawn_blocking(move || {
            let (pixels, width, height) = persistence::read_fits_pixels(&path_owned)?;
            let arr = ndarray::Array2::from_shape_vec((width as usize, height as usize), pixels)
                .map_err(|e| {
                    crate::error::RpError::Imaging(format!("FITS shape mismatch: {}", e))
                })?;
            imaging::measure_basic(arr.view(), threshold, min_a, max_a, None)
        })
        .await
        .map_err(|e| crate::error::RpError::Imaging(format!("task join error: {}", e)))?
    }

    pub(crate) async fn estimate_via_document(
        &self,
        doc_id: &str,
        params: &ResolvedClipParams,
    ) -> crate::error::Result<BackgroundOutcome> {
        if let Some(cached) = self.image_cache.resolve(doc_id).await {
            return crate::dispatch_pixels!(&cached.pixels, |arr| clip_outcome(arr, params));
        }

        debug!(document_id = %doc_id, "image cache miss, falling back to FITS");
        let doc = self
            .image_cache
            .resolve_document(doc_id)
            .await
            .ok_or_else(|| {
                crate::error::RpError::Imaging(format!("document not found: {}", doc_id))
            })?;
        self.estimate_via_path(&doc.file_path, params).await
    }

    pub(crate) async fn estimate_via_path(
        &self,
        path: &str,
        params: &ResolvedClipParams,
    ) -> crate::error::Result<BackgroundOutcome> {
        let path_owned = path.to_string();
        let k = params.k;
        let max_iters = params.max_iters;
        tokio::task::spawn_blocking(move || {
            let (pixels, width, height) = persistence::read_fits_pixels(&path_owned)?;
            let arr = ndarray::Array2::from_shape_vec((width as usize, height as usize), pixels)
                .map_err(|e| {
                    crate::error::RpError::Imaging(format!("FITS shape mismatch: {}", e))
                })?;
            clip_outcome(arr.view(), &ResolvedClipParams { k, max_iters })
        })
        .await
        .map_err(|e| crate::error::RpError::Imaging(format!("task join error: {}", e)))?
    }

    pub(crate) async fn detect_via_document(
        &self,
        doc_id: &str,
        params: &ResolvedDetectParams,
    ) -> crate::error::Result<DetectStarsOutcome> {
        if let Some(cached) = self.image_cache.resolve(doc_id).await {
            let max_adu = Some(cached.max_adu);
            return crate::dispatch_pixels!(&cached.pixels, |arr| detect_outcome(
                arr, params, max_adu
            ));
        }

        debug!(document_id = %doc_id, "image cache miss, falling back to FITS");
        let doc = self
            .image_cache
            .resolve_document(doc_id)
            .await
            .ok_or_else(|| {
                crate::error::RpError::Imaging(format!("document not found: {}", doc_id))
            })?;
        // No camera context here — pass max_adu = None (matches measure_basic).
        self.detect_via_path(&doc.file_path, params).await
    }

    pub(crate) async fn detect_via_path(
        &self,
        path: &str,
        params: &ResolvedDetectParams,
    ) -> crate::error::Result<DetectStarsOutcome> {
        let path_owned = path.to_string();
        let resolved = ResolvedDetectParams {
            threshold_sigma: params.threshold_sigma,
            min_area: params.min_area,
            max_area: params.max_area,
        };
        tokio::task::spawn_blocking(move || {
            let (pixels, width, height) = persistence::read_fits_pixels(&path_owned)?;
            let arr = ndarray::Array2::from_shape_vec((width as usize, height as usize), pixels)
                .map_err(|e| {
                    crate::error::RpError::Imaging(format!("FITS shape mismatch: {}", e))
                })?;
            detect_outcome(arr.view(), &resolved, None)
        })
        .await
        .map_err(|e| crate::error::RpError::Imaging(format!("task join error: {}", e)))?
    }

    pub(crate) async fn measure_stars_via_document(
        &self,
        doc_id: &str,
        params: &ResolvedMeasureStarsParams,
    ) -> crate::error::Result<imaging::MeasureStarsResult> {
        if let Some(cached) = self.image_cache.resolve(doc_id).await {
            let max_adu = Some(cached.max_adu);
            return crate::dispatch_pixels!(&cached.pixels, |arr| imaging::measure_stars(
                arr,
                params.threshold_sigma,
                params.min_area,
                params.max_area,
                max_adu,
                params.stamp_half_size,
            ));
        }

        debug!(document_id = %doc_id, "image cache miss, falling back to FITS");
        let doc = self
            .image_cache
            .resolve_document(doc_id)
            .await
            .ok_or_else(|| {
                crate::error::RpError::Imaging(format!("document not found: {}", doc_id))
            })?;
        self.measure_stars_via_path(&doc.file_path, params).await
    }

    pub(crate) async fn measure_stars_via_path(
        &self,
        path: &str,
        params: &ResolvedMeasureStarsParams,
    ) -> crate::error::Result<imaging::MeasureStarsResult> {
        let path_owned = path.to_string();
        let threshold = params.threshold_sigma;
        let min_a = params.min_area;
        let max_a = params.max_area;
        let stamp = params.stamp_half_size;
        tokio::task::spawn_blocking(move || {
            let (pixels, width, height) = persistence::read_fits_pixels(&path_owned)?;
            let arr = ndarray::Array2::from_shape_vec((width as usize, height as usize), pixels)
                .map_err(|e| {
                    crate::error::RpError::Imaging(format!("FITS shape mismatch: {}", e))
                })?;
            imaging::measure_stars(arr.view(), threshold, min_a, max_a, None, stamp)
        })
        .await
        .map_err(|e| crate::error::RpError::Imaging(format!("task join error: {}", e)))?
    }

    pub(crate) async fn snr_via_document(
        &self,
        doc_id: &str,
        params: &ResolvedDetectParams,
    ) -> crate::error::Result<imaging::SnrResult> {
        if let Some(cached) = self.image_cache.resolve(doc_id).await {
            let max_adu = Some(cached.max_adu);
            return crate::dispatch_pixels!(&cached.pixels, |arr| imaging::compute_snr(
                arr,
                params.threshold_sigma,
                params.min_area,
                params.max_area,
                max_adu,
            ));
        }

        debug!(document_id = %doc_id, "image cache miss, falling back to FITS");
        let doc = self
            .image_cache
            .resolve_document(doc_id)
            .await
            .ok_or_else(|| {
                crate::error::RpError::Imaging(format!("document not found: {}", doc_id))
            })?;
        self.snr_via_path(&doc.file_path, params).await
    }

    pub(crate) async fn snr_via_path(
        &self,
        path: &str,
        params: &ResolvedDetectParams,
    ) -> crate::error::Result<imaging::SnrResult> {
        let path_owned = path.to_string();
        let threshold = params.threshold_sigma;
        let min_a = params.min_area;
        let max_a = params.max_area;
        tokio::task::spawn_blocking(move || {
            let (pixels, width, height) = persistence::read_fits_pixels(&path_owned)?;
            let arr = ndarray::Array2::from_shape_vec((width as usize, height as usize), pixels)
                .map_err(|e| {
                    crate::error::RpError::Imaging(format!("FITS shape mismatch: {}", e))
                })?;
            imaging::compute_snr(arr.view(), threshold, min_a, max_a, None)
        })
        .await
        .map_err(|e| crate::error::RpError::Imaging(format!("task join error: {}", e)))?
    }

    /// Persist the document and (on success) populate the image cache.
    ///
    /// Sidecar failure contract: if `write_sidecar` fails the cache insert
    /// is skipped, a `document_persistence_failed` event is emitted, and
    /// the function returns. The FITS file remains on disk; the
    /// `document_id` is unreachable via cache or disk fallback (no
    /// sidecar) until callers fall back to the FITS path directly. See
    /// `docs/services/rp.md` → Capture Tool Details → Sidecar failure
    /// contract.
    pub(crate) async fn persist_capture_artifact(
        &self,
        doc: ExposureDocument,
        cached_pixels: Option<CachedPixels>,
        captured_max_adu: Option<u32>,
    ) {
        let document_id = doc.id.clone();
        let image_path = doc.file_path.clone();
        let width = doc.width;
        let height = doc.height;

        let document_persisted = match crate::persistence::write_sidecar(&doc).await {
            Ok(()) => true,
            Err(e) => {
                debug!(error = %e, "sidecar write failed, skipping cache insert");
                self.event_bus.emit(
                    "document_persistence_failed",
                    serde_json::json!({
                        "document_id": document_id,
                        "file_path": image_path,
                        "error": e.to_string(),
                    }),
                );
                false
            }
        };

        if document_persisted {
            if let (Some(max_adu), Some(cp)) = (captured_max_adu, cached_pixels) {
                self.image_cache.insert(
                    document_id.clone(),
                    CachedImage::new(
                        cp,
                        width,
                        height,
                        std::path::PathBuf::from(&image_path),
                        max_adu,
                        doc,
                    ),
                );
            }
        }
    }

    /// Run the full capture pipeline against the named camera and return
    /// `(image_path, document_id)`. Shared body of the `capture` MCP tool
    /// and the `auto_focus` compound tool's per-step capture call —
    /// both want the same exposure / FITS-write / cache-insert / event
    /// flow.
    pub(crate) async fn do_capture(
        &self,
        camera_id: &str,
        duration: Duration,
    ) -> std::result::Result<(String, String), String> {
        let cam_entry = self
            .equipment
            .find_camera(camera_id)
            .ok_or_else(|| format!("camera not found: {}", camera_id))?;
        let cam = cam_entry
            .device
            .as_ref()
            .cloned()
            .ok_or_else(|| format!("camera not connected: {}", camera_id))?;
        // Snapshot the configured focal length now — `cam_entry` is a
        // borrow off `self.equipment` and we want the f64 (Copy) without
        // hanging on to that borrow across the `await`s below.
        let focal_length_mm = cam_entry.config.focal_length_mm;

        let document_id = Uuid::new_v4().to_string();
        // The 8-char UUID suffix is the on-disk reverse-lookup key used by
        // the cache's disk-fallback resolution (see Phase 7 of
        // `docs/plans/archive/image-evaluation-tools.md` and `rp.md` Persistence).
        // Operator-controlled `file_naming_pattern` rendering is reserved
        // until a token resolver lands; for now capture writes
        // `<uuid8>.fits` regardless of any configured template.
        let uuid8 = &document_id[..8];
        let image_path = format!("{}/{}.fits", self.session_config.data_directory, uuid8);

        self.event_bus.emit(
            "exposure_started",
            serde_json::json!({
                "camera_id": camera_id,
                "duration": humantime::format_duration(duration).to_string(),
            }),
        );

        cam.start_exposure(duration, true)
            .await
            .map_err(|e| format!("failed to start exposure: {}", e))?;

        // Poll until the frame is ready — but a not-ready camera is not
        // necessarily still exposing. An Alpaca camera that *fails* an
        // exposure transitions to `CameraState::Error` and leaves
        // `ImageReady` false forever; polling `ImageReady` alone treats
        // that as "still exposing" and loops indefinitely. That is the
        // bug that ran CI's closed-loop centering BDD to GitHub's 6 h job
        // cap: `sky-survey-camera`'s follow-mode mount read timed out
        // under load, the exposure failed, and `do_capture` span here
        // forever. Treat `Error` as terminal (surfacing the camera's
        // stored reason via `image_array`), and cap the total wait with a
        // deadline as a backstop for a camera wedged in `Exposing`.
        let deadline = tokio::time::Instant::now() + duration + CAPTURE_READOUT_GRACE;
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            match cam.image_ready().await {
                Ok(true) => break,
                Ok(false) => {
                    // A transient `camera_state` read error is non-fatal —
                    // `ImageReady` stays the primary signal and the deadline
                    // below still bounds the wait.
                    if let Ok(CameraState::Error) = cam.camera_state().await {
                        let detail = cam
                            .image_array()
                            .await
                            .err()
                            .map(|e| e.to_string())
                            .unwrap_or_else(|| "camera reported error state".to_string());
                        return Err(format!("exposure failed: {}", detail));
                    }
                    if tokio::time::Instant::now() >= deadline {
                        return Err(format!(
                            "timeout waiting for image_ready after {:?}",
                            duration + CAPTURE_READOUT_GRACE
                        ));
                    }
                }
                Err(e) => return Err(format!("error checking image ready: {}", e)),
            }
        }

        let image_array = cam
            .image_array()
            .await
            .map_err(|e| format!("failed to download image array: {}", e))?;

        let (dim_x, dim_y, _planes) = image_array.dim();
        let width = dim_x as u32;
        let height = dim_y as u32;

        // Read max_adu *before* collecting pixels: it decides whether
        // we need a u16 or i32 buffer, so reading first lets us collect
        // straight into the destination type and avoid the wasted
        // i32→u16 round trip.
        //
        // max_adu feeds three consumers: on-disk FITS bit-depth, cache
        // variant, and the exposure document's `max_adu` field
        // (sidecar self-describing for rehydration/archival lineage).
        // A transient Alpaca failure here is localized — the next
        // capture re-reads independently. On failure we persist
        // `max_adu: None`, write the FITS as i32 (lossless fallback),
        // and skip the cache insert; the FITS file on disk plus the
        // sidecar remain the durable record.
        let captured_max_adu: Option<u32> = match cam.max_adu().await {
            Ok(v) => Some(v),
            Err(e) => {
                debug!(error = %e, "max_adu unavailable for this capture");
                None
            }
        };

        // Optical geometry for the sidecar's `optics` block. Combines the
        // operator-supplied focal length with raw Alpaca pixel-size and
        // sensor-dimension reads. Any missing piece (focal length not
        // configured, camera read failed) drops the whole block — see
        // `docs/services/rp.md` §"Core Fields". Failures are isolated to
        // this auxiliary metadata; capture itself proceeds.
        let optics = match focal_length_mm {
            Some(focal_length_mm) => {
                let pixel_size_x_um = match cam.pixel_size_x().await {
                    Ok(v) => Some(v),
                    Err(e) => {
                        debug!(error = %e, "pixel_size_x unavailable for this capture");
                        None
                    }
                };
                let pixel_size_y_um = match cam.pixel_size_y().await {
                    Ok(v) => Some(v),
                    Err(e) => {
                        debug!(error = %e, "pixel_size_y unavailable for this capture");
                        None
                    }
                };
                let sensor_width_px = match cam.camera_x_size().await {
                    Ok(v) => Some(v),
                    Err(e) => {
                        debug!(error = %e, "camera_x_size unavailable for this capture");
                        None
                    }
                };
                let sensor_height_px = match cam.camera_y_size().await {
                    Ok(v) => Some(v),
                    Err(e) => {
                        debug!(error = %e, "camera_y_size unavailable for this capture");
                        None
                    }
                };
                match (
                    pixel_size_x_um,
                    pixel_size_y_um,
                    sensor_width_px,
                    sensor_height_px,
                ) {
                    (Some(px), Some(py), Some(sw), Some(sh)) => {
                        let derived = persistence::Optics::from_camera_geometry(
                            focal_length_mm,
                            px,
                            py,
                            sw,
                            sh,
                        );
                        if derived.is_none() {
                            // All Alpaca reads succeeded but the derivation
                            // declined — typically a non-positive or
                            // wild-magnitude reading that would have
                            // overflowed the derived pixel scale / FOV.
                            // Surface enough to diagnose bad camera state
                            // or a misconfigured focal length.
                            debug!(
                                camera_id,
                                focal_length_mm,
                                pixel_size_x_um = px,
                                pixel_size_y_um = py,
                                sensor_width_px = sw,
                                sensor_height_px = sh,
                                "optics derivation declined; omitting block"
                            );
                        }
                        derived
                    }
                    _ => None,
                }
            }
            None => {
                debug!(
                    camera_id,
                    "focal_length_mm not configured; omitting optics block"
                );
                None
            }
        };

        // Dispatch on max_adu, collecting pixels directly into the
        // narrowest type each path needs and reusing the same buffer
        // for the cache insert.
        let shape = (width as usize, height as usize);
        let cached_pixels: Option<CachedPixels> = match captured_max_adu {
            Some(max_adu) if max_adu <= u16::MAX as u32 => {
                let max_adu_i32 = max_adu as i32;
                let u16_pixels: Vec<u16> = image_array
                    .iter()
                    .map(|&p| p.clamp(0, max_adu_i32) as u16)
                    .collect();
                drop(image_array);
                persistence::write_fits_u16(&image_path, &u16_pixels, width, height, &document_id)
                    .await
                    .map_err(|e| format!("failed to write FITS file: {}", e))?;
                CachedPixels::from_u16_pixels(u16_pixels, shape)
            }
            _ => {
                let i32_pixels: Vec<i32> = image_array.iter().copied().collect();
                drop(image_array);
                persistence::write_fits_i32(&image_path, &i32_pixels, width, height, &document_id)
                    .await
                    .map_err(|e| format!("failed to write FITS file: {}", e))?;
                captured_max_adu.and_then(|m| CachedPixels::from_i32_pixels(i32_pixels, shape, m))
            }
        };

        let doc = ExposureDocument {
            id: document_id.clone(),
            captured_at: chrono::Utc::now().to_rfc3339(),
            file_path: image_path.clone(),
            width,
            height,
            camera_id: Some(camera_id.to_string()),
            duration: Some(duration),
            max_adu: captured_max_adu,
            optics,
            sections: serde_json::Map::new(),
        };
        self.persist_capture_artifact(doc, cached_pixels, captured_max_adu)
            .await;

        self.event_bus.emit(
            "exposure_complete",
            serde_json::json!({
                "document_id": document_id,
                "file_path": image_path,
            }),
        );

        Ok((image_path, document_id))
    }

    /// Resolve a focuser, validate the requested `position` against the
    /// operator-supplied `min_position`/`max_position` bounds, issue the
    /// Alpaca move, poll `is_moving` until idle (with a 120 s deadline),
    /// and return the focuser's reported `position` after settling.
    ///
    /// This is the shared body of the `move_focuser` MCP tool and the
    /// `auto_focus` compound tool's per-step focuser drive — both want
    /// the same bounds-check + blocking-poll semantics.
    pub(crate) async fn do_move_focuser_blocking(
        &self,
        focuser_id: &str,
        position: i32,
    ) -> std::result::Result<i32, String> {
        let foc_entry = self
            .equipment
            .find_focuser(focuser_id)
            .ok_or_else(|| format!("focuser not found: {}", focuser_id))?;
        let foc = foc_entry
            .device
            .as_ref()
            .cloned()
            .ok_or_else(|| format!("focuser not connected: {}", focuser_id))?;

        if let Some(min) = foc_entry.config.min_position {
            if position < min {
                return Err(format!(
                    "position out of range: {} < min_position {}",
                    position, min
                ));
            }
        }
        if let Some(max) = foc_entry.config.max_position {
            if position > max {
                return Err(format!(
                    "position out of range: {} > max_position {}",
                    position, max
                ));
            }
        }

        debug!(focuser_id, position, "moving focuser");
        foc.move_(position)
            .await
            .map_err(|e| format!("failed to move focuser: {}", e))?;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            match foc.is_moving().await {
                Ok(false) => break,
                Ok(true) if tokio::time::Instant::now() < deadline => continue,
                Ok(true) => return Err("timeout waiting for focuser to settle".to_string()),
                Err(e) => return Err(format!("error polling focuser is_moving: {}", e)),
            }
        }

        foc.position()
            .await
            .map_err(|e| format!("failed to read focuser position: {}", e))
    }

    /// Resolve the singular mount, returning the entry + connected device
    /// or a string error matching the convention `resolve_device!` uses
    /// for `id`-keyed devices ("no mount configured" / "mount not
    /// connected"). Singular: no `id` parameter.
    pub(crate) fn resolve_mount(
        &self,
    ) -> std::result::Result<
        (
            &crate::equipment::MountEntry,
            Arc<dyn ascom_alpaca::api::Telescope>,
        ),
        String,
    > {
        let entry = self
            .equipment
            .find_mount()
            .ok_or_else(|| "no mount configured".to_string())?;
        let device = entry
            .device
            .as_ref()
            .cloned()
            .ok_or_else(|| "mount not connected".to_string())?;
        Ok((entry, device))
    }

    /// Resolve the mount, issue an async slew, poll `slewing()` until
    /// idle (with a 300 s deadline), sleep `settle_after`, then read
    /// the post-slew RA/Dec and return them.
    ///
    /// Best-effort `abort_slew()` on deadline expiry before returning
    /// the timeout error — mount runaways have higher blast radius
    /// than focuser runaways (cables, hard stops, sun in a flat
    /// workflow).
    ///
    /// Mirrors `do_move_focuser_blocking`'s shape; same pass-through
    /// error mapping. Does NOT touch `Tracking` (per `mount.feature`
    /// + ASCOM contract — Tracking must already be on for
    ///   `slew_to_coordinates_async`).
    pub(crate) async fn do_slew_blocking(
        &self,
        ra: f64,
        dec: f64,
        settle_after: Duration,
    ) -> std::result::Result<(f64, f64), String> {
        let (_entry, mount) = self.resolve_mount()?;

        debug!(ra, dec, "slewing mount");
        mount
            .slew_to_coordinates_async(ra, dec)
            .await
            .map_err(|e| format!("failed to slew: {}", e))?;

        match poll_slewing_until_idle(mount.as_ref()).await {
            Ok(()) => {}
            Err(PollIdleError::Timeout) => {
                // Best-effort abort; ignore the abort's own result and
                // surface the timeout error as the primary failure.
                let _ = mount.abort_slew().await;
                return Err("timeout waiting for mount to settle".to_string());
            }
            Err(PollIdleError::Read(e)) => {
                return Err(format!("error polling mount slewing: {}", e));
            }
        }

        if !settle_after.is_zero() {
            debug!(?settle_after, "waiting for mount settle");
            tokio::time::sleep(settle_after).await;
        }

        let actual_ra = mount
            .right_ascension()
            .await
            .map_err(|e| format!("failed to read mount right_ascension: {}", e))?;
        let actual_dec = mount
            .declination()
            .await
            .map_err(|e| format!("failed to read mount declination: {}", e))?;
        Ok((actual_ra, actual_dec))
    }

    /// Resolve the mount, issue `park()`, then poll `at_park()` every
    /// 100 ms until it returns `true` (300 s deadline).
    ///
    /// `AtPark` is the ASCOM-canonical "park is complete" signal — set
    /// in exactly one code path (the slew-to-park completion handler).
    /// Polling `Slewing` would be over-conservative: ASCOM's
    /// `IsSlewing` is sticky on `MoveAxis`-driven rate state and any
    /// non-idle `SlewState`, so unrelated prior activity can keep it
    /// `true` even after `ChangePark(true)` has fired.
    ///
    /// Unlike `do_slew_blocking`, this does NOT auto-abort on timeout
    /// — a partially-completed park is closer to safe than an
    /// aborted one (the mount is actively trying to reach a known
    /// safe position; aborting leaves it in an unknown state
    /// mid-traversal). Callers that want to interrupt a stuck park
    /// can call the `abort_slew` MCP tool explicitly.
    ///
    /// Per ASCOM, a successful `park()` clears `Tracking`. We don't
    /// touch tracking ourselves; the contract is the driver's.
    pub(crate) async fn do_park_blocking(&self) -> std::result::Result<(), String> {
        let (_entry, mount) = self.resolve_mount()?;

        debug!("parking mount");
        mount
            .park()
            .await
            .map_err(|e| format!("failed to park: {}", e))?;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
        loop {
            match mount.at_park().await {
                Ok(true) => return Ok(()),
                Ok(false) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Ok(false) => return Err("timeout waiting for mount to park".to_string()),
                Err(e) => return Err(format!("error polling mount at_park: {}", e)),
            }
        }
    }

    /// Resolve the mount and issue a sync to the given equatorial
    /// coordinates (RA hours, Dec degrees). No polling — `sync` is
    /// immediate per ASCOM. Mirrors the shape of `do_slew_blocking`
    /// minus the polling loop. Used by both the primitive
    /// `sync_mount` MCP tool and the `center_on_target` compound
    /// tool's per-iteration sync; one helper, one place to change
    /// the error-mapping convention.
    pub(crate) async fn do_sync_mount(
        &self,
        ra_hours: f64,
        dec_deg: f64,
    ) -> std::result::Result<(), String> {
        let (_entry, mount) = self.resolve_mount()?;
        debug!(ra = ra_hours, dec = dec_deg, "syncing mount");
        mount
            .sync_to_coordinates(ra_hours, dec_deg)
            .await
            .map_err(|e| format!("failed to sync mount: {}", e))
    }

    /// Read the current mount pointing for `plate_solve`'s
    /// `use_mount_hints` convenience. Converts Alpaca's decimal
    /// hours `RightAscension` to the wrapper's degrees-on-the-wire
    /// contract (`× 15`); `Declination` passes through. Failure
    /// modes — no mount configured, mount not connected, Alpaca
    /// read error — surface as a single string the caller appends
    /// to its diagnostic.
    pub(crate) async fn read_mount_hints_for_plate_solve(&self) -> Result<(f64, f64), String> {
        let (_entry, mount) = self.resolve_mount()?;
        let ra_hours = mount
            .right_ascension()
            .await
            .map_err(|e| format!("failed to read mount right_ascension: {e}"))?;
        let dec_deg = mount
            .declination()
            .await
            .map_err(|e| format!("failed to read mount declination: {e}"))?;
        Ok((ra_hours * 15.0, dec_deg))
    }

    /// Shared body of the standalone `plate_solve` MCP tool *and* the
    /// `center_on_target` compound tool's per-iteration solve. Both
    /// callers want the same configured-check, document resolution,
    /// hint sourcing, request build, error mapping, and `wcs`
    /// persistence — extracting them here keeps any future change to
    /// defaults / validation / persistence in exactly one place.
    ///
    /// Caller responsibilities:
    /// - Standalone `plate_solve` validates "neither `document_id`
    ///   nor `image_path` supplied" itself (so the error message
    ///   shape matches what its BDD pins).
    /// - `center_on_target` always supplies `document_id` and
    ///   hardcodes `pointing_hint: None, use_mount_hints: true`.
    pub(crate) async fn do_plate_solve(
        &self,
        input: DoPlateSolveInput<'_>,
    ) -> Result<DoPlateSolveOutput, String> {
        // Hint validation: pointing_hint and use_mount_hints=true are
        // mutually exclusive. center_on_target hardcodes
        // pointing_hint=None so it never trips this; the standalone
        // tool may.
        if input.pointing_hint.is_some() && input.use_mount_hints {
            return Err(
                "plate_solve: provide explicit pointing_hint or use_mount_hints, not both"
                    .to_string(),
            );
        }

        let client = self
            .plate_solver
            .as_ref()
            .cloned()
            .ok_or_else(|| "plate_solve: plate solver not configured".to_string())?;

        // Resolve fits_path: document_id wins when both supplied.
        let (fits_path, target_doc_id) = match input.document_id {
            Some(doc_id) => match self.image_cache.resolve_document(doc_id).await {
                Some(doc) => (doc.file_path.clone(), Some(doc_id.to_string())),
                None => return Err(format!("plate_solve: document not found: {}", doc_id)),
            },
            None => {
                let path = input.image_path.ok_or_else(|| {
                    "plate_solve: missing required argument: provide either document_id or image_path"
                        .to_string()
                })?;
                (path.to_string(), None)
            }
        };

        // Resolve hints. The wrapper takes flat ra_hint/dec_hint in
        // decimal degrees; the mount-hint helper does the Alpaca-
        // hours → degrees ×15 conversion.
        let (ra_hint, dec_hint) = if let Some((ra_deg, dec_deg)) = input.pointing_hint {
            (Some(ra_deg), Some(dec_deg))
        } else if input.use_mount_hints {
            match self.read_mount_hints_for_plate_solve().await {
                Ok((ra_deg, dec_deg)) => (Some(ra_deg), Some(dec_deg)),
                Err(e) => return Err(format!("plate_solve: use_mount_hints requested but {}", e)),
            }
        } else {
            (None, None)
        };

        // search_radius_deg: per-call value > config default > absent.
        let search_radius_deg = input
            .search_radius_deg
            .or(self.plate_solver_default_search_radius_deg);

        let request = rp_plate_solver::SolveRequest {
            fits_path: fits_path.clone(),
            ra_hint,
            dec_hint,
            fov_hint_deg: input.fov_hint_deg,
            search_radius_deg,
            timeout: input.timeout,
        };

        let outcome = match client.solve(request).await {
            Ok(o) => o,
            Err(rp_plate_solver::SolveError::ServiceUnreachable(reason)) => {
                return Err(format!("plate_solve: service unreachable: {}", reason));
            }
            Err(rp_plate_solver::SolveError::Wrapper {
                code,
                message,
                details,
            }) => {
                if details.is_null() {
                    return Err(format!("plate_solve: {}: {}", code, message));
                }
                return Err(format!(
                    "plate_solve: {}: {} (details: {})",
                    code, message, details
                ));
            }
            Err(rp_plate_solver::SolveError::Internal(reason)) => {
                return Err(format!("plate_solve: internal: {}", reason));
            }
        };

        // Persist `wcs` section. document_id mode targets the
        // resolved document directly; image_path mode reads the
        // sibling `<base>.json` sidecar via
        // `ImageCache::resolve_document_by_path` so the late-solve
        // workflow's call (path-only, no document_id known to the
        // caller) still updates the matching sidecar.
        let payload = serde_json::json!({
            "ra_center": outcome.ra_center,
            "dec_center": outcome.dec_center,
            "pixel_scale_arcsec": outcome.pixel_scale_arcsec,
            "rotation_deg": outcome.rotation_deg,
            "solver": outcome.solver,
        });
        let persist_doc_id = match target_doc_id {
            Some(id) => Some(id),
            None => self
                .image_cache
                .resolve_document_by_path(&fits_path)
                .await
                .map(|d| d.id.clone()),
        };
        if let Some(doc_id) = persist_doc_id {
            if let Err(e) = self
                .image_cache
                .put_section(&doc_id, "wcs", payload.clone())
                .await
            {
                debug!(error = %e, document_id = %doc_id, "failed to persist wcs section");
            }
        } else {
            debug!(
                fits_path = %fits_path,
                "image_path did not resolve to a known document; skipping wcs persistence"
            );
        }

        Ok(DoPlateSolveOutput {
            ra_center: outcome.ra_center,
            dec_center: outcome.dec_center,
            pixel_scale_arcsec: outcome.pixel_scale_arcsec,
            rotation_deg: outcome.rotation_deg,
            solver: outcome.solver,
        })
    }
}

/// Input bundle for [`McpHandler::do_plate_solve`]. Borrows the two
/// path-shaped inputs by `&str` (call sites already own the strings)
/// and takes the rest by value (all small `Copy` / `Option` types).
pub(crate) struct DoPlateSolveInput<'a> {
    pub document_id: Option<&'a str>,
    pub image_path: Option<&'a str>,
    /// Decimal degrees `(ra, dec)`. Mutually exclusive with
    /// `use_mount_hints == true`.
    pub pointing_hint: Option<(f64, f64)>,
    pub use_mount_hints: bool,
    pub fov_hint_deg: Option<f64>,
    pub search_radius_deg: Option<f64>,
    pub timeout: Option<Duration>,
}

/// Output of [`McpHandler::do_plate_solve`]: the wrapper's success
/// fields verbatim. Callers wrap this in a `tool_success!` payload
/// or in a [`crate::imaging::tools::center_on_target::SolveOutcome`]
/// as needed.
pub(crate) struct DoPlateSolveOutput {
    pub ra_center: f64,
    pub dec_center: f64,
    pub pixel_scale_arcsec: f64,
    pub rotation_deg: f64,
    pub solver: String,
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

pub(crate) fn stats_outcome<T: imaging::Pixel>(
    view: ndarray::ArrayView2<T>,
) -> crate::error::Result<imaging::ImageStats> {
    // `compute_stats` is typed on `&mut [i32]` and uses
    // `select_nth_unstable` in place. Materialize a flat i32 buffer
    // once here and hand it to the kernel mutably so the cached-pixel
    // path doesn't pay the second n × 4 bytes that an immutable slice
    // signature would force (caller copy + kernel-internal clone).
    // Negative pixels are clamped to 0 inside `compute_stats`, so the
    // `to_u32() as i32` round-trip is safe for realistic camera
    // ranges (u16 cameras + i32 scientific HDR ≤ i32::MAX).
    let mut pixels: Vec<i32> = view.iter().map(|p| p.to_u32() as i32).collect();
    imaging::compute_stats(&mut pixels)
        .ok_or_else(|| crate::error::RpError::Imaging("image has no pixels".into()))
}

pub(crate) fn clip_outcome<T: imaging::Pixel>(
    view: ndarray::ArrayView2<T>,
    params: &ResolvedClipParams,
) -> crate::error::Result<BackgroundOutcome> {
    let (rows, cols) = view.dim();
    let total_pixels = (rows as u64) * (cols as u64);
    let stats =
        imaging::sigma_clipped_stats(view, params.k, params.max_iters).ok_or_else(|| {
            crate::error::RpError::Imaging("background estimation failed".to_string())
        })?;
    Ok(BackgroundOutcome {
        stats,
        total_pixels,
    })
}

pub(crate) fn detect_outcome<T: imaging::Pixel>(
    view: ndarray::ArrayView2<T>,
    params: &ResolvedDetectParams,
    max_adu: Option<u32>,
) -> crate::error::Result<DetectStarsOutcome> {
    let background = imaging::estimate_background(view).ok_or_else(|| {
        crate::error::RpError::Imaging("background estimation failed".to_string())
    })?;

    let detection = DetectionParams {
        threshold_sigma: params.threshold_sigma,
        smoothing_sigma: 1.0,
        min_area: params.min_area,
        max_area: params.max_area,
        max_adu,
    };
    let stars = imaging::detect_stars(view, &background, &detection);
    Ok(DetectStarsOutcome { stars, background })
}

pub(crate) fn star_to_json(s: &Star) -> serde_json::Value {
    serde_json::json!({
        "x": s.centroid_x,
        "y": s.centroid_y,
        "flux": s.total_flux,
        "peak": s.peak,
        "saturated_pixel_count": s.saturated_pixel_count,
    })
}

/// Outcome variants for [`poll_slewing_until_idle`].
pub(crate) enum PollIdleError {
    /// Deadline expired with `slewing()` still returning `true`.
    Timeout,
    /// `slewing()` itself returned an Alpaca error.
    Read(ascom_alpaca::ASCOMError),
}

/// Poll `mount.slewing()` every 100 ms until it returns `false`,
/// bounded by a 300 s deadline. Used by `do_slew_blocking`. (The
/// sibling `do_park_blocking` polls `at_park()` directly rather than
/// `slewing()` because `IsSlewing` is sticky on `MoveAxis` rate
/// state and `AtPark` is the ASCOM-canonical "park is complete"
/// signal — see the comment on `do_park_blocking`.) On
/// [`PollIdleError::Timeout`] the caller decides whether to
/// best-effort `abort_slew()` (slew does) or just surface the
/// timeout.
pub(crate) async fn poll_slewing_until_idle(
    mount: &(dyn ascom_alpaca::api::Telescope + Send + Sync),
) -> std::result::Result<(), PollIdleError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        match mount.slewing().await {
            Ok(false) => return Ok(()),
            Ok(true) if tokio::time::Instant::now() < deadline => continue,
            Ok(true) => return Err(PollIdleError::Timeout),
            Err(e) => return Err(PollIdleError::Read(e)),
        }
    }
}
