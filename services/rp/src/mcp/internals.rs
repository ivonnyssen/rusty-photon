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
use tokio::time::Instant;
use tracing::debug;
use uuid::Uuid;

use crate::equipment::alpaca::retry_idempotent_read;
use crate::events::EventEnvelope;
use crate::imaging::{self, BackgroundStats, DetectionParams, Star};
use crate::persistence::{self, CachedImage, CachedPixels, ExposureDocument};

use super::handler::McpHandler;
use super::progress::{ProgressEmitter, PROGRESS_INTERVAL};

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

/// Default per-camera readout + download estimate used to size the
/// predictive exposure deadline (§2.4) when `camera.readout_time_estimate`
/// is omitted. Conservative (slow side) so the advertised `predicted`
/// over-estimates rather than under-estimates — the Sentinel watchdog must
/// not flag a healthy-but-slow readout. A fast USB-3 CMOS reads out in well
/// under a second; this 15 s default is sized for an unconfigured slow
/// USB-2 CCD, and a real rig sets a tighter value per camera.
const DEFAULT_READOUT_TIME_ESTIMATE: Duration = Duration::from_secs(15);

/// Additive slack over the exposure `predicted` for the advertised
/// hard-ceiling `max` (§2.4): `max = duration + readout_time_estimate +
/// EXPOSURE_READOUT_HEADROOM`. Covers a slow USB-2 download tail beyond the
/// per-camera estimate. This sizes only the deadline carried on the
/// `exposure_started` envelope (which the Sentinel watchdog tracks) — rp's
/// own readout backstop is the separate, deliberately more generous
/// [`CAPTURE_READOUT_GRACE`]; the camera driver owns enforcement.
const EXPOSURE_READOUT_HEADROOM: Duration = Duration::from_secs(30);

/// Size the predictive exposure deadline (§2.4) for the `exposure_started`
/// envelope: `predicted = duration + readout_estimate`, `max = predicted +
/// EXPOSURE_READOUT_HEADROOM`. Pure millisecond math returning the
/// `(predicted_ms, max_ms)` envelope pair. Unlike the slew/park/focuser
/// helpers it takes no `&self` and never fails: the camera is already
/// resolved at the `do_capture` call site, there is no pre-op device read,
/// and rp does not enforce this deadline (so it returns no poll `Duration`).
/// Saturating arithmetic keeps an absurd operator-supplied `duration` from
/// overflowing rather than panicking.
pub(crate) fn exposure_deadlines(duration: Duration, readout_estimate: Duration) -> (u64, u64) {
    let predicted = duration.saturating_add(readout_estimate);
    let max = predicted.saturating_add(EXPOSURE_READOUT_HEADROOM);
    (
        u64::try_from(predicted.as_millis()).unwrap_or(u64::MAX),
        u64::try_from(max.as_millis()).unwrap_or(u64::MAX),
    )
}

/// Size the predictive `center_on_target` deadline (§2.5) for the
/// `centering_started` envelope. The Sentinel watchdog tracks only the
/// outer loop (each per-iteration `slew`/`capture` carries its own
/// deadline), so: `per_iter = capture_duration + solve_time_estimate +
/// slew_overhead_estimate`, `predicted = per_iter` (optimistic single-pass
/// convergence), `max = max_attempts × per_iter` (every attempt used).
/// Pure millisecond math returning the `(predicted_ms, max_ms)` envelope
/// pair; saturating arithmetic guards against overflow from an absurd
/// `duration` or `max_attempts`. Like [`exposure_deadlines`], this is
/// advisory only — rp does not enforce it (the inner ops do).
pub(crate) fn centering_deadlines(
    max_attempts: usize,
    capture_duration: Duration,
    solve_time_estimate: Duration,
    slew_overhead_estimate: Duration,
) -> (u64, u64) {
    let per_iter = capture_duration
        .saturating_add(solve_time_estimate)
        .saturating_add(slew_overhead_estimate);
    let predicted_ms = u64::try_from(per_iter.as_millis()).unwrap_or(u64::MAX);
    let max_ms = predicted_ms.saturating_mul(max_attempts as u64);
    (predicted_ms, max_ms)
}

/// Floor on the predictive slew deadline (§2.1 of the predictive-deadlines
/// plan). A short slew still gets at least this long before it's considered
/// overrun, covering fixed overheads that `distance / rate` ignores:
/// acceleration ramps and controller/`IsSlewing` latency. The binding
/// constraint is the OmniSim BDD simulator — it slews at 20°/s with a fixed
/// deceleration tail, so a from-rest slew (its physical axes reset to a
/// startup position each scenario, while `sync_mount` only moves the
/// *reported* coordinates) takes up to ~12 s regardless of the small
/// reported distance rp sizes the deadline from. A real mount's tiny slew
/// is far quicker, so this floor is slack in production. 30 s is ~2.5×
/// OmniSim's ~12 s worst case — margin for a contended CI runner dropping
/// timer ticks (the goto-slew advances a fixed angle per tick, so a stalled
/// timer stretches wall-clock time) — while still surfacing a wedged slew
/// ~10× sooner than the prior hardcoded 300 s ceiling, and well before
/// rmcp's 300 s session keep-alive (the swallowed-hang trigger this plan
/// fixes).
const MIN_SLEW_DEADLINE: Duration = Duration::from_secs(30);

/// Slew deadline used when the predicted deadline can't be computed — the
/// mount isn't resolvable yet, or the pre-slew pointing read failed. A
/// prediction is an optimization, not a precondition for slewing, so the
/// deadline degrades to the historical 300 s ceiling rather than failing
/// the slew.
const SLEW_DEADLINE_FALLBACK: Duration = Duration::from_secs(300);

/// Worst-case axis traverse used to size the park deadline (§2.2). The
/// generic Alpaca `Telescope` trait exposes no park-position getter, so rp
/// cannot compute a great-circle distance to the park position the way
/// `slew` does. 180° is the maximum angular separation between any two
/// points on the sphere — the honest upper bound on how far park can
/// traverse without reading the park coordinates.
const PARK_WORST_CASE_TRAVERSE_DEG: f64 = 180.0;

/// Headroom multiplier over the worst-case park `predicted`. Smaller than
/// slew's ×3 (which sits over a *measured* small distance): park's
/// `predicted` is already a worst-case 180° traverse, so ×3 would re-approach
/// the old 300 s ceiling and defeat the point.
const PARK_DEADLINE_HEADROOM: f64 = 2.0;

/// Floor on the predictive park deadline. More generous than
/// [`MIN_SLEW_DEADLINE`] — park traverses to a fixed mechanical position
/// that can be a long way off, and OmniSim's BDD park is a from-rest
/// physical traverse.
const MIN_PARK_DEADLINE: Duration = Duration::from_secs(60);

/// Park deadline used when no mount is configured (the only case in which
/// the park deadline can't be sized). Park would fail immediately without a
/// mount anyway; the fallback keeps the poll loop bounded for symmetry with
/// the slew path.
const PARK_DEADLINE_FALLBACK: Duration = Duration::from_secs(300);

/// Headroom multiplier over the focuser `predicted` (§2.3): `max =
/// max(predicted × 2, MIN_FOCUSER_DEADLINE)`.
const FOCUSER_DEADLINE_HEADROOM: f64 = 2.0;

/// Floor on the predictive `move_focuser` deadline — a tiny move still gets
/// at least this long, covering fixed controller/`IsMoving` latency.
const MIN_FOCUSER_DEADLINE: Duration = Duration::from_secs(5);

/// Move-focuser deadline used when the predicted deadline can't be computed
/// — the focuser isn't resolvable, or the pre-move position read failed. A
/// prediction is an optimization, not a precondition for moving, so the
/// deadline degrades to the historical 120 s ceiling rather than failing
/// the move.
const FOCUSER_DEADLINE_FALLBACK: Duration = Duration::from_secs(120);

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
    ///
    /// When `progress` is `Some`, the poll loop emits
    /// `notifications/progress` every [`PROGRESS_INTERVAL`] so rmcp's
    /// 300 s session keep-alive cannot fire during a legitimate
    /// long exposure (`duration` plus `CAPTURE_READOUT_GRACE`). The
    /// emitted `progress` is the elapsed fraction of the total
    /// `duration + CAPTURE_READOUT_GRACE` budget; messages cycle
    /// `"exposing"` → `"reading_out"` once `image_ready` flips true.
    /// `None` (unit tests, MCP clients that omitted `progressToken`)
    /// makes the emission a no-op.
    pub(crate) async fn do_capture(
        &self,
        camera_id: &str,
        duration: Duration,
        progress: Option<&dyn ProgressEmitter>,
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
        // Snapshot the operator-supplied focal length and the five
        // invariant physical-sensor properties cached at connect time.
        // `cam_entry` is a borrow off `self.equipment`; the snapshot
        // copies out the `Copy`/`Option<Copy>` values so the borrow does
        // not have to outlive the `await`s below — which is also what
        // lets `do_capture` avoid the 5 Alpaca round-trips per exposure
        // it used to pay for these properties (see `CameraEntry` docs).
        let focal_length_mm = cam_entry.config.focal_length_mm;
        // Per-camera readout estimate sizes the predictive exposure deadline
        // (§2.4). Omitted in config → the conservative built-in default. rp
        // does not enforce this; it rides the `exposure_started` envelope for
        // the Sentinel watchdog (the camera driver owns the exposure, and
        // `CAPTURE_READOUT_GRACE` below remains rp's own readout backstop).
        let readout_time_estimate = cam_entry
            .config
            .readout_time_estimate
            .unwrap_or(DEFAULT_READOUT_TIME_ESTIMATE);
        let cached_max_adu = cam_entry.max_adu;
        let cached_pixel_size_x_um = cam_entry.pixel_size_x_um;
        let cached_pixel_size_y_um = cam_entry.pixel_size_y_um;
        let cached_sensor_width_px = cam_entry.sensor_width_px;
        let cached_sensor_height_px = cam_entry.sensor_height_px;

        let document_id = Uuid::new_v4().to_string();
        // The 8-char UUID suffix is the on-disk reverse-lookup key used by
        // the cache's disk-fallback resolution (see Phase 7 of
        // `docs/plans/archive/image-evaluation-tools.md` and `rp.md` Persistence).
        // Operator-controlled `file_naming_pattern` rendering is reserved
        // until a token resolver lands; for now capture writes
        // `<uuid8>.fits` regardless of any configured template.
        let uuid8 = &document_id[..8];
        let image_path = format!("{}/{}.fits", self.session_config.data_directory, uuid8);

        let operation_id = Uuid::new_v4().to_string();
        let started_at = chrono::Utc::now();
        let (predicted_ms, max_ms) = exposure_deadlines(duration, readout_time_estimate);
        self.event_bus.emit_operation(
            EventEnvelope::started(
                "exposure",
                &operation_id,
                started_at,
                serde_json::json!({
                    "camera_id": camera_id,
                    "duration": humantime::format_duration(duration).to_string(),
                }),
            )
            .with_deadlines(predicted_ms, max_ms),
        );

        // Run the exposure body (start → poll → download → write FITS →
        // persist) inside one future so the public method emits exactly
        // one of `exposure_complete` / `exposure_failed` to mirror the
        // `exposure_started` above, under a shared `operation_id`. `?`
        // and early `return Err` inside resolve to this block's Result.
        let capture_result: std::result::Result<(), String> = async {
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
            let started_at = Instant::now();
            let total_budget = duration + CAPTURE_READOUT_GRACE;
            let deadline = started_at + total_budget;
            let total_budget_secs = total_budget.as_secs_f64();
            // While `image_ready` returns `false` *before* the requested
            // exposure window elapses, the camera is shuttering. Switch the
            // emitted message to `"reading_out"` once we cross that mark —
            // most cameras hold `image_ready` false until the readout
            // download finishes too, which is when the keep-alive race is
            // most likely to bite (a long sky-survey download in CI). The
            // boundary is informational; the emit cadence is unchanged.
            let mut last_progress_at = started_at;
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
                        let now = Instant::now();
                        if now >= deadline {
                            return Err(format!(
                                "timeout waiting for image_ready after {:?}",
                                total_budget
                            ));
                        }
                        if let Some(sink) = progress {
                            if now.duration_since(last_progress_at) >= PROGRESS_INTERVAL {
                                let elapsed = now.duration_since(started_at).as_secs_f64();
                                let phase = if now.duration_since(started_at) < duration {
                                    "exposing"
                                } else {
                                    "reading_out"
                                };
                                sink.emit(
                                    elapsed,
                                    Some(total_budget_secs),
                                    Some(phase.to_string()),
                                )
                                .await;
                                last_progress_at = now;
                            }
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

            // `captured_max_adu` decides whether we need a u16 or i32 buffer,
            // so it is consulted *before* collecting pixels to let us collect
            // straight into the destination type and avoid the wasted i32→u16
            // round trip.
            //
            // max_adu feeds three consumers: on-disk FITS bit-depth, cache
            // variant, and the exposure document's `max_adu` field
            // (sidecar self-describing for rehydration/archival lineage).
            // The value was read once at connect time and stashed on
            // `CameraEntry` — see its docstring for the connect-time-failure
            // semantics. When `None` we still persist the document with
            // `max_adu: None`, write the FITS as i32 (lossless fallback), and
            // skip the cache insert.
            let captured_max_adu: Option<u32> = cached_max_adu;

            // Optical geometry for the sidecar's `optics` block. Combines the
            // operator-supplied focal length with the cached pixel-size and
            // sensor-dimension reads from `CameraEntry`. Any missing piece
            // (focal length not configured, connect-time read failed) drops
            // the whole block — see `docs/services/rp.md` §"Core Fields".
            let optics = match focal_length_mm {
                Some(focal_length_mm) => {
                    match (
                        cached_pixel_size_x_um,
                        cached_pixel_size_y_um,
                        cached_sensor_width_px,
                        cached_sensor_height_px,
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
                                // All cached values are present but the
                                // derivation declined — typically a non-
                                // positive or wild-magnitude reading that
                                // would have overflowed the derived pixel
                                // scale / FOV. Surface enough to diagnose
                                // bad camera state or a misconfigured focal
                                // length.
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
                    persistence::write_fits_u16(
                        &image_path,
                        &u16_pixels,
                        width,
                        height,
                        &document_id,
                    )
                    .await
                    .map_err(|e| format!("failed to write FITS file: {}", e))?;
                    CachedPixels::from_u16_pixels(u16_pixels, shape)
                }
                _ => {
                    let i32_pixels: Vec<i32> = image_array.iter().copied().collect();
                    drop(image_array);
                    persistence::write_fits_i32(
                        &image_path,
                        &i32_pixels,
                        width,
                        height,
                        &document_id,
                    )
                    .await
                    .map_err(|e| format!("failed to write FITS file: {}", e))?;
                    captured_max_adu
                        .and_then(|m| CachedPixels::from_i32_pixels(i32_pixels, shape, m))
                }
            };

            // Cooling metadata (rp.md § Camera Cooling): the rung the
            // controller currently holds for this camera, and a
            // best-effort post-readout temperature read. Both are
            // auxiliary — a failed read only drops the field, never
            // the capture.
            let cooler_setpoint_c = self
                .cooling
                .as_ref()
                .and_then(|cooling| cooling.rung_for(camera_id));
            let sensor_temperature_c = cam.ccd_temperature().await.ok();

            let doc = ExposureDocument {
                id: document_id.clone(),
                captured_at: chrono::Utc::now().to_rfc3339(),
                file_path: image_path.clone(),
                width,
                height,
                camera_id: Some(camera_id.to_string()),
                duration: Some(duration),
                max_adu: captured_max_adu,
                cooler_setpoint_c,
                sensor_temperature_c,
                optics,
                sections: serde_json::Map::new(),
            };
            self.persist_capture_artifact(doc, cached_pixels, captured_max_adu)
                .await;

            Ok(())
        }
        .await;

        match &capture_result {
            Ok(()) => self.event_bus.emit_operation(EventEnvelope::complete(
                "exposure",
                &operation_id,
                started_at,
                serde_json::json!({
                    "document_id": document_id,
                    "file_path": image_path,
                }),
            )),
            Err(e) => self.event_bus.emit_operation(EventEnvelope::failed(
                "exposure",
                &operation_id,
                started_at,
                e,
            )),
        }
        capture_result.map(|()| (image_path, document_id))
    }

    /// Size the predictive `move_focuser` deadline from the focuser's
    /// current position, the requested target, and the configured step rate
    /// (§2.3): `predicted = |target − current| / steps_per_sec`,
    /// `max = max(predicted × 2, MIN_FOCUSER_DEADLINE)`. Returns the poll
    /// deadline plus the `(predicted_ms, max_ms)` pair for the
    /// `move_focuser_started` envelope.
    ///
    /// `Err` if the focuser can't be resolved, the pre-move position read
    /// fails, or an absurdly small (but config-valid) step rate makes the
    /// deadline overflow `Duration` (`try_from_secs_f64`); the caller then
    /// falls back to [`FOCUSER_DEADLINE_FALLBACK`] and omits the envelope
    /// deadline fields.
    async fn compute_focuser_deadline(
        &self,
        focuser_id: &str,
        target: i32,
    ) -> std::result::Result<(Duration, u64, u64), String> {
        let foc_entry = self
            .equipment
            .find_focuser(focuser_id)
            .ok_or_else(|| format!("focuser not found: {focuser_id}"))?;
        let foc = foc_entry
            .device
            .as_ref()
            .ok_or_else(|| format!("focuser not connected: {focuser_id}"))?;
        let rate = foc_entry.config.steps_per_sec.value();
        let current = foc
            .position()
            .await
            .map_err(|e| format!("failed to read focuser position: {e}"))?;
        // i64 difference can't overflow two i32s; abs gives the step travel.
        let distance = (i64::from(target) - i64::from(current)).unsigned_abs() as f64;
        let predicted_secs = distance / rate;
        let max_secs =
            (predicted_secs * FOCUSER_DEADLINE_HEADROOM).max(MIN_FOCUSER_DEADLINE.as_secs_f64());
        let deadline = Duration::try_from_secs_f64(max_secs).map_err(|e| {
            format!(
                "predicted focuser deadline out of range \
                 (steps_per_sec {rate}, distance {distance} steps): {e}"
            )
        })?;
        Ok((
            deadline,
            (predicted_secs * 1000.0).round() as u64,
            (max_secs * 1000.0).round() as u64,
        ))
    }

    /// Resolve a focuser, validate the requested `position` against the
    /// operator-supplied `min_position`/`max_position` bounds, issue the
    /// Alpaca move, poll `is_moving` until idle (bounded by a predicted
    /// deadline; see [`Self::compute_focuser_deadline`]), and return the
    /// focuser's reported `position` after settling.
    ///
    /// This is the shared body of the `move_focuser` MCP tool and the
    /// `auto_focus` compound tool's per-step focuser drive — both want
    /// the same bounds-check + blocking-poll semantics.
    ///
    /// When `progress` is `Some`, the `is_moving` poll loop emits
    /// `notifications/progress` every [`PROGRESS_INTERVAL`] so rmcp's
    /// 300 s session keep-alive sees session activity from a focuser
    /// run that approaches its own deadline. `None` (unit tests,
    /// clients without `progressToken`) makes the emission a no-op.
    pub(crate) async fn do_move_focuser_blocking(
        &self,
        focuser_id: &str,
        position: i32,
        progress: Option<&dyn ProgressEmitter>,
    ) -> std::result::Result<i32, String> {
        let operation_id = Uuid::new_v4().to_string();
        let started_at = chrono::Utc::now();

        // Size the deadline from the move's actual workload. If the focuser
        // can't be resolved or the pre-move position read fails, fall back to
        // the historical 120 s ceiling and omit the deadline fields — a
        // prediction is an optimization, not a precondition for moving.
        let started_payload = serde_json::json!({ "focuser_id": focuser_id, "position": position });
        let (deadline, started_event) = match self
            .compute_focuser_deadline(focuser_id, position)
            .await
        {
            Ok((deadline, predicted_ms, max_ms)) => (
                deadline,
                EventEnvelope::started("move_focuser", &operation_id, started_at, started_payload)
                    .with_deadlines(predicted_ms, max_ms),
            ),
            Err(e) => {
                debug!(error = %e, "move_focuser deadline prediction unavailable; using fallback ceiling");
                (
                    FOCUSER_DEADLINE_FALLBACK,
                    EventEnvelope::started(
                        "move_focuser",
                        &operation_id,
                        started_at,
                        started_payload,
                    ),
                )
            }
        };
        self.event_bus.emit_operation(started_event);

        let result = self
            .do_move_focuser_blocking_inner(focuser_id, position, deadline, progress)
            .await;
        match &result {
            Ok(final_position) => self.event_bus.emit_operation(EventEnvelope::complete(
                "move_focuser",
                &operation_id,
                started_at,
                serde_json::json!({ "focuser_id": focuser_id, "position": final_position }),
            )),
            Err(e) => self.event_bus.emit_operation(EventEnvelope::failed(
                "move_focuser",
                &operation_id,
                started_at,
                e,
            )),
        }
        result
    }

    /// Inner body of [`do_move_focuser_blocking`] — resolve + bounds-check
    /// then move, poll until idle, and read back. Split out so the public
    /// method wraps it in the `move_focuser_started` /
    /// `move_focuser_complete` / `move_focuser_failed` triple. `deadline` is
    /// the predicted poll ceiling sized by the wrapper (see
    /// [`Self::compute_focuser_deadline`]).
    async fn do_move_focuser_blocking_inner(
        &self,
        focuser_id: &str,
        position: i32,
        deadline: Duration,
        progress: Option<&dyn ProgressEmitter>,
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

        let total_budget = deadline;
        let total_budget_secs = total_budget.as_secs_f64();
        let started_at = Instant::now();
        let deadline = started_at + total_budget;
        let mut last_progress_at = started_at;
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            match foc.is_moving().await {
                Ok(false) => break,
                Ok(true) if Instant::now() < deadline => {
                    let now = Instant::now();
                    if let Some(sink) = progress {
                        if now.duration_since(last_progress_at) >= PROGRESS_INTERVAL {
                            let elapsed = now.duration_since(started_at).as_secs_f64();
                            sink.emit(
                                elapsed,
                                Some(total_budget_secs),
                                Some("focuser_moving".to_string()),
                            )
                            .await;
                            last_progress_at = now;
                        }
                    }
                    continue;
                }
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

    /// Size the predictive slew deadline from the mount's current
    /// pointing, the requested target, the configured slew rate, and the
    /// settle time (§2.1). `ra` is in hours (the `slew` boundary unit),
    /// `dec` in degrees. Returns the poll deadline plus the
    /// `(predicted_ms, max_ms)` pair for the `slew_started` envelope.
    ///
    /// `Err` if the mount can't be resolved or a pre-slew pointing read
    /// fails; the caller then falls back to [`SLEW_DEADLINE_FALLBACK`] and
    /// omits the envelope deadline fields.
    async fn compute_slew_deadline(
        &self,
        ra: f64,
        dec: f64,
        settle_after: Duration,
    ) -> std::result::Result<(Duration, u64, u64), String> {
        let (entry, mount) = self.resolve_mount()?;
        let rate = entry.config.slew_rate_arcsec_per_sec.value();
        let current_ra = mount
            .right_ascension()
            .await
            .map_err(|e| format!("failed to read mount right_ascension: {}", e))?;
        let current_dec = mount
            .declination()
            .await
            .map_err(|e| format!("failed to read mount declination: {}", e))?;
        // `haversine_arcsec` takes degrees for both coordinates; RA is in
        // hours at this boundary, so scale both RA terms by 15 (matching
        // `center_on_target`).
        let distance_arcsec =
            imaging::haversine_arcsec(current_ra * 15.0, current_dec, ra * 15.0, dec);
        let predicted_secs = distance_arcsec / rate + settle_after.as_secs_f64();
        let max_secs = (predicted_secs * 3.0).max(MIN_SLEW_DEADLINE.as_secs_f64());
        // An absurdly small (but config-valid) rate makes distance / rate
        // huge — either +inf, or finite-but-larger than a `Duration` can
        // hold. `try_from_secs_f64` rejects non-finite, negative, AND
        // overflowing values, so we fall back to the 300 s ceiling rather
        // than panicking (which `Duration::from_secs_f64` would do for any
        // of those).
        let deadline = Duration::try_from_secs_f64(max_secs).map_err(|e| {
            format!(
                "predicted slew deadline out of range \
                 (slew_rate_arcsec_per_sec {rate}, distance {distance_arcsec} arcsec): {e}"
            )
        })?;
        Ok((
            deadline,
            (predicted_secs * 1000.0).round() as u64,
            (max_secs * 1000.0).round() as u64,
        ))
    }

    /// Resolve the mount, issue an async slew, poll `slewing()` until
    /// idle (bounded by a predicted deadline; see
    /// [`Self::compute_slew_deadline`]), sleep `settle_after`, then read
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
    ///
    /// When `progress` is `Some`, the inner `poll_slewing_until_idle`
    /// and the `settle_after` sleep emit `notifications/progress`
    /// every [`PROGRESS_INTERVAL`] so rmcp's 300 s session keep-alive
    /// cannot fire during a legitimate long slew (whose deadline scales
    /// with distance and can exceed the 300 s keep-alive).
    pub(crate) async fn do_slew_blocking(
        &self,
        ra: f64,
        dec: f64,
        settle_after: Duration,
        progress: Option<&dyn ProgressEmitter>,
    ) -> std::result::Result<(f64, f64), String> {
        let operation_id = Uuid::new_v4().to_string();
        let started_at = chrono::Utc::now();

        // Size the deadline from the slew's actual workload. If the mount
        // can't be resolved or the pre-slew pointing read fails, fall back
        // to the historical 300 s ceiling and omit the deadline fields —
        // a prediction is an optimization, not a precondition for slewing.
        let started_payload = serde_json::json!({ "ra": ra, "dec": dec });
        let (deadline, started_event) = match self
            .compute_slew_deadline(ra, dec, settle_after)
            .await
        {
            Ok((deadline, predicted_ms, max_ms)) => (
                deadline,
                EventEnvelope::started("slew", &operation_id, started_at, started_payload)
                    .with_deadlines(predicted_ms, max_ms),
            ),
            Err(e) => {
                debug!(error = %e, "slew deadline prediction unavailable; using fallback ceiling");
                (
                    SLEW_DEADLINE_FALLBACK,
                    EventEnvelope::started("slew", &operation_id, started_at, started_payload),
                )
            }
        };
        self.event_bus.emit_operation(started_event);

        let result = self
            .do_slew_blocking_inner(ra, dec, settle_after, deadline, progress)
            .await;
        match &result {
            Ok((actual_ra, actual_dec)) => self.event_bus.emit_operation(EventEnvelope::complete(
                "slew",
                &operation_id,
                started_at,
                serde_json::json!({
                    "ra": ra,
                    "dec": dec,
                    "actual_ra": actual_ra,
                    "actual_dec": actual_dec,
                }),
            )),
            Err(e) => self.event_bus.emit_operation(EventEnvelope::failed(
                "slew",
                &operation_id,
                started_at,
                e,
            )),
        }
        result
    }

    /// Inner body of [`do_slew_blocking`] — the slew + poll-until-idle +
    /// settle + post-slew read. Split out so the public method can wrap
    /// it in the `slew_started` / `slew_complete` / `slew_failed` event
    /// triple under one `operation_id`. Every call (including
    /// `center_on_target`'s per-iteration slews) emits its own triple;
    /// Sentinel filters inner-vs-outer in Phase 4. `deadline` is the
    /// predicted poll ceiling sized by the wrapper (see
    /// [`Self::compute_slew_deadline`]).
    async fn do_slew_blocking_inner(
        &self,
        ra: f64,
        dec: f64,
        settle_after: Duration,
        deadline: Duration,
        progress: Option<&dyn ProgressEmitter>,
    ) -> std::result::Result<(f64, f64), String> {
        let (_entry, mount) = self.resolve_mount()?;

        debug!(ra, dec, "slewing mount");
        mount
            .slew_to_coordinates_async(ra, dec)
            .await
            .map_err(|e| format!("failed to slew: {}", e))?;

        match poll_slewing_until_idle(mount.as_ref(), deadline, progress).await {
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
            // For settles long enough to cross PROGRESS_INTERVAL, emit
            // a single tick so the session keep-alive can't fire
            // during the settle even when the upstream slew finished
            // quickly.
            if let Some(sink) = progress {
                if settle_after >= PROGRESS_INTERVAL {
                    sink.emit(
                        0.0,
                        Some(settle_after.as_secs_f64()),
                        Some("settling".to_string()),
                    )
                    .await;
                }
            }
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

    /// Size the park deadline (§2.2). rp can't read the mount's park
    /// coordinates — the generic Alpaca `Telescope` trait exposes no
    /// park-position getter — so the deadline is the worst-case full-axis
    /// traverse ([`PARK_WORST_CASE_TRAVERSE_DEG`]) at the configured slew
    /// rate, not a distance-scaled prediction like slew:
    /// `predicted = 180° / slew_rate + settle`,
    /// `max = max(predicted × 2, MIN_PARK_DEADLINE)`. Returns the poll
    /// deadline plus the `(predicted_ms, max_ms)` pair for the
    /// `park_started` envelope.
    ///
    /// `Err` when no mount is configured, or when an absurdly small (but
    /// config-valid) slew rate makes the worst-case deadline overflow
    /// `Duration` (`try_from_secs_f64`). The caller then falls back to
    /// [`PARK_DEADLINE_FALLBACK`] and omits the envelope deadline fields
    /// (and park fails immediately anyway when no mount is configured).
    fn compute_park_deadline(&self) -> std::result::Result<(Duration, u64, u64), String> {
        let entry = self
            .equipment
            .find_mount()
            .ok_or_else(|| "no mount configured".to_string())?;
        let rate = entry.config.slew_rate_arcsec_per_sec.value();
        let settle = entry.config.settle_after_slew.unwrap_or(Duration::ZERO);
        let worst_case_arcsec = PARK_WORST_CASE_TRAVERSE_DEG * 3600.0;
        let predicted_secs = worst_case_arcsec / rate + settle.as_secs_f64();
        let max_secs =
            (predicted_secs * PARK_DEADLINE_HEADROOM).max(MIN_PARK_DEADLINE.as_secs_f64());
        let deadline = Duration::try_from_secs_f64(max_secs).map_err(|e| {
            format!("predicted park deadline out of range (slew_rate_arcsec_per_sec {rate}): {e}")
        })?;
        Ok((
            deadline,
            (predicted_secs * 1000.0).round() as u64,
            (max_secs * 1000.0).round() as u64,
        ))
    }

    /// Resolve the mount, issue `park()`, then poll `at_park()` every
    /// 100 ms until it returns `true`, bounded by a predicted deadline
    /// (see [`Self::compute_park_deadline`]).
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
    ///
    /// When `progress` is `Some`, the `at_park` poll loop emits
    /// `notifications/progress` every [`PROGRESS_INTERVAL`] so rmcp's
    /// 300 s session keep-alive cannot fire during a legitimate
    /// long park (whose deadline can exceed the keep-alive).
    pub(crate) async fn do_park_blocking(
        &self,
        progress: Option<&dyn ProgressEmitter>,
    ) -> std::result::Result<(), String> {
        let operation_id = Uuid::new_v4().to_string();
        let started_at = chrono::Utc::now();

        // Size the deadline from the worst-case traverse at the configured
        // slew rate. With no mount configured, fall back to the historical
        // 300 s ceiling and omit the deadline fields.
        let (deadline, started_event) = match self.compute_park_deadline() {
            Ok((deadline, predicted_ms, max_ms)) => (
                deadline,
                EventEnvelope::started("park", &operation_id, started_at, serde_json::json!({}))
                    .with_deadlines(predicted_ms, max_ms),
            ),
            Err(e) => {
                debug!(error = %e, "park deadline prediction unavailable; using fallback ceiling");
                (
                    PARK_DEADLINE_FALLBACK,
                    EventEnvelope::started(
                        "park",
                        &operation_id,
                        started_at,
                        serde_json::json!({}),
                    ),
                )
            }
        };
        self.event_bus.emit_operation(started_event);

        let result = self.do_park_blocking_inner(deadline, progress).await;
        match &result {
            Ok(()) => self.event_bus.emit_operation(EventEnvelope::complete(
                "park",
                &operation_id,
                started_at,
                serde_json::json!({}),
            )),
            Err(e) => self.event_bus.emit_operation(EventEnvelope::failed(
                "park",
                &operation_id,
                started_at,
                e,
            )),
        }
        result
    }

    /// Inner body of [`do_park_blocking`] — the `park()` call + the
    /// `at_park` poll loop. Split out so the public method wraps it in
    /// the `park_started` / `park_complete` / `park_failed` triple. The
    /// timeout path still returns `Err` (so `park_failed` fires) and
    /// still does NOT auto-abort — the watchdog ladder owns that decision
    /// in Phase 5. `deadline` is the predicted poll ceiling sized by the
    /// wrapper (see [`Self::compute_park_deadline`]).
    async fn do_park_blocking_inner(
        &self,
        deadline: Duration,
        progress: Option<&dyn ProgressEmitter>,
    ) -> std::result::Result<(), String> {
        let (_entry, mount) = self.resolve_mount()?;

        debug!("parking mount");
        mount
            .park()
            .await
            .map_err(|e| format!("failed to park: {}", e))?;

        let total_budget = deadline;
        let total_budget_secs = total_budget.as_secs_f64();
        let started_at = Instant::now();
        let deadline = started_at + total_budget;
        let mut last_progress_at = started_at;
        loop {
            match mount.at_park().await {
                Ok(true) => return Ok(()),
                Ok(false) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    let now = Instant::now();
                    if let Some(sink) = progress {
                        if now.duration_since(last_progress_at) >= PROGRESS_INTERVAL {
                            let elapsed = now.duration_since(started_at).as_secs_f64();
                            sink.emit(
                                elapsed,
                                Some(total_budget_secs),
                                Some("parking".to_string()),
                            )
                            .await;
                            last_progress_at = now;
                        }
                    }
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
        let operation_id = Uuid::new_v4().to_string();
        let started_at = chrono::Utc::now();
        let result = self.do_sync_mount_inner(ra_hours, dec_deg).await;
        match &result {
            Ok(()) => self.event_bus.emit_operation(EventEnvelope::complete(
                "sync_mount",
                &operation_id,
                started_at,
                serde_json::json!({ "ra": ra_hours, "dec": dec_deg }),
            )),
            Err(e) => self.event_bus.emit_operation(EventEnvelope::failed(
                "sync_mount",
                &operation_id,
                started_at,
                e,
            )),
        }
        result
    }

    /// Inner body of [`do_sync_mount`]. Sync is instant per ASCOM, so the
    /// public method emits only `sync_mount_complete` /
    /// `sync_mount_failed` (no `_started` / timer).
    async fn do_sync_mount_inner(
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
    ///
    /// `center_on_target` issues this read every iteration; under heavy
    /// parallel-OmniSim CI load it was the read that stalled and hung
    /// the whole loop (issue #319). Both reads are idempotent, so they
    /// retry a transient failure via [`retry_idempotent_read`] rather
    /// than aborting the compound tool on a single hiccup; the
    /// per-request read timeout (see `equipment::alpaca`) bounds each
    /// attempt.
    pub(crate) async fn read_mount_hints_for_plate_solve(&self) -> Result<(f64, f64), String> {
        let (_entry, mount) = self.resolve_mount()?;
        let ra_hours = retry_idempotent_read("mount right_ascension", || {
            let mount = mount.clone();
            async move {
                mount
                    .right_ascension()
                    .await
                    .map_err(|e| format!("failed to read mount right_ascension: {e}"))
            }
        })
        .await?;
        let dec_deg = retry_idempotent_read("mount declination", || {
            let mount = mount.clone();
            async move {
                mount
                    .declination()
                    .await
                    .map_err(|e| format!("failed to read mount declination: {e}"))
            }
        })
        .await?;
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
        let operation_id = Uuid::new_v4().to_string();
        let started_at = chrono::Utc::now();
        self.event_bus.emit_operation(EventEnvelope::started(
            "plate_solve",
            &operation_id,
            started_at,
            serde_json::json!({
                "document_id": input.document_id,
                "image_path": input.image_path,
                "use_mount_hints": input.use_mount_hints,
            }),
        ));
        let result = self.do_plate_solve_inner(input).await;
        match &result {
            Ok(out) => self.event_bus.emit_operation(EventEnvelope::complete(
                "plate_solve",
                &operation_id,
                started_at,
                serde_json::json!({
                    "ra_center": out.ra_center,
                    "dec_center": out.dec_center,
                    "pixel_scale_arcsec": out.pixel_scale_arcsec,
                    "rotation_deg": out.rotation_deg,
                    "solver": out.solver,
                }),
            )),
            Err(e) => self.event_bus.emit_operation(EventEnvelope::failed(
                "plate_solve",
                &operation_id,
                started_at,
                e,
            )),
        }
        result
    }

    /// Inner body of [`do_plate_solve`]. Split out so the public method
    /// wraps it in the `plate_solve_started` / `plate_solve_complete` /
    /// `plate_solve_failed` triple under one `operation_id`.
    async fn do_plate_solve_inner(
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
#[derive(Debug)]
pub(crate) enum PollIdleError {
    /// Deadline expired with `slewing()` still returning `true`.
    Timeout,
    /// `slewing()` itself returned an Alpaca error.
    Read(ascom_alpaca::ASCOMError),
}

/// Consecutive `slewing()` read errors [`poll_slewing_until_idle`]
/// tolerates before giving up. A transient read failure — the now
/// timeout-bounded stall a loaded OmniSim or a flaky link produces
/// (issue #319) — is treated like "not idle yet" and retried on the
/// next tick; only a *persistent* failure aborts the slew. The 300 s
/// deadline still caps the total wait. Mirrors the connect path's
/// tolerance for a transient device stall.
const SLEWING_READ_ERROR_TOLERANCE: u32 = 5;

/// Poll `mount.slewing()` every 100 ms until it returns `false`,
/// bounded by `deadline`. `do_slew_blocking` sizes the deadline from the
/// slew distance (see `compute_slew_deadline`); a flaky pre-slew read
/// falls back to `SLEW_DEADLINE_FALLBACK`. (The sibling
/// `do_park_blocking` polls `at_park()` directly rather than
/// `slewing()` because `IsSlewing` is sticky on `MoveAxis` rate
/// state and `AtPark` is the ASCOM-canonical "park is complete"
/// signal — see the comment on `do_park_blocking`.) On
/// [`PollIdleError::Timeout`] the caller decides whether to
/// best-effort `abort_slew()` (slew does) or just surface the
/// timeout.
///
/// A transient `slewing()` read error is tolerated (kept polling) up to
/// [`SLEWING_READ_ERROR_TOLERANCE`] consecutive failures so a brief
/// device hiccup mid-slew doesn't abort the whole `center_on_target`
/// loop; a successful read resets the counter.
///
/// When `progress` is `Some`, the loop emits
/// `notifications/progress` every [`PROGRESS_INTERVAL`] so rmcp's
/// 300 s session keep-alive cannot fire during a legitimate slew
/// (a long slew's `deadline` can exceed the 300 s keep-alive — without
/// progress emission the two timers race).
pub(crate) async fn poll_slewing_until_idle(
    mount: &(dyn ascom_alpaca::api::Telescope + Send + Sync),
    deadline: Duration,
    progress: Option<&dyn ProgressEmitter>,
) -> std::result::Result<(), PollIdleError> {
    let total_budget = deadline;
    let total_budget_secs = total_budget.as_secs_f64();
    let started_at = Instant::now();
    let deadline = started_at + total_budget;
    let mut last_progress_at = started_at;
    let mut consecutive_read_errors: u32 = 0;
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        match mount.slewing().await {
            Ok(false) => return Ok(()),
            Ok(true) if Instant::now() < deadline => {
                consecutive_read_errors = 0;
                let now = Instant::now();
                if let Some(sink) = progress {
                    if now.duration_since(last_progress_at) >= PROGRESS_INTERVAL {
                        let elapsed = now.duration_since(started_at).as_secs_f64();
                        sink.emit(
                            elapsed,
                            Some(total_budget_secs),
                            Some("slewing".to_string()),
                        )
                        .await;
                        last_progress_at = now;
                    }
                }
                continue;
            }
            Ok(true) => return Err(PollIdleError::Timeout),
            Err(e) => {
                consecutive_read_errors += 1;
                if consecutive_read_errors >= SLEWING_READ_ERROR_TOLERANCE
                    || Instant::now() >= deadline
                {
                    return Err(PollIdleError::Read(e));
                }
                debug!(
                    consecutive_read_errors,
                    max = SLEWING_READ_ERROR_TOLERANCE,
                    error = %e,
                    "transient mount slewing() read error, continuing to poll"
                );
            }
        }
    }
}
