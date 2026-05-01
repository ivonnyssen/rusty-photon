use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::cover_calibrator::{CalibratorStatus, CoverStatus};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use tracing::debug;
use uuid::Uuid;

use crate::equipment::EquipmentRegistry;
use crate::events::EventBus;
use crate::imaging::{self, BackgroundStats, DetectionParams, Star};
use crate::persistence::{self, CachedImage, CachedPixels, ExposureDocument, ImageCache};
use crate::session::SessionConfig;

// ---------------------------------------------------------------------------
// Macros
// ---------------------------------------------------------------------------

/// Build a successful `CallToolResult` from a `serde_json::json!(...)` value.
macro_rules! tool_success {
    ($($json:tt)+) => {
        CallToolResult::success(vec![Content::text(
            serde_json::json!($($json)+).to_string(),
        )])
    };
}

/// Build an error `CallToolResult` from a format string or literal.
macro_rules! tool_error {
    ($lit:literal) => {
        CallToolResult::error(vec![Content::text($lit)])
    };
    ($($arg:tt)+) => {
        CallToolResult::error(vec![Content::text(format!($($arg)+))])
    };
}

/// Look up a device by ID and return the entry + connected device, or
/// early-return a `tool_error` `CallToolResult` from the enclosing function.
///
/// Usage: `let (entry, device) = resolve_device!(self, find_camera, id, "camera");`
macro_rules! resolve_device {
    ($self:expr, $finder:ident, $id:expr, $kind:literal) => {{
        let entry = match $self.equipment.$finder($id) {
            Some(e) => e,
            None => return Ok(tool_error!(concat!($kind, " not found: {}"), $id)),
        };
        let device = match &entry.device {
            Some(d) => d.clone(),
            None => return Ok(tool_error!(concat!($kind, " not connected: {}"), $id)),
        };
        (entry, device)
    }};
}

// ---------------------------------------------------------------------------
// Parameter structs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CaptureParams {
    /// Camera device ID
    pub camera_id: String,
    /// Exposure time as a humantime string (e.g. `"500ms"`, `"30s"`, `"1m30s"`).
    #[serde(with = "humantime_serde")]
    #[schemars(with = "String")]
    pub duration: Duration,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CameraIdParams {
    /// Camera device ID
    pub camera_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ComputeImageStatsParams {
    /// Filesystem path to FITS image file
    pub image_path: String,
    /// Optional: document ID for tracking
    #[serde(default)]
    pub document_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeasureBasicParams {
    /// Document ID of a previously-captured image. Resolved via the image
    /// cache first, falling back to the FITS file recorded on the document.
    /// One of `document_id` or `image_path` is required. If both are
    /// supplied, `document_id` takes precedence and `image_path` is ignored
    /// (per the design doc — the cache resolution path always wins).
    #[serde(default)]
    pub document_id: Option<String>,
    /// Filesystem path to a FITS file. Used when no `document_id` is given.
    #[serde(default)]
    pub image_path: Option<String>,
    /// Detection threshold above sky in multiples of background stddev.
    #[serde(default = "default_threshold_sigma")]
    pub threshold_sigma: f64,
    /// Minimum component pixel area to admit as a star. Required, but
    /// modeled as `Option` so the tool body can validate input presence in
    /// a deterministic order — `image_path`/`document_id` first, areas
    /// second — and produce input-shaped error messages.
    #[serde(default)]
    pub min_area: Option<usize>,
    /// Maximum component pixel area to admit as a star. Required (same
    /// rationale as `min_area`).
    #[serde(default)]
    pub max_area: Option<usize>,
}

fn default_threshold_sigma() -> f64 {
    5.0
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EstimateBackgroundParams {
    /// Document ID of a previously-captured image. Resolved via the image
    /// cache first, falling back to the FITS file recorded on the document.
    /// One of `document_id` or `image_path` is required. If both are
    /// supplied, `document_id` takes precedence and `image_path` is ignored
    /// (per the design doc — the cache resolution path always wins).
    #[serde(default)]
    pub document_id: Option<String>,
    /// Filesystem path to a FITS file. Used when no `document_id` is given.
    #[serde(default)]
    pub image_path: Option<String>,
    /// Sigma-clip threshold in stddev units. Must be > 0.
    #[serde(default = "default_clip_k")]
    pub k: f64,
    /// Maximum clip iterations. Must be >= 1.
    #[serde(default = "default_clip_max_iters")]
    pub max_iters: u32,
}

fn default_clip_k() -> f64 {
    3.0
}

fn default_clip_max_iters() -> u32 {
    5
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeasureStarsParams {
    /// Document ID of a previously-captured image. Resolved via the image
    /// cache first, falling back to the FITS file recorded on the document.
    /// One of `document_id` or `image_path` is required. If both are
    /// supplied, `document_id` takes precedence and `image_path` is ignored
    /// (per the design doc — the cache resolution path always wins).
    #[serde(default)]
    pub document_id: Option<String>,
    /// Filesystem path to a FITS file. Used when no `document_id` is given.
    #[serde(default)]
    pub image_path: Option<String>,
    /// Detection threshold above sky in multiples of background stddev.
    #[serde(default = "default_threshold_sigma")]
    pub threshold_sigma: f64,
    /// Minimum component pixel area to admit as a star. Required (validated
    /// in body, same pattern as `measure_basic` / `detect_stars`).
    #[serde(default)]
    pub min_area: Option<usize>,
    /// Maximum component pixel area to admit as a star. Required.
    #[serde(default)]
    pub max_area: Option<usize>,
    /// Half-side (px) of the postage stamp used for the 2D Gaussian fit.
    /// Stars whose stamp would cross the image boundary are kept with
    /// `fwhm: null` / `eccentricity: null`.
    #[serde(default = "default_stamp_half_size")]
    pub stamp_half_size: usize,
}

fn default_stamp_half_size() -> usize {
    imaging::DEFAULT_STAMP_HALF_SIZE
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ComputeSnrParams {
    /// Document ID of a previously-captured image. Resolved via the image
    /// cache first, falling back to the FITS file recorded on the document.
    /// One of `document_id` or `image_path` is required. If both are
    /// supplied, `document_id` takes precedence and `image_path` is ignored
    /// (per the design doc — the cache resolution path always wins).
    #[serde(default)]
    pub document_id: Option<String>,
    /// Filesystem path to a FITS file. Used when no `document_id` is given.
    #[serde(default)]
    pub image_path: Option<String>,
    /// Detection threshold above sky in multiples of background stddev.
    #[serde(default = "default_threshold_sigma")]
    pub threshold_sigma: f64,
    /// Minimum component pixel area to admit as a star. Required (validated
    /// in body, same pattern as the other imaging tools).
    #[serde(default)]
    pub min_area: Option<usize>,
    /// Maximum component pixel area to admit as a star. Required.
    #[serde(default)]
    pub max_area: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DetectStarsParams {
    /// Document ID of a previously-captured image. Resolved via the image
    /// cache first, falling back to the FITS file recorded on the document.
    /// One of `document_id` or `image_path` is required. If both are
    /// supplied, `document_id` takes precedence and `image_path` is ignored
    /// (per the design doc — the cache resolution path always wins).
    #[serde(default)]
    pub document_id: Option<String>,
    /// Filesystem path to a FITS file. Used when no `document_id` is given.
    #[serde(default)]
    pub image_path: Option<String>,
    /// Detection threshold above sky in multiples of background stddev.
    #[serde(default = "default_threshold_sigma")]
    pub threshold_sigma: f64,
    /// Minimum component pixel area to admit as a star. Required, but
    /// modeled as `Option` so the tool body can validate input presence in
    /// a deterministic order — `image_path`/`document_id` first, areas
    /// second — and produce input-shaped error messages (same pattern as
    /// `measure_basic`).
    #[serde(default)]
    pub min_area: Option<usize>,
    /// Maximum component pixel area to admit as a star. Required (same
    /// rationale as `min_area`).
    #[serde(default)]
    pub max_area: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetFilterParams {
    /// Filter wheel device ID
    pub filter_wheel_id: String,
    /// Filter name (must match filter wheel configuration)
    pub filter_name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FilterWheelIdParams {
    /// Filter wheel device ID
    pub filter_wheel_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CalibratorIdParams {
    /// CoverCalibrator device ID
    pub calibrator_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CalibratorOnParams {
    /// CoverCalibrator device ID
    pub calibrator_id: String,
    /// Brightness level (0..max_brightness). Defaults to max if omitted
    #[serde(default)]
    pub brightness: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FocuserIdParams {
    /// Focuser device ID
    pub focuser_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MoveFocuserParams {
    /// Focuser device ID
    pub focuser_id: String,
    /// Target absolute position
    pub position: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AutoFocusToolParams {
    /// Camera device ID. Required (validated in body for deterministic
    /// error ordering — see `docs/services/rp.md` `auto_focus` Contract).
    #[serde(default)]
    pub camera_id: Option<String>,
    /// Focuser device ID. Required (validated in body).
    #[serde(default)]
    pub focuser_id: Option<String>,
    /// Per-frame exposure duration as a humantime string (e.g. `"2s"`,
    /// `"500ms"`). Required (validated in body).
    #[serde(default, with = "humantime_serde")]
    #[schemars(with = "Option<String>")]
    pub duration: Option<Duration>,
    /// Step between adjacent sweep grid points, in absolute focuser
    /// steps. Must be positive. Required (validated in body).
    #[serde(default)]
    pub step_size: Option<i32>,
    /// Half-width of the sweep grid: positions are
    /// `current_position ± half_width` in `step_size` increments,
    /// clamped to the focuser's `min_position` / `max_position`. Must
    /// be positive. Required (validated in body).
    #[serde(default)]
    pub half_width: Option<i32>,
    /// Minimum component pixel area to admit as a star, passed through
    /// to per-frame `measure_basic`. Required (validated in body).
    #[serde(default)]
    pub min_area: Option<usize>,
    /// Maximum component pixel area, passed through to per-frame
    /// `measure_basic`. Required (validated in body).
    #[serde(default)]
    pub max_area: Option<usize>,
    /// Detection threshold above sky in stddev units, passed through to
    /// per-frame `measure_basic`. Default `5.0`.
    #[serde(default = "default_threshold_sigma")]
    pub threshold_sigma: f64,
    /// Minimum non-null HFR samples required for the parabola fit.
    /// Must be at least 3. Default 5.
    #[serde(default)]
    pub min_fit_points: Option<usize>,
}

// ---------------------------------------------------------------------------
// McpHandler
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct McpHandler {
    pub equipment: Arc<EquipmentRegistry>,
    pub event_bus: Arc<EventBus>,
    pub session_config: SessionConfig,
    pub image_cache: ImageCache,
}

impl McpHandler {
    pub fn new(
        equipment: Arc<EquipmentRegistry>,
        event_bus: Arc<EventBus>,
        session_config: SessionConfig,
        image_cache: ImageCache,
    ) -> Self {
        Self {
            equipment,
            event_bus,
            session_config,
            image_cache,
        }
    }

    async fn measure_via_document(
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

    async fn measure_via_path(
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
}

/// `MeasureBasicParams` after schema-level optionals are validated by the
/// tool body. Pure data, no `Option`s — passed to the imaging composer.
struct ResolvedParams {
    threshold_sigma: f64,
    min_area: usize,
    max_area: usize,
}

/// `EstimateBackgroundParams` after sign/range validation. Same pattern as
/// `ResolvedParams`: schema-level optionals, validated in the tool body.
struct ResolvedClipParams {
    k: f64,
    max_iters: usize,
}

/// Background stats paired with the input pixel area (rows × cols). The
/// kernel's `BackgroundStats.n_pixels` is the *surviving* count after
/// sigma-clipping; `total_pixels` is what we report as `pixel_count` in
/// the tool's JSON contract — consistent with `measure_basic`.
struct BackgroundOutcome {
    stats: BackgroundStats,
    total_pixels: u64,
}

impl McpHandler {
    async fn estimate_via_document(
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

    async fn estimate_via_path(
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
}

fn clip_outcome<T: imaging::Pixel>(
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

/// `DetectStarsParams` after schema-level optionals are validated by the
/// tool body. Pure data, no `Option`s — passed to the imaging composer.
struct ResolvedDetectParams {
    threshold_sigma: f64,
    min_area: usize,
    max_area: usize,
}

/// Detection outcome: the star list paired with the background stats used
/// to set the threshold. The tool's JSON contract surfaces both.
struct DetectStarsOutcome {
    stars: Vec<Star>,
    background: BackgroundStats,
}

impl McpHandler {
    async fn detect_via_document(
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

    async fn detect_via_path(
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
}

fn detect_outcome<T: imaging::Pixel>(
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

fn star_to_json(s: &Star) -> serde_json::Value {
    serde_json::json!({
        "x": s.centroid_x,
        "y": s.centroid_y,
        "flux": s.total_flux,
        "peak": s.peak,
        "saturated_pixel_count": s.saturated_pixel_count,
    })
}

/// `MeasureStarsParams` after schema-level optionals are validated by the
/// tool body.
struct ResolvedMeasureStarsParams {
    threshold_sigma: f64,
    min_area: usize,
    max_area: usize,
    stamp_half_size: usize,
}

impl McpHandler {
    async fn measure_stars_via_document(
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

    async fn measure_stars_via_path(
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

    async fn snr_via_document(
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

    async fn snr_via_path(
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
    async fn persist_capture_artifact(
        &self,
        doc: ExposureDocument,
        pixels: Vec<i32>,
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
            if let Some(max_adu) = captured_max_adu {
                let shape = (width as usize, height as usize);
                if let Some(cp) = CachedPixels::from_i32_pixels(pixels, shape, max_adu) {
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
                } else {
                    debug!(
                        document_id = %document_id,
                        "cache: shape mismatch, skipping insert"
                    );
                }
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

        let document_id = Uuid::new_v4().to_string();
        // The 8-char UUID suffix is the on-disk reverse-lookup key used by
        // the cache's disk-fallback resolution (see Phase 7 of
        // `docs/plans/image-evaluation-tools.md` and `rp.md` Persistence).
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

        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            match cam.image_ready().await {
                Ok(true) => break,
                Ok(false) => continue,
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
        let pixels: Vec<i32> = image_array.iter().copied().collect();

        persistence::write_fits(&image_path, &pixels, width, height, &document_id)
            .await
            .map_err(|e| format!("failed to write FITS file: {}", e))?;

        // Read max_adu once. The value feeds two consumers: the cache
        // variant choice (u16 vs i32) and the exposure document's
        // `max_adu` field, which makes the sidecar self-describing for
        // downstream rehydration (Phase 7) and archival lineage. A
        // transient Alpaca failure here is localized to this capture —
        // the next capture re-reads independently. On failure we persist
        // `max_adu: None` on the document and skip the cache insert; the
        // FITS file on disk plus the sidecar remain the durable record.
        let captured_max_adu: Option<u32> = match cam.max_adu().await {
            Ok(v) => Some(v),
            Err(e) => {
                debug!(error = %e, "max_adu unavailable for this capture");
                None
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
            sections: serde_json::Map::new(),
        };
        self.persist_capture_artifact(doc, pixels, captured_max_adu)
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
}

#[tool_router(server_handler)]
impl McpHandler {
    // -------------------------------------------------------------------
    // Camera tools
    // -------------------------------------------------------------------

    #[tool(description = "Capture an image, download image_array, save FITS file")]
    async fn capture(
        &self,
        Parameters(params): Parameters<CaptureParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self.do_capture(&params.camera_id, params.duration).await {
            Ok((image_path, document_id)) => Ok(tool_success!({
                "image_path": image_path,
                "document_id": document_id,
            })),
            Err(e) => Ok(tool_error!("{}", e)),
        }
    }

    #[tool(description = "Read camera capabilities: max_adu, exposure limits, sensor dimensions")]
    async fn get_camera_info(
        &self,
        Parameters(params): Parameters<CameraIdParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (_cam_entry, cam) = resolve_device!(self, find_camera, &params.camera_id, "camera");

        let max_adu = match cam.max_adu().await {
            Ok(v) => v,
            Err(e) => return Ok(tool_error!("failed to read max_adu: {}", e)),
        };

        let (sensor_x, sensor_y) = match cam.camera_size().await {
            Ok(size) => (size[0], size[1]),
            Err(e) => return Ok(tool_error!("failed to read sensor size: {}", e)),
        };

        let (bin_x, bin_y) = match cam.bin().await {
            Ok(bin) => (bin[0] as u32, bin[1] as u32),
            Err(e) => {
                debug!(error = %e, "failed to read binning, using defaults");
                (1u32, 1u32)
            }
        };

        let (exposure_min, exposure_max) = match cam.exposure_range().await {
            Ok(range) => (*range.start(), *range.end()),
            Err(e) => {
                debug!(error = %e, "failed to read exposure range, using defaults");
                (Duration::from_millis(1), Duration::from_secs(3600))
            }
        };

        Ok(tool_success!({
            "camera_id": params.camera_id,
            "max_adu": max_adu,
            "sensor_x": sensor_x,
            "sensor_y": sensor_y,
            "bin_x": bin_x,
            "bin_y": bin_y,
            "exposure_min": humantime::format_duration(exposure_min).to_string(),
            "exposure_max": humantime::format_duration(exposure_max).to_string(),
        }))
    }

    // -------------------------------------------------------------------
    // Image stats tool
    // -------------------------------------------------------------------

    #[tool(
        description = "Read FITS file and compute pixel statistics (median, mean, min, max ADU)"
    )]
    async fn compute_image_stats(
        &self,
        Parameters(params): Parameters<ComputeImageStatsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let image_path = params.image_path;

        let path_clone = image_path.clone();
        let stats = match tokio::task::spawn_blocking(move || {
            let (pixels, _w, _h) = persistence::read_fits_pixels(&path_clone)?;
            imaging::compute_stats(&pixels)
                .ok_or_else(|| crate::error::RpError::Imaging("image has no pixels".into()))
        })
        .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return Ok(tool_error!("failed to compute stats: {}", e)),
            Err(e) => return Ok(tool_error!("task error: {}", e)),
        };

        debug!(
            image_path = %image_path,
            median = stats.median_adu,
            mean = %stats.mean_adu,
            "computed image stats"
        );

        Ok(tool_success!({
            "median_adu": stats.median_adu,
            "mean_adu": stats.mean_adu,
            "min_adu": stats.min_adu,
            "max_adu": stats.max_adu,
            "pixel_count": stats.pixel_count,
        }))
    }

    #[tool(
        description = "Detect stars and compute HFR / sigma-clipped background statistics on a captured image"
    )]
    async fn measure_basic(
        &self,
        Parameters(params): Parameters<MeasureBasicParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        if params.document_id.is_none() && params.image_path.is_none() {
            return Ok(tool_error!(
                "missing required argument: provide either document_id or image_path"
            ));
        }
        let min_area = match params.min_area {
            Some(v) => v,
            None => {
                return Ok(tool_error!("missing required parameter: min_area"));
            }
        };
        let max_area = match params.max_area {
            Some(v) => v,
            None => {
                return Ok(tool_error!("missing required parameter: max_area"));
            }
        };
        let resolved = ResolvedParams {
            threshold_sigma: params.threshold_sigma,
            min_area,
            max_area,
        };

        let result = if let Some(doc_id) = params.document_id.as_deref() {
            match self.measure_via_document(doc_id, &resolved).await {
                Ok(r) => r,
                Err(e) => return Ok(tool_error!("{}", e)),
            }
        } else {
            let path = params.image_path.as_deref().expect("checked above");
            match self.measure_via_path(path, &resolved).await {
                Ok(r) => r,
                Err(e) => return Ok(tool_error!("{}", e)),
            }
        };

        if let Some(doc_id) = params.document_id.as_deref() {
            let value = serde_json::to_value(&result).unwrap_or(serde_json::Value::Null);
            if let Err(e) = self
                .image_cache
                .put_section(doc_id, "image_analysis", value)
                .await
            {
                debug!(error = %e, document_id = %doc_id, "failed to persist image_analysis section");
            }
        }

        Ok(tool_success!({
            "hfr": result.hfr,
            "star_count": result.star_count,
            "saturated_star_count": result.saturated_star_count,
            "background_mean": result.background_mean,
            "background_stddev": result.background_stddev,
            "pixel_count": result.pixel_count,
        }))
    }

    #[tool(description = "Sigma-clipped background mean / stddev / median for a captured image")]
    async fn estimate_background(
        &self,
        Parameters(params): Parameters<EstimateBackgroundParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        if params.document_id.is_none() && params.image_path.is_none() {
            return Ok(tool_error!(
                "missing required argument: provide either document_id or image_path"
            ));
        }
        if !params.k.is_finite() || params.k <= 0.0 {
            return Ok(tool_error!("invalid parameter: k must be > 0"));
        }
        if params.max_iters == 0 {
            return Ok(tool_error!("invalid parameter: max_iters must be >= 1"));
        }
        let resolved = ResolvedClipParams {
            k: params.k,
            max_iters: params.max_iters as usize,
        };

        let outcome = if let Some(doc_id) = params.document_id.as_deref() {
            match self.estimate_via_document(doc_id, &resolved).await {
                Ok(s) => s,
                Err(e) => return Ok(tool_error!("{}", e)),
            }
        } else {
            let path = params.image_path.as_deref().expect("checked above");
            match self.estimate_via_path(path, &resolved).await {
                Ok(s) => s,
                Err(e) => return Ok(tool_error!("{}", e)),
            }
        };

        let payload = serde_json::json!({
            "mean": outcome.stats.mean,
            "stddev": outcome.stats.stddev,
            "median": outcome.stats.median,
            "pixel_count": outcome.total_pixels,
        });

        if let Some(doc_id) = params.document_id.as_deref() {
            if let Err(e) = self
                .image_cache
                .put_section(doc_id, "background", payload.clone())
                .await
            {
                debug!(error = %e, document_id = %doc_id, "failed to persist background section");
            }
        }

        Ok(CallToolResult::success(vec![Content::text(
            payload.to_string(),
        )]))
    }

    #[tool(
        description = "Detect stars on a captured image and return per-star coordinates, flux, peak, and saturation flags"
    )]
    async fn detect_stars(
        &self,
        Parameters(params): Parameters<DetectStarsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        if params.document_id.is_none() && params.image_path.is_none() {
            return Ok(tool_error!(
                "missing required argument: provide either document_id or image_path"
            ));
        }
        let min_area = match params.min_area {
            Some(v) => v,
            None => {
                return Ok(tool_error!("missing required parameter: min_area"));
            }
        };
        let max_area = match params.max_area {
            Some(v) => v,
            None => {
                return Ok(tool_error!("missing required parameter: max_area"));
            }
        };
        let resolved = ResolvedDetectParams {
            threshold_sigma: params.threshold_sigma,
            min_area,
            max_area,
        };

        let outcome = if let Some(doc_id) = params.document_id.as_deref() {
            match self.detect_via_document(doc_id, &resolved).await {
                Ok(o) => o,
                Err(e) => return Ok(tool_error!("{}", e)),
            }
        } else {
            let path = params.image_path.as_deref().expect("checked above");
            match self.detect_via_path(path, &resolved).await {
                Ok(o) => o,
                Err(e) => return Ok(tool_error!("{}", e)),
            }
        };

        let stars_json: Vec<serde_json::Value> = outcome.stars.iter().map(star_to_json).collect();
        let star_count = outcome.stars.len() as u32;
        let saturated_star_count = outcome
            .stars
            .iter()
            .filter(|s| s.saturated_pixel_count > 0)
            .count() as u32;

        let payload = serde_json::json!({
            "stars": stars_json,
            "star_count": star_count,
            "saturated_star_count": saturated_star_count,
            "background_mean": outcome.background.mean,
            "background_stddev": outcome.background.stddev,
        });

        if let Some(doc_id) = params.document_id.as_deref() {
            if let Err(e) = self
                .image_cache
                .put_section(doc_id, "detected_stars", payload.clone())
                .await
            {
                debug!(error = %e, document_id = %doc_id, "failed to persist detected_stars section");
            }
        }

        Ok(CallToolResult::success(vec![Content::text(
            payload.to_string(),
        )]))
    }

    #[tool(
        description = "Per-star photometry and PSF metrics (HFR, FWHM, eccentricity, flux) on a captured image"
    )]
    async fn measure_stars(
        &self,
        Parameters(params): Parameters<MeasureStarsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        if params.document_id.is_none() && params.image_path.is_none() {
            return Ok(tool_error!(
                "missing required argument: provide either document_id or image_path"
            ));
        }
        let min_area = match params.min_area {
            Some(v) => v,
            None => {
                return Ok(tool_error!("missing required parameter: min_area"));
            }
        };
        let max_area = match params.max_area {
            Some(v) => v,
            None => {
                return Ok(tool_error!("missing required parameter: max_area"));
            }
        };
        if params.stamp_half_size == 0 {
            return Ok(tool_error!(
                "invalid parameter: stamp_half_size must be >= 1"
            ));
        }
        let resolved = ResolvedMeasureStarsParams {
            threshold_sigma: params.threshold_sigma,
            min_area,
            max_area,
            stamp_half_size: params.stamp_half_size,
        };

        let result = if let Some(doc_id) = params.document_id.as_deref() {
            match self.measure_stars_via_document(doc_id, &resolved).await {
                Ok(r) => r,
                Err(e) => return Ok(tool_error!("{}", e)),
            }
        } else {
            let path = params.image_path.as_deref().expect("checked above");
            match self.measure_stars_via_path(path, &resolved).await {
                Ok(r) => r,
                Err(e) => return Ok(tool_error!("{}", e)),
            }
        };

        let payload = serde_json::to_value(&result).unwrap_or(serde_json::Value::Null);

        if let Some(doc_id) = params.document_id.as_deref() {
            if let Err(e) = self
                .image_cache
                .put_section(doc_id, "measured_stars", payload.clone())
                .await
            {
                debug!(error = %e, document_id = %doc_id, "failed to persist measured_stars section");
            }
        }

        Ok(CallToolResult::success(vec![Content::text(
            payload.to_string(),
        )]))
    }

    #[tool(
        description = "Median per-star signal-to-noise ratio via the CCD-equation approximation"
    )]
    async fn compute_snr(
        &self,
        Parameters(params): Parameters<ComputeSnrParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        if params.document_id.is_none() && params.image_path.is_none() {
            return Ok(tool_error!(
                "missing required argument: provide either document_id or image_path"
            ));
        }
        let min_area = match params.min_area {
            Some(v) => v,
            None => {
                return Ok(tool_error!("missing required parameter: min_area"));
            }
        };
        let max_area = match params.max_area {
            Some(v) => v,
            None => {
                return Ok(tool_error!("missing required parameter: max_area"));
            }
        };
        let resolved = ResolvedDetectParams {
            threshold_sigma: params.threshold_sigma,
            min_area,
            max_area,
        };

        let result = if let Some(doc_id) = params.document_id.as_deref() {
            match self.snr_via_document(doc_id, &resolved).await {
                Ok(r) => r,
                Err(e) => return Ok(tool_error!("{}", e)),
            }
        } else {
            let path = params.image_path.as_deref().expect("checked above");
            match self.snr_via_path(path, &resolved).await {
                Ok(r) => r,
                Err(e) => return Ok(tool_error!("{}", e)),
            }
        };

        let payload = serde_json::to_value(&result).unwrap_or(serde_json::Value::Null);

        if let Some(doc_id) = params.document_id.as_deref() {
            if let Err(e) = self
                .image_cache
                .put_section(doc_id, "snr", payload.clone())
                .await
            {
                debug!(error = %e, document_id = %doc_id, "failed to persist snr section");
            }
        }

        Ok(CallToolResult::success(vec![Content::text(
            payload.to_string(),
        )]))
    }

    // -------------------------------------------------------------------
    // Filter wheel tools
    // -------------------------------------------------------------------

    #[tool(description = "Set the active filter on a filter wheel")]
    async fn set_filter(
        &self,
        Parameters(params): Parameters<SetFilterParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (fw_entry, fw) = resolve_device!(
            self,
            find_filter_wheel,
            &params.filter_wheel_id,
            "filter wheel"
        );

        let position = match fw_entry
            .config
            .filters
            .iter()
            .position(|f| f == &params.filter_name)
        {
            Some(p) => p,
            None => return Ok(tool_error!("filter not found: {}", params.filter_name)),
        };

        if let Err(e) = fw.set_position(position).await {
            return Ok(tool_error!("failed to set filter position: {}", e));
        }

        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            match fw.position().await {
                Ok(Some(p)) if p == position => break,
                Ok(Some(_)) | Ok(None) => continue,
                Err(e) => {
                    return Ok(tool_error!("error waiting for filter wheel: {}", e));
                }
            }
        }

        self.event_bus.emit(
            "filter_switch",
            serde_json::json!({
                "filter_wheel_id": params.filter_wheel_id,
                "filter_name": params.filter_name,
            }),
        );

        Ok(tool_success!({
            "filter_wheel_id": params.filter_wheel_id,
            "filter_name": params.filter_name,
            "position": position,
        }))
    }

    #[tool(description = "Get the current filter on a filter wheel")]
    async fn get_filter(
        &self,
        Parameters(params): Parameters<FilterWheelIdParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (fw_entry, fw) = resolve_device!(
            self,
            find_filter_wheel,
            &params.filter_wheel_id,
            "filter wheel"
        );

        let position = match fw.position().await {
            Ok(Some(p)) => p,
            Ok(None) => return Ok(tool_error!("filter wheel is moving")),
            Err(e) => {
                return Ok(tool_error!("failed to get filter position: {}", e));
            }
        };

        let filter_name = fw_entry
            .config
            .filters
            .get(position)
            .cloned()
            .unwrap_or_else(|| format!("Filter {}", position));

        Ok(tool_success!({
            "filter_wheel_id": params.filter_wheel_id,
            "filter_name": filter_name,
            "position": position,
        }))
    }

    // -------------------------------------------------------------------
    // CoverCalibrator tools
    // -------------------------------------------------------------------

    #[tool(description = "Close the dust cover (blocks until closed)")]
    async fn close_cover(
        &self,
        Parameters(params): Parameters<CalibratorIdParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (cc_entry, cc) = resolve_device!(
            self,
            find_cover_calibrator,
            &params.calibrator_id,
            "calibrator"
        );
        let poll_interval = cc_entry.config.poll_interval;

        debug!(calibrator_id = %params.calibrator_id, "closing cover");
        if let Err(e) = cc.close_cover().await {
            return Ok(tool_error!("failed to close cover: {}", e));
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            tokio::time::sleep(poll_interval).await;
            match cc.cover_state().await {
                Ok(CoverStatus::Closed) => {
                    debug!(calibrator_id = %params.calibrator_id, "cover closed");
                    return Ok(tool_success!({"status": "closed"}));
                }
                Ok(_) if tokio::time::Instant::now() < deadline => continue,
                Ok(_) => break,
                Err(e) => {
                    return Ok(tool_error!("error polling cover state: {}", e));
                }
            }
        }

        Ok(tool_error!("timeout waiting for cover to close"))
    }

    #[tool(description = "Open the dust cover (blocks until open)")]
    async fn open_cover(
        &self,
        Parameters(params): Parameters<CalibratorIdParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (cc_entry, cc) = resolve_device!(
            self,
            find_cover_calibrator,
            &params.calibrator_id,
            "calibrator"
        );
        let poll_interval = cc_entry.config.poll_interval;

        debug!(calibrator_id = %params.calibrator_id, "opening cover");
        if let Err(e) = cc.open_cover().await {
            return Ok(tool_error!("failed to open cover: {}", e));
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            tokio::time::sleep(poll_interval).await;
            match cc.cover_state().await {
                Ok(CoverStatus::Open) => {
                    debug!(calibrator_id = %params.calibrator_id, "cover opened");
                    return Ok(tool_success!({"status": "open"}));
                }
                Ok(_) if tokio::time::Instant::now() < deadline => continue,
                Ok(_) => break,
                Err(e) => {
                    return Ok(tool_error!("error polling cover state: {}", e));
                }
            }
        }

        Ok(tool_error!("timeout waiting for cover to open"))
    }

    #[tool(description = "Turn on flat panel at brightness (default: max). Blocks until ready")]
    async fn calibrator_on(
        &self,
        Parameters(params): Parameters<CalibratorOnParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (cc_entry, cc) = resolve_device!(
            self,
            find_cover_calibrator,
            &params.calibrator_id,
            "calibrator"
        );
        let poll_interval = cc_entry.config.poll_interval;

        let brightness = if let Some(b) = params.brightness {
            b
        } else {
            match cc.max_brightness().await {
                Ok(max) => max,
                Err(e) => return Ok(tool_error!("failed to read max_brightness: {}", e)),
            }
        };

        debug!(calibrator_id = %params.calibrator_id, brightness = brightness, "turning calibrator on");
        if let Err(e) = cc.calibrator_on(brightness).await {
            return Ok(tool_error!("failed to turn calibrator on: {}", e));
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            tokio::time::sleep(poll_interval).await;
            match cc.calibrator_state().await {
                Ok(CalibratorStatus::Ready) => {
                    debug!(calibrator_id = %params.calibrator_id, "calibrator ready");
                    return Ok(tool_success!({"status": "ready", "brightness": brightness}));
                }
                Ok(_) if tokio::time::Instant::now() < deadline => continue,
                Ok(_) => break,
                Err(e) => {
                    return Ok(tool_error!("error polling calibrator state: {}", e));
                }
            }
        }

        Ok(tool_error!(
            "timeout waiting for calibrator to become ready"
        ))
    }

    #[tool(description = "Turn off flat panel. Blocks until off")]
    async fn calibrator_off(
        &self,
        Parameters(params): Parameters<CalibratorIdParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (cc_entry, cc) = resolve_device!(
            self,
            find_cover_calibrator,
            &params.calibrator_id,
            "calibrator"
        );
        let poll_interval = cc_entry.config.poll_interval;

        debug!(calibrator_id = %params.calibrator_id, "turning calibrator off");
        if let Err(e) = cc.calibrator_off().await {
            return Ok(tool_error!("failed to turn calibrator off: {}", e));
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            tokio::time::sleep(poll_interval).await;
            match cc.calibrator_state().await {
                Ok(CalibratorStatus::Off) => {
                    debug!(calibrator_id = %params.calibrator_id, "calibrator off");
                    return Ok(tool_success!({"status": "off"}));
                }
                Ok(_) if tokio::time::Instant::now() < deadline => continue,
                Ok(_) => break,
                Err(e) => {
                    return Ok(tool_error!("error polling calibrator state: {}", e));
                }
            }
        }

        Ok(tool_error!("timeout waiting for calibrator to turn off"))
    }

    // -------------------------------------------------------------------
    // Focuser tools
    // -------------------------------------------------------------------

    #[tool(description = "Move the focuser to an absolute position (blocks until idle)")]
    async fn move_focuser(
        &self,
        Parameters(params): Parameters<MoveFocuserParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self
            .do_move_focuser_blocking(&params.focuser_id, params.position)
            .await
        {
            Ok(actual_position) => Ok(tool_success!({
                "focuser_id": params.focuser_id,
                "actual_position": actual_position,
            })),
            Err(e) => Ok(tool_error!("{}", e)),
        }
    }

    #[tool(description = "Read the current absolute position of the focuser")]
    async fn get_focuser_position(
        &self,
        Parameters(params): Parameters<FocuserIdParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (_entry, foc) = resolve_device!(self, find_focuser, &params.focuser_id, "focuser");

        let position = match foc.position().await {
            Ok(p) => p,
            Err(e) => return Ok(tool_error!("failed to read focuser position: {}", e)),
        };

        Ok(tool_success!({
            "focuser_id": params.focuser_id,
            "position": position,
        }))
    }

    #[tool(description = "Read the focuser temperature sensor (null if not implemented)")]
    async fn get_focuser_temperature(
        &self,
        Parameters(params): Parameters<FocuserIdParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (_entry, foc) = resolve_device!(self, find_focuser, &params.focuser_id, "focuser");

        // ASCOM `Temperature` and `TempCompAvailable` are independent: a
        // focuser may expose a temperature reading while reporting
        // `TempCompAvailable=false` (qhy-focuser is the canonical local
        // example). Try the temperature read directly and only translate
        // a `NOT_IMPLEMENTED` rejection to `null`; surface every other
        // error to the caller.
        let temperature_c: Option<f64> = match foc.temperature().await {
            Ok(t) => Some(t),
            Err(e) if e.code == ascom_alpaca::ASCOMErrorCode::NOT_IMPLEMENTED => None,
            Err(e) => return Ok(tool_error!("failed to read focuser temperature: {}", e)),
        };

        Ok(tool_success!({
            "focuser_id": params.focuser_id,
            "temperature_c": temperature_c,
        }))
    }

    // -------------------------------------------------------------------
    // Compound: auto_focus (V-curve)
    // -------------------------------------------------------------------

    #[tool(
        description = "V-curve auto-focus: sweep ± half_width around the focuser's current position, capture and run measure_basic at each step, fit a parabola in HFR, and move the focuser to the fitted minimum"
    )]
    async fn auto_focus(
        &self,
        Parameters(params): Parameters<AutoFocusToolParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        // Field-presence validation runs in input order so the error
        // message always points at the first missing field — same
        // pattern as `measure_basic`.
        let camera_id = match params.camera_id.as_deref() {
            Some(s) => s.to_string(),
            None => return Ok(tool_error!("missing required parameter: camera_id")),
        };
        let focuser_id = match params.focuser_id.as_deref() {
            Some(s) => s.to_string(),
            None => return Ok(tool_error!("missing required parameter: focuser_id")),
        };
        let duration = match params.duration {
            Some(d) => d,
            None => return Ok(tool_error!("missing required parameter: duration")),
        };
        let step_size = match params.step_size {
            Some(s) => s,
            None => return Ok(tool_error!("missing required parameter: step_size")),
        };
        let half_width = match params.half_width {
            Some(s) => s,
            None => return Ok(tool_error!("missing required parameter: half_width")),
        };
        let min_area = match params.min_area {
            Some(s) => s,
            None => return Ok(tool_error!("missing required parameter: min_area")),
        };
        let max_area = match params.max_area {
            Some(s) => s,
            None => return Ok(tool_error!("missing required parameter: max_area")),
        };

        // Resolve devices early — emits the standard
        // "<kind> not found" / "<kind> not connected" errors expected
        // by the BDD device-resolution scenarios. Camera order before
        // focuser matches input order in the contract.
        let (_cam_entry, cam) = resolve_device!(self, find_camera, &camera_id, "camera");
        let _ = cam; // resolved purely for the connection check; do_capture re-resolves.
        let (foc_entry, foc) = resolve_device!(self, find_focuser, &focuser_id, "focuser");

        // Read current focuser position + temperature once for the
        // `focus_started` event (per the Contract algorithm step 1).
        // run_auto_focus re-reads internally — two cheap Alpaca calls
        // is fine; threading them through would require extending the
        // FocuserOps trait or run_auto_focus's signature with no real
        // benefit.
        let starting_position = match foc.position().await {
            Ok(p) => p,
            Err(e) => return Ok(tool_error!("failed to read focuser position: {}", e)),
        };
        let starting_temperature_c: Option<f64> = foc.temperature().await.ok();
        self.event_bus.emit(
            "focus_started",
            serde_json::json!({
                "camera_id": camera_id,
                "focuser_id": focuser_id,
                "position": starting_position,
                "temperature": starting_temperature_c,
            }),
        );

        let bounds = (foc_entry.config.min_position, foc_entry.config.max_position);
        let af_params = imaging::tools::auto_focus::AutoFocusParams {
            duration,
            step_size,
            half_width,
            min_area,
            max_area,
            threshold_sigma: params.threshold_sigma,
            min_fit_points: params.min_fit_points.unwrap_or(5),
        };

        let adapter = AutoFocusAdapter {
            handler: self,
            camera_id: camera_id.clone(),
            focuser_id: focuser_id.clone(),
            foc,
        };

        match imaging::tools::auto_focus::run_auto_focus(
            &adapter, &adapter, &adapter, bounds, af_params,
        )
        .await
        {
            Ok(result) => {
                self.event_bus.emit(
                    "focus_complete",
                    serde_json::json!({
                        "camera_id": camera_id,
                        "focuser_id": focuser_id,
                        "position": result.best_position,
                        "hfr": result.best_hfr,
                        "samples_used": result.samples_used,
                    }),
                );
                let curve_points =
                    serde_json::to_value(&result.curve_points).unwrap_or(serde_json::Value::Null);
                Ok(tool_success!({
                    "best_position": result.best_position,
                    "best_hfr": result.best_hfr,
                    "final_position": result.final_position,
                    "samples_used": result.samples_used,
                    "curve_points": curve_points,
                    "temperature_c": result.temperature_c,
                }))
            }
            Err(e) => Ok(tool_error!("{}", e)),
        }
    }
}

/// Adapter that satisfies all three [`auto_focus`] traits
/// (`FocuserOps`, `CaptureOps`, `MeasureOps`) by delegating to the
/// existing [`McpHandler`] helpers (`do_move_focuser_blocking`,
/// `do_capture`, `measure_via_document` + cache `put_section`).
///
/// Keeps the compound tool's wiring close to the corresponding
/// primitive tools: same bounds-check / poll semantics on focuser
/// motion, same FITS write / cache insert / event emission on
/// capture, same `image_analysis` section persistence on measure.
struct AutoFocusAdapter<'a> {
    handler: &'a McpHandler,
    camera_id: String,
    focuser_id: String,
    foc: Arc<dyn ascom_alpaca::api::Focuser>,
}

#[async_trait::async_trait]
impl imaging::tools::auto_focus::FocuserOps for AutoFocusAdapter<'_> {
    async fn position(&self) -> std::result::Result<i32, String> {
        self.foc
            .position()
            .await
            .map_err(|e| format!("failed to read focuser position: {}", e))
    }

    async fn move_to(&self, position: i32) -> std::result::Result<i32, String> {
        self.handler
            .do_move_focuser_blocking(&self.focuser_id, position)
            .await
    }

    async fn temperature(&self) -> Option<f64> {
        // Non-fatal read: NOT_IMPLEMENTED → None; transient errors → None.
        // The result's `temperature_c` field is informational; never
        // load-bearing on the sweep, so we never abort the run on a
        // temperature read.
        self.foc.temperature().await.ok()
    }
}

#[async_trait::async_trait]
impl imaging::tools::auto_focus::CaptureOps for AutoFocusAdapter<'_> {
    async fn capture(&self, duration: Duration) -> std::result::Result<String, String> {
        let (_image_path, document_id) = self.handler.do_capture(&self.camera_id, duration).await?;
        Ok(document_id)
    }
}

#[async_trait::async_trait]
impl imaging::tools::auto_focus::MeasureOps for AutoFocusAdapter<'_> {
    async fn measure(
        &self,
        document_id: &str,
        min_area: usize,
        max_area: usize,
        threshold_sigma: f64,
    ) -> std::result::Result<imaging::tools::auto_focus::HfrSample, String> {
        let resolved = ResolvedParams {
            threshold_sigma,
            min_area,
            max_area,
        };
        let result = self
            .handler
            .measure_via_document(document_id, &resolved)
            .await
            .map_err(|e| e.to_string())?;
        // Persist the per-frame `image_analysis` section, matching the
        // standalone `measure_basic` tool's side effect — auto_focus is
        // explicitly composed of measure_basic calls per the contract.
        let value = serde_json::to_value(&result).unwrap_or(serde_json::Value::Null);
        if let Err(e) = self
            .handler
            .image_cache
            .put_section(document_id, "image_analysis", value)
            .await
        {
            debug!(error = %e, document_id, "failed to persist image_analysis section");
        }
        Ok(imaging::tools::auto_focus::HfrSample {
            hfr: result.hfr,
            star_count: result.star_count,
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use ascom_alpaca::ASCOMError;

    // -----------------------------------------------------------------------
    // Mock Device macro
    // -----------------------------------------------------------------------

    /// Generates Debug + Device impl with stubs for all required methods.
    macro_rules! impl_mock_device {
        ($name:ident) => {
            impl std::fmt::Debug for $name {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    write!(f, stringify!($name))
                }
            }

            #[async_trait::async_trait]
            impl ascom_alpaca::api::Device for $name {
                fn static_name(&self) -> &str {
                    "mock"
                }
                fn unique_id(&self) -> &str {
                    "mock-id"
                }
                async fn connected(&self) -> ascom_alpaca::ASCOMResult<bool> {
                    Ok(true)
                }
                async fn set_connected(&self, _: bool) -> ascom_alpaca::ASCOMResult<()> {
                    Ok(())
                }
                async fn description(&self) -> ascom_alpaca::ASCOMResult<String> {
                    Ok("mock".into())
                }
                async fn driver_info(&self) -> ascom_alpaca::ASCOMResult<String> {
                    Ok("mock".into())
                }
                async fn driver_version(&self) -> ascom_alpaca::ASCOMResult<String> {
                    Ok("0.0".into())
                }
            }
        };
    }

    // -----------------------------------------------------------------------
    // MockCamera — single configurable mock for all Camera error-injection
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct MockCamera {
        fail_start_exposure: bool,
        fail_image_ready: bool,
        fail_image_array: bool,
        fail_max_adu: bool,
        fail_camera_size: bool,
        fail_exposure_range: bool,
        /// `0` ⇒ default 65535. Any other value is returned verbatim — set
        /// to `> u16::MAX` to exercise the I32 cache-insert path.
        max_adu_value: u32,
    }

    impl_mock_device!(MockCamera);

    #[async_trait::async_trait]
    impl ascom_alpaca::api::Camera for MockCamera {
        async fn start_exposure(
            &self,
            _duration: Duration,
            _light: bool,
        ) -> ascom_alpaca::ASCOMResult<()> {
            if self.fail_start_exposure {
                return Err(ASCOMError::invalid_operation("shutter jammed"));
            }
            Ok(())
        }

        async fn image_ready(&self) -> ascom_alpaca::ASCOMResult<bool> {
            if self.fail_image_ready {
                return Err(ASCOMError::invalid_operation("readout failed"));
            }
            Ok(true)
        }

        async fn image_array(
            &self,
        ) -> ascom_alpaca::ASCOMResult<ascom_alpaca::api::camera::ImageArray> {
            if self.fail_image_array {
                return Err(ASCOMError::invalid_operation("download timeout"));
            }
            Ok(ndarray::Array3::<i32>::zeros((2, 2, 1)).into())
        }

        async fn max_adu(&self) -> ascom_alpaca::ASCOMResult<u32> {
            if self.fail_max_adu {
                return Err(ASCOMError::invalid_operation("not available"));
            }
            Ok(if self.max_adu_value == 0 {
                65535
            } else {
                self.max_adu_value
            })
        }

        async fn camera_x_size(&self) -> ascom_alpaca::ASCOMResult<u32> {
            if self.fail_camera_size {
                return Err(ASCOMError::invalid_operation("sensor error"));
            }
            Ok(1024)
        }

        async fn camera_y_size(&self) -> ascom_alpaca::ASCOMResult<u32> {
            if self.fail_camera_size {
                return Err(ASCOMError::invalid_operation("sensor error"));
            }
            Ok(1024)
        }

        async fn exposure_max(&self) -> ascom_alpaca::ASCOMResult<Duration> {
            if self.fail_exposure_range {
                return Err(ASCOMError::invalid_operation("range unavailable"));
            }
            Ok(Duration::from_secs(3600))
        }

        async fn exposure_min(&self) -> ascom_alpaca::ASCOMResult<Duration> {
            if self.fail_exposure_range {
                return Err(ASCOMError::invalid_operation("range unavailable"));
            }
            Ok(Duration::from_millis(1))
        }

        async fn exposure_resolution(&self) -> ascom_alpaca::ASCOMResult<Duration> {
            Ok(Duration::from_millis(1))
        }

        async fn has_shutter(&self) -> ascom_alpaca::ASCOMResult<bool> {
            Ok(true)
        }

        async fn pixel_size_x(&self) -> ascom_alpaca::ASCOMResult<f64> {
            Ok(3.76)
        }

        async fn pixel_size_y(&self) -> ascom_alpaca::ASCOMResult<f64> {
            Ok(3.76)
        }

        async fn start_x(&self) -> ascom_alpaca::ASCOMResult<u32> {
            Ok(0)
        }

        async fn set_start_x(&self, _start_x: u32) -> ascom_alpaca::ASCOMResult<()> {
            Ok(())
        }

        async fn start_y(&self) -> ascom_alpaca::ASCOMResult<u32> {
            Ok(0)
        }

        async fn set_start_y(&self, _start_y: u32) -> ascom_alpaca::ASCOMResult<()> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // MockFilterWheel — single configurable mock for FilterWheel errors
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct MockFilterWheel {
        fail_set_position: bool,
        fail_position_poll: bool,
        report_moving: bool,
    }

    impl_mock_device!(MockFilterWheel);

    #[async_trait::async_trait]
    impl ascom_alpaca::api::FilterWheel for MockFilterWheel {
        async fn set_position(&self, _position: usize) -> ascom_alpaca::ASCOMResult<()> {
            if self.fail_set_position {
                return Err(ASCOMError::invalid_operation("wheel stuck"));
            }
            Ok(())
        }

        async fn position(&self) -> ascom_alpaca::ASCOMResult<Option<usize>> {
            if self.fail_position_poll {
                return Err(ASCOMError::invalid_operation("encoder error"));
            }
            if self.report_moving {
                return Ok(None);
            }
            Ok(Some(0))
        }

        async fn names(&self) -> ascom_alpaca::ASCOMResult<Vec<String>> {
            Ok(vec!["Lum".into(), "Red".into()])
        }

        async fn focus_offsets(&self) -> ascom_alpaca::ASCOMResult<Vec<i32>> {
            Ok(vec![0, 0])
        }
    }

    // -----------------------------------------------------------------------
    // MockCoverCalibrator — single configurable mock for CoverCalibrator
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct MockCoverCalibrator {
        fail_close_cover: bool,
        fail_open_cover: bool,
        fail_calibrator_on: bool,
        fail_calibrator_off: bool,
        fail_max_brightness: bool,
        fail_cover_state_poll: bool,
        stuck_cover_moving: bool,
        fail_calibrator_state_poll: bool,
        stuck_calibrator_not_ready: bool,
    }

    impl_mock_device!(MockCoverCalibrator);

    #[async_trait::async_trait]
    impl ascom_alpaca::api::CoverCalibrator for MockCoverCalibrator {
        async fn close_cover(&self) -> ascom_alpaca::ASCOMResult<()> {
            if self.fail_close_cover {
                return Err(ASCOMError::invalid_operation("motor fault"));
            }
            Ok(())
        }

        async fn open_cover(&self) -> ascom_alpaca::ASCOMResult<()> {
            if self.fail_open_cover {
                return Err(ASCOMError::invalid_operation("motor fault"));
            }
            Ok(())
        }

        async fn calibrator_on(&self, _brightness: u32) -> ascom_alpaca::ASCOMResult<()> {
            if self.fail_calibrator_on {
                return Err(ASCOMError::invalid_operation("lamp failure"));
            }
            Ok(())
        }

        async fn calibrator_off(&self) -> ascom_alpaca::ASCOMResult<()> {
            if self.fail_calibrator_off {
                return Err(ASCOMError::invalid_operation("stuck on"));
            }
            Ok(())
        }

        async fn cover_state(&self) -> ascom_alpaca::ASCOMResult<CoverStatus> {
            if self.fail_cover_state_poll {
                return Err(ASCOMError::invalid_operation("device unreachable"));
            }
            if self.stuck_cover_moving {
                return Ok(CoverStatus::Moving);
            }
            Ok(CoverStatus::Closed)
        }

        async fn calibrator_state(&self) -> ascom_alpaca::ASCOMResult<CalibratorStatus> {
            if self.fail_calibrator_state_poll {
                return Err(ASCOMError::invalid_operation("device unreachable"));
            }
            if self.stuck_calibrator_not_ready {
                return Ok(CalibratorStatus::NotReady);
            }
            Ok(CalibratorStatus::Off)
        }

        async fn max_brightness(&self) -> ascom_alpaca::ASCOMResult<u32> {
            if self.fail_max_brightness {
                return Err(ASCOMError::invalid_operation("not supported"));
            }
            Ok(255)
        }

        async fn brightness(&self) -> ascom_alpaca::ASCOMResult<u32> {
            Ok(0)
        }
    }

    // -----------------------------------------------------------------------
    // MockFocuser — single configurable mock for Focuser
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct MockFocuser {
        fail_move: bool,
        fail_is_moving: bool,
        fail_position: bool,
        /// `true` ⇒ `temperature()` returns a generic INVALID_OPERATION
        /// error (sensor wired but reading failed). Distinct from
        /// `temperature_not_implemented` below.
        fail_temperature: bool,
        /// `true` ⇒ `temperature()` returns `ASCOMError::NOT_IMPLEMENTED`.
        /// Models a focuser that does not implement the `Temperature`
        /// property at all.
        temperature_not_implemented: bool,
        stuck_moving: bool,
        temperature_value: f64,
        position_value: i32,
    }

    impl_mock_device!(MockFocuser);

    #[async_trait::async_trait]
    impl ascom_alpaca::api::Focuser for MockFocuser {
        async fn absolute(&self) -> ascom_alpaca::ASCOMResult<bool> {
            Ok(true)
        }

        async fn is_moving(&self) -> ascom_alpaca::ASCOMResult<bool> {
            if self.fail_is_moving {
                return Err(ASCOMError::invalid_operation("encoder fault"));
            }
            Ok(self.stuck_moving)
        }

        async fn max_increment(&self) -> ascom_alpaca::ASCOMResult<u32> {
            Ok(100000)
        }

        async fn max_step(&self) -> ascom_alpaca::ASCOMResult<u32> {
            Ok(100000)
        }

        async fn position(&self) -> ascom_alpaca::ASCOMResult<i32> {
            if self.fail_position {
                return Err(ASCOMError::invalid_operation("position unavailable"));
            }
            Ok(self.position_value)
        }

        async fn step_size(&self) -> ascom_alpaca::ASCOMResult<f64> {
            Err(ASCOMError::NOT_IMPLEMENTED)
        }

        async fn temp_comp(&self) -> ascom_alpaca::ASCOMResult<bool> {
            Ok(false)
        }

        async fn set_temp_comp(&self, _: bool) -> ascom_alpaca::ASCOMResult<()> {
            Err(ASCOMError::NOT_IMPLEMENTED)
        }

        async fn temp_comp_available(&self) -> ascom_alpaca::ASCOMResult<bool> {
            Ok(false)
        }

        async fn temperature(&self) -> ascom_alpaca::ASCOMResult<f64> {
            if self.temperature_not_implemented {
                return Err(ASCOMError::NOT_IMPLEMENTED);
            }
            if self.fail_temperature {
                return Err(ASCOMError::invalid_operation("sensor failure"));
            }
            Ok(self.temperature_value)
        }

        async fn halt(&self) -> ascom_alpaca::ASCOMResult<()> {
            Ok(())
        }

        async fn move_(&self, _position: i32) -> ascom_alpaca::ASCOMResult<()> {
            if self.fail_move {
                return Err(ASCOMError::invalid_operation("focuser stuck"));
            }
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Helper functions
    // -----------------------------------------------------------------------

    fn test_handler(registry: crate::equipment::EquipmentRegistry) -> McpHandler {
        McpHandler::new(
            Arc::new(registry),
            Arc::new(crate::events::EventBus::from_config(&[])),
            SessionConfig {
                data_directory: std::env::temp_dir()
                    .join("rp-unit-test")
                    .to_string_lossy()
                    .to_string(),
            },
            ImageCache::new(64, 4, std::path::PathBuf::from("/nonexistent")),
        )
    }

    fn assert_tool_error(result: Result<CallToolResult, rmcp::ErrorData>, expected_substr: &str) {
        let call_result = result.expect("tool returned protocol error");
        assert!(
            call_result.is_error.unwrap_or(false),
            "expected is_error=true"
        );
        let text = call_result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|tc| tc.text.as_str())
            .unwrap_or("");
        assert!(
            text.contains(expected_substr),
            "expected error containing '{}', got: '{}'",
            expected_substr,
            text
        );
    }

    // -----------------------------------------------------------------------
    // Registry builders
    // -----------------------------------------------------------------------

    fn camera_registry(
        cam: Arc<dyn ascom_alpaca::api::Camera>,
    ) -> crate::equipment::EquipmentRegistry {
        crate::equipment::EquipmentRegistry {
            cameras: vec![crate::equipment::CameraEntry {
                id: "cam".to_string(),
                connected: true,
                config: crate::config::CameraConfig {
                    id: "cam".to_string(),
                    name: "mock".to_string(),
                    alpaca_url: "http://localhost:1".to_string(),
                    device_type: String::new(),
                    device_number: 0,
                    cooler_target_c: None,
                    gain: None,
                    offset: None,
                    auth: None,
                },
                device: Some(cam),
            }],
            filter_wheels: vec![],
            cover_calibrators: vec![],
            focusers: vec![],
        }
    }

    fn filter_wheel_registry(
        fw: Arc<dyn ascom_alpaca::api::FilterWheel>,
    ) -> crate::equipment::EquipmentRegistry {
        crate::equipment::EquipmentRegistry {
            cameras: vec![],
            filter_wheels: vec![crate::equipment::FilterWheelEntry {
                id: "fw".to_string(),
                connected: true,
                config: crate::config::FilterWheelConfig {
                    id: "fw".to_string(),
                    camera_id: String::new(),
                    alpaca_url: "http://localhost:1".to_string(),
                    device_number: 0,
                    filters: vec!["Lum".to_string(), "Red".to_string()],
                    auth: None,
                },
                device: Some(fw),
            }],
            cover_calibrators: vec![],
            focusers: vec![],
        }
    }

    fn calibrator_registry(
        cc: Arc<dyn ascom_alpaca::api::CoverCalibrator>,
    ) -> crate::equipment::EquipmentRegistry {
        crate::equipment::EquipmentRegistry {
            cameras: vec![],
            filter_wheels: vec![],
            cover_calibrators: vec![crate::equipment::CoverCalibratorEntry {
                id: "cc".to_string(),
                connected: true,
                config: crate::config::CoverCalibratorConfig {
                    id: "cc".to_string(),
                    alpaca_url: "http://localhost:1".to_string(),
                    device_number: 0,
                    poll_interval: Duration::from_secs(1),
                    auth: None,
                },
                device: Some(cc),
            }],
            focusers: vec![],
        }
    }

    fn focuser_registry(
        foc: Arc<dyn ascom_alpaca::api::Focuser>,
        min_position: Option<i32>,
        max_position: Option<i32>,
    ) -> crate::equipment::EquipmentRegistry {
        crate::equipment::EquipmentRegistry {
            cameras: vec![],
            filter_wheels: vec![],
            cover_calibrators: vec![],
            focusers: vec![crate::equipment::FocuserEntry {
                id: "foc".to_string(),
                connected: true,
                config: crate::config::FocuserConfig {
                    id: "foc".to_string(),
                    camera_id: String::new(),
                    alpaca_url: "http://localhost:1".to_string(),
                    device_number: 0,
                    min_position,
                    max_position,
                    auth: None,
                },
                device: Some(foc),
            }],
        }
    }

    // -----------------------------------------------------------------------
    // Capture tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_capture_start_exposure_fails() {
        let cam = MockCamera {
            fail_start_exposure: true,
            ..Default::default()
        };
        let handler = test_handler(camera_registry(Arc::new(cam)));
        let result = handler
            .capture(Parameters(CaptureParams {
                camera_id: "cam".into(),
                duration: Duration::from_millis(100),
            }))
            .await;
        assert_tool_error(result, "failed to start exposure");
    }

    #[tokio::test]
    async fn test_capture_image_ready_error() {
        let cam = MockCamera {
            fail_image_ready: true,
            ..Default::default()
        };
        let handler = test_handler(camera_registry(Arc::new(cam)));
        let result = handler
            .capture(Parameters(CaptureParams {
                camera_id: "cam".into(),
                duration: Duration::from_millis(100),
            }))
            .await;
        assert_tool_error(result, "error checking image ready");
    }

    #[tokio::test]
    async fn test_capture_image_array_fails() {
        let cam = MockCamera {
            fail_image_array: true,
            ..Default::default()
        };
        let handler = test_handler(camera_registry(Arc::new(cam)));
        let result = handler
            .capture(Parameters(CaptureParams {
                camera_id: "cam".into(),
                duration: Duration::from_millis(100),
            }))
            .await;
        assert_tool_error(result, "failed to download image array");
    }

    // -----------------------------------------------------------------------
    // get_camera_info tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_camera_info_max_adu_fails() {
        let cam = MockCamera {
            fail_max_adu: true,
            ..Default::default()
        };
        let handler = test_handler(camera_registry(Arc::new(cam)));
        let result = handler
            .get_camera_info(Parameters(CameraIdParams {
                camera_id: "cam".into(),
            }))
            .await;
        assert_tool_error(result, "failed to read max_adu");
    }

    #[tokio::test]
    async fn test_get_camera_info_sensor_size_fails() {
        let cam = MockCamera {
            fail_camera_size: true,
            ..Default::default()
        };
        let handler = test_handler(camera_registry(Arc::new(cam)));
        let result = handler
            .get_camera_info(Parameters(CameraIdParams {
                camera_id: "cam".into(),
            }))
            .await;
        assert_tool_error(result, "failed to read sensor size");
    }

    // -----------------------------------------------------------------------
    // set_filter tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_set_filter_set_position_fails() {
        let fw = MockFilterWheel {
            fail_set_position: true,
            ..Default::default()
        };
        let handler = test_handler(filter_wheel_registry(Arc::new(fw)));
        let result = handler
            .set_filter(Parameters(SetFilterParams {
                filter_wheel_id: "fw".into(),
                filter_name: "Lum".into(),
            }))
            .await;
        assert_tool_error(result, "failed to set filter position");
    }

    // -----------------------------------------------------------------------
    // CoverCalibrator tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_close_cover_command_fails() {
        let cc = MockCoverCalibrator {
            fail_close_cover: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .close_cover(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        assert_tool_error(result, "failed to close cover");
    }

    #[tokio::test]
    async fn test_close_cover_polling_error() {
        let cc = MockCoverCalibrator {
            fail_cover_state_poll: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .close_cover(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        assert_tool_error(result, "error polling cover state");
    }

    #[tokio::test]
    async fn test_open_cover_command_fails() {
        let cc = MockCoverCalibrator {
            fail_open_cover: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .open_cover(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        assert_tool_error(result, "failed to open cover");
    }

    #[tokio::test]
    async fn test_calibrator_on_max_brightness_fails() {
        let cc = MockCoverCalibrator {
            fail_max_brightness: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .calibrator_on(Parameters(CalibratorOnParams {
                calibrator_id: "cc".into(),
                brightness: None,
            }))
            .await;
        assert_tool_error(result, "failed to read max_brightness");
    }

    #[tokio::test]
    async fn test_calibrator_on_command_fails() {
        let cc = MockCoverCalibrator {
            fail_calibrator_on: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .calibrator_on(Parameters(CalibratorOnParams {
                calibrator_id: "cc".into(),
                brightness: None,
            }))
            .await;
        assert_tool_error(result, "failed to turn calibrator on");
    }

    #[tokio::test]
    async fn test_calibrator_off_command_fails() {
        let cc = MockCoverCalibrator {
            fail_calibrator_off: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .calibrator_off(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        assert_tool_error(result, "failed to turn calibrator off");
    }

    // -----------------------------------------------------------------------
    // capture — write_fits failure
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_capture_write_fits_fails() {
        let cam = MockCamera::default(); // succeeds through image_array
        let registry = camera_registry(Arc::new(cam));
        // Use an existing file as the "directory" so write_fits fails cross-platform.
        // The capture tool appends /<uuid8>.fits — creating a file inside
        // another file fails on all OSes.
        let blocker = tempfile::NamedTempFile::new().unwrap();
        let handler = McpHandler::new(
            Arc::new(registry),
            Arc::new(crate::events::EventBus::from_config(&[])),
            SessionConfig {
                data_directory: blocker.path().to_string_lossy().to_string(),
            },
            ImageCache::new(64, 4, std::path::PathBuf::from("/nonexistent")),
        );
        let result = handler
            .capture(Parameters(CaptureParams {
                camera_id: "cam".into(),
                duration: Duration::from_millis(100),
            }))
            .await;
        assert_tool_error(result, "failed to write FITS file");
    }

    // -----------------------------------------------------------------------
    // capture — caches I32 variant when max_adu > u16::MAX
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_capture_caches_i32_when_max_adu_above_u16_max() {
        // Drives the scientific-camera (I32) cache-insert branch in
        // `capture` — exercised by no other test, since OmniSim and the
        // default MockCamera both report max_adu ≤ 65535. Pins the
        // capture invariant: a successful capture leaves the embedded
        // document accessible through the cache entry (now the single
        // source of truth) with the matching `max_adu`.
        let cam = MockCamera {
            max_adu_value: 1 << 20,
            ..Default::default()
        };
        let temp = tempfile::tempdir().unwrap();
        let cache = ImageCache::new(64, 4, std::path::PathBuf::from("/nonexistent"));
        let handler = McpHandler::new(
            Arc::new(camera_registry(Arc::new(cam))),
            Arc::new(crate::events::EventBus::from_config(&[])),
            SessionConfig {
                data_directory: temp.path().to_string_lossy().to_string(),
            },
            cache.clone(),
        );
        let result = handler
            .capture(Parameters(CaptureParams {
                camera_id: "cam".into(),
                duration: Duration::from_millis(100),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|tc| tc.text.clone())
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let doc_id = json["document_id"].as_str().unwrap();
        let cached = cache.get(doc_id).expect("expected cache entry");
        assert_eq!(cached.max_adu, 1 << 20);
        assert!(
            matches!(cached.pixels, CachedPixels::I32(_)),
            "expected I32 variant for max_adu > u16::MAX"
        );
        let doc = cache
            .resolve_document(doc_id)
            .await
            .expect("expected cache entry to carry the document");
        assert_eq!(doc.max_adu, Some(1 << 20));
    }

    // -----------------------------------------------------------------------
    // capture — filename uses 8-char UUID suffix
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_capture_filename_uses_uuid8_suffix() {
        // Pins the on-disk reverse-lookup contract: the FITS basename matches
        // the first 8 hex chars of the document_id. The disk-fallback
        // resolution path in Phase 7 grep's by this suffix.
        let cam = MockCamera::default();
        let temp = tempfile::tempdir().unwrap();
        let cache = ImageCache::new(64, 4, std::path::PathBuf::from("/nonexistent"));
        let handler = McpHandler::new(
            Arc::new(camera_registry(Arc::new(cam))),
            Arc::new(crate::events::EventBus::from_config(&[])),
            SessionConfig {
                data_directory: temp.path().to_string_lossy().to_string(),
            },
            cache,
        );
        let result = handler
            .capture(Parameters(CaptureParams {
                camera_id: "cam".into(),
                duration: Duration::from_millis(100),
            }))
            .await
            .unwrap();
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|tc| tc.text.clone())
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let doc_id = json["document_id"].as_str().unwrap().to_string();
        let image_path = json["image_path"].as_str().unwrap().to_string();
        let basename = std::path::Path::new(&image_path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            basename,
            format!("{}.fits", &doc_id[..8]),
            "FITS basename must equal first 8 hex chars of document_id + .fits"
        );
        assert!(
            std::path::Path::new(&image_path).exists(),
            "FITS file should exist at the reported path"
        );
    }

    // -----------------------------------------------------------------------
    // persist_capture_artifact — sidecar failure skips cache
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_persist_capture_artifact_skips_cache_on_sidecar_failure() {
        // Pins the sidecar-failure branch in `persist_capture_artifact` (the
        // post-FITS persistence step extracted from `capture`). Contract
        // documented in `docs/services/rp.md` → Capture Tool Details
        // → Sidecar failure contract: write_sidecar fails →
        // `document_persistence_failed` event payload is constructed → cache
        // insert is skipped → `document_id`-keyed lookups return 404.
        //
        // Forcing the failure: `doc.file_path` lives inside a regular file so
        // `create_dir_all(parent)` in write_sidecar errors with NotADirectory.
        // Same trick as the put_section rollback tests in cache.rs.
        let temp = tempfile::tempdir().unwrap();
        let blocker = temp.path().join("blocker");
        std::fs::write(&blocker, b"not a directory").unwrap();

        let cache = ImageCache::new(64, 4, std::path::PathBuf::from("/nonexistent"));
        let handler = McpHandler::new(
            Arc::new(crate::equipment::EquipmentRegistry {
                cameras: vec![],
                filter_wheels: vec![],
                cover_calibrators: vec![],
                focusers: vec![],
            }),
            Arc::new(crate::events::EventBus::from_config(&[])),
            SessionConfig {
                data_directory: temp.path().to_string_lossy().to_string(),
            },
            cache.clone(),
        );

        let doc = ExposureDocument {
            id: "doc-fail-1".to_string(),
            captured_at: "2026-04-30T00:00:00Z".to_string(),
            file_path: blocker.join("x.fits").to_string_lossy().into_owned(),
            width: 2,
            height: 2,
            camera_id: Some("cam".into()),
            duration: Some(Duration::from_millis(100)),
            max_adu: Some(65535),
            sections: serde_json::Map::new(),
        };
        let pixels: Vec<i32> = vec![1, 2, 3, 4];

        handler
            .persist_capture_artifact(doc, pixels, Some(65535))
            .await;

        assert!(
            cache.get("doc-fail-1").is_none(),
            "cache must not be populated when sidecar write fails"
        );
    }

    // -----------------------------------------------------------------------
    // get_camera_info — exposure_range fallback
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_camera_info_exposure_range_fallback() {
        let cam = MockCamera {
            fail_exposure_range: true,
            ..Default::default()
        };
        let handler = test_handler(camera_registry(Arc::new(cam)));
        let result = handler
            .get_camera_info(Parameters(CameraIdParams {
                camera_id: "cam".into(),
            }))
            .await;
        // This is a soft failure — it falls back to defaults, so the call succeeds
        let call_result = result.unwrap();
        assert!(!call_result.is_error.unwrap_or(false));
        let text = call_result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|tc| tc.text.as_str())
            .unwrap_or("");
        let json: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(json["exposure_min"], "1ms");
        assert_eq!(json["exposure_max"], "1h");
    }

    // -----------------------------------------------------------------------
    // set_filter — polling error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_set_filter_polling_error() {
        let fw = MockFilterWheel {
            fail_position_poll: true,
            ..Default::default()
        };
        let handler = test_handler(filter_wheel_registry(Arc::new(fw)));
        let result = handler
            .set_filter(Parameters(SetFilterParams {
                filter_wheel_id: "fw".into(),
                filter_name: "Lum".into(),
            }))
            .await;
        assert_tool_error(result, "error waiting for filter wheel");
    }

    // -----------------------------------------------------------------------
    // get_filter — errors
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_filter_position_error() {
        let fw = MockFilterWheel {
            fail_position_poll: true,
            ..Default::default()
        };
        let handler = test_handler(filter_wheel_registry(Arc::new(fw)));
        let result = handler
            .get_filter(Parameters(FilterWheelIdParams {
                filter_wheel_id: "fw".into(),
            }))
            .await;
        assert_tool_error(result, "failed to get filter position");
    }

    #[tokio::test]
    async fn test_get_filter_wheel_moving() {
        let fw = MockFilterWheel {
            report_moving: true,
            ..Default::default()
        };
        let handler = test_handler(filter_wheel_registry(Arc::new(fw)));
        let result = handler
            .get_filter(Parameters(FilterWheelIdParams {
                filter_wheel_id: "fw".into(),
            }))
            .await;
        assert_tool_error(result, "filter wheel is moving");
    }

    // -----------------------------------------------------------------------
    // Timeout tests (use tokio::time::pause to fast-forward)
    // -----------------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn test_close_cover_timeout() {
        let cc = MockCoverCalibrator {
            stuck_cover_moving: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .close_cover(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        assert_tool_error(result, "timeout waiting for cover to close");
    }

    #[tokio::test(start_paused = true)]
    async fn test_open_cover_timeout() {
        let cc = MockCoverCalibrator {
            stuck_cover_moving: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .open_cover(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        assert_tool_error(result, "timeout waiting for cover to open");
    }

    #[tokio::test]
    async fn test_open_cover_polling_error() {
        let cc = MockCoverCalibrator {
            fail_cover_state_poll: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .open_cover(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        assert_tool_error(result, "error polling cover state");
    }

    #[tokio::test(start_paused = true)]
    async fn test_calibrator_on_timeout() {
        let cc = MockCoverCalibrator {
            stuck_calibrator_not_ready: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .calibrator_on(Parameters(CalibratorOnParams {
                calibrator_id: "cc".into(),
                brightness: Some(100),
            }))
            .await;
        assert_tool_error(result, "timeout waiting for calibrator to become ready");
    }

    #[tokio::test]
    async fn test_calibrator_on_polling_error() {
        let cc = MockCoverCalibrator {
            fail_calibrator_state_poll: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .calibrator_on(Parameters(CalibratorOnParams {
                calibrator_id: "cc".into(),
                brightness: Some(100),
            }))
            .await;
        assert_tool_error(result, "error polling calibrator state");
    }

    #[tokio::test(start_paused = true)]
    async fn test_calibrator_off_timeout() {
        let cc = MockCoverCalibrator {
            stuck_calibrator_not_ready: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .calibrator_off(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        assert_tool_error(result, "timeout waiting for calibrator to turn off");
    }

    #[tokio::test]
    async fn test_calibrator_off_polling_error() {
        let cc = MockCoverCalibrator {
            fail_calibrator_state_poll: true,
            ..Default::default()
        };
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .calibrator_off(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        assert_tool_error(result, "error polling calibrator state");
    }

    // -----------------------------------------------------------------------
    // compute_image_stats error paths
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_compute_image_stats_bad_fits() {
        // Write a non-FITS file so read_fits_pixels fails inside spawn_blocking
        let dir = tempfile::tempdir().unwrap();
        let bad_file = dir.path().join("bad.fits");
        std::fs::write(&bad_file, b"not a fits file").unwrap();

        let handler = test_handler(crate::equipment::EquipmentRegistry {
            cameras: vec![],
            filter_wheels: vec![],
            cover_calibrators: vec![],
            focusers: vec![],
        });
        let result = handler
            .compute_image_stats(Parameters(ComputeImageStatsParams {
                image_path: bad_file.to_string_lossy().to_string(),
                document_id: None,
            }))
            .await;
        assert_tool_error(result, "failed to compute stats");
    }

    // -----------------------------------------------------------------------
    // set_filter — filter not found
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_set_filter_filter_not_found() {
        let fw = MockFilterWheel::default();
        let handler = test_handler(filter_wheel_registry(Arc::new(fw)));
        let result = handler
            .set_filter(Parameters(SetFilterParams {
                filter_wheel_id: "fw".into(),
                filter_name: "Ultraviolet".into(), // not in mock's filter list
            }))
            .await;
        assert_tool_error(result, "filter not found");
    }

    // -----------------------------------------------------------------------
    // get_filter — success path (covers lines 387-391)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_filter_success() {
        let fw = MockFilterWheel::default(); // position() returns Some(0)
        let handler = test_handler(filter_wheel_registry(Arc::new(fw)));
        let result = handler
            .get_filter(Parameters(FilterWheelIdParams {
                filter_wheel_id: "fw".into(),
            }))
            .await;
        let call_result = result.unwrap();
        assert!(!call_result.is_error.unwrap_or(false));
        let text = call_result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|tc| tc.text.as_str())
            .unwrap_or("");
        let json: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(json["filter_name"], "Lum");
        assert_eq!(json["position"], 0);
    }

    // -----------------------------------------------------------------------
    // CoverCalibrator success paths (covers resolve_device! macro lines)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_close_cover_success() {
        let cc = MockCoverCalibrator::default(); // cover_state returns Closed
        let handler = test_handler(calibrator_registry(Arc::new(cc)));
        let result = handler
            .close_cover(Parameters(CalibratorIdParams {
                calibrator_id: "cc".into(),
            }))
            .await;
        let call_result = result.unwrap();
        assert!(!call_result.is_error.unwrap_or(false));
    }

    // -----------------------------------------------------------------------
    // Focuser tests
    // -----------------------------------------------------------------------

    fn ok_text(call_result: CallToolResult) -> serde_json::Value {
        assert!(
            !call_result.is_error.unwrap_or(false),
            "expected success, got error: {:?}",
            call_result.content
        );
        let text = call_result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|tc| tc.text.as_str())
            .unwrap_or("");
        serde_json::from_str(text).expect("valid JSON")
    }

    #[tokio::test]
    async fn test_move_focuser_success() {
        let foc = MockFocuser {
            position_value: 4321,
            ..Default::default()
        };
        let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
        let result = handler
            .move_focuser(Parameters(MoveFocuserParams {
                focuser_id: "foc".into(),
                position: 4321,
            }))
            .await
            .unwrap();
        let json = ok_text(result);
        assert_eq!(json["actual_position"], 4321);
        assert_eq!(json["focuser_id"], "foc");
    }

    #[tokio::test]
    async fn test_move_focuser_not_found() {
        let foc = MockFocuser::default();
        let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
        let result = handler
            .move_focuser(Parameters(MoveFocuserParams {
                focuser_id: "missing".into(),
                position: 100,
            }))
            .await;
        assert_tool_error(result, "focuser not found");
    }

    #[tokio::test]
    async fn test_move_focuser_below_min_position() {
        let foc = MockFocuser::default();
        let handler = test_handler(focuser_registry(Arc::new(foc), Some(1000), Some(9000)));
        let result = handler
            .move_focuser(Parameters(MoveFocuserParams {
                focuser_id: "foc".into(),
                position: 500,
            }))
            .await;
        assert_tool_error(result, "position out of range");
    }

    #[tokio::test]
    async fn test_move_focuser_above_max_position() {
        let foc = MockFocuser::default();
        let handler = test_handler(focuser_registry(Arc::new(foc), Some(1000), Some(9000)));
        let result = handler
            .move_focuser(Parameters(MoveFocuserParams {
                focuser_id: "foc".into(),
                position: 9500,
            }))
            .await;
        assert_tool_error(result, "position out of range");
    }

    #[tokio::test]
    async fn test_move_focuser_command_fails() {
        let foc = MockFocuser {
            fail_move: true,
            ..Default::default()
        };
        let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
        let result = handler
            .move_focuser(Parameters(MoveFocuserParams {
                focuser_id: "foc".into(),
                position: 1000,
            }))
            .await;
        assert_tool_error(result, "failed to move focuser");
    }

    #[tokio::test]
    async fn test_move_focuser_is_moving_poll_fails() {
        let foc = MockFocuser {
            fail_is_moving: true,
            ..Default::default()
        };
        let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
        let result = handler
            .move_focuser(Parameters(MoveFocuserParams {
                focuser_id: "foc".into(),
                position: 1000,
            }))
            .await;
        assert_tool_error(result, "error polling focuser is_moving");
    }

    #[tokio::test]
    async fn test_move_focuser_position_read_fails() {
        let foc = MockFocuser {
            fail_position: true,
            ..Default::default()
        };
        let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
        let result = handler
            .move_focuser(Parameters(MoveFocuserParams {
                focuser_id: "foc".into(),
                position: 1000,
            }))
            .await;
        assert_tool_error(result, "failed to read focuser position");
    }

    #[tokio::test(start_paused = true)]
    async fn test_move_focuser_timeout() {
        let foc = MockFocuser {
            stuck_moving: true,
            ..Default::default()
        };
        let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
        let result = handler
            .move_focuser(Parameters(MoveFocuserParams {
                focuser_id: "foc".into(),
                position: 1000,
            }))
            .await;
        assert_tool_error(result, "timeout waiting for focuser to settle");
    }

    #[tokio::test]
    async fn test_move_focuser_not_connected() {
        let registry = crate::equipment::EquipmentRegistry {
            cameras: vec![],
            filter_wheels: vec![],
            cover_calibrators: vec![],
            focusers: vec![crate::equipment::FocuserEntry {
                id: "foc".to_string(),
                connected: false,
                config: crate::config::FocuserConfig {
                    id: "foc".to_string(),
                    camera_id: String::new(),
                    alpaca_url: "http://localhost:1".to_string(),
                    device_number: 0,
                    min_position: None,
                    max_position: None,
                    auth: None,
                },
                device: None,
            }],
        };
        let handler = test_handler(registry);
        let result = handler
            .move_focuser(Parameters(MoveFocuserParams {
                focuser_id: "foc".into(),
                position: 1000,
            }))
            .await;
        assert_tool_error(result, "focuser not connected");
    }

    #[tokio::test]
    async fn test_get_focuser_position_success() {
        let foc = MockFocuser {
            position_value: 12345,
            ..Default::default()
        };
        let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
        let result = handler
            .get_focuser_position(Parameters(FocuserIdParams {
                focuser_id: "foc".into(),
            }))
            .await
            .unwrap();
        let json = ok_text(result);
        assert_eq!(json["position"], 12345);
    }

    #[tokio::test]
    async fn test_get_focuser_position_not_connected() {
        let registry = crate::equipment::EquipmentRegistry {
            cameras: vec![],
            filter_wheels: vec![],
            cover_calibrators: vec![],
            focusers: vec![crate::equipment::FocuserEntry {
                id: "foc".to_string(),
                connected: false,
                config: crate::config::FocuserConfig {
                    id: "foc".to_string(),
                    camera_id: String::new(),
                    alpaca_url: "http://localhost:1".to_string(),
                    device_number: 0,
                    min_position: None,
                    max_position: None,
                    auth: None,
                },
                device: None,
            }],
        };
        let handler = test_handler(registry);
        let result = handler
            .get_focuser_position(Parameters(FocuserIdParams {
                focuser_id: "foc".into(),
            }))
            .await;
        assert_tool_error(result, "focuser not connected");
    }

    /// `Temperature` is independent of `TempCompAvailable`: a focuser may
    /// report a temperature reading regardless of whether temperature
    /// compensation is available. The mock leaves `temp_comp_available()`
    /// at its default `Ok(false)` to make that decoupling explicit.
    #[tokio::test]
    async fn test_get_focuser_temperature_returns_value() {
        let foc = MockFocuser {
            temperature_value: 12.5,
            ..Default::default()
        };
        let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
        let result = handler
            .get_focuser_temperature(Parameters(FocuserIdParams {
                focuser_id: "foc".into(),
            }))
            .await
            .unwrap();
        let json = ok_text(result);
        assert_eq!(json["temperature_c"], 12.5);
    }

    /// `Temperature` returning `NOT_IMPLEMENTED` is the only signal that
    /// the property is unsupported on this device; the tool surfaces
    /// `temperature_c: null` for that exact case.
    #[tokio::test]
    async fn test_get_focuser_temperature_null_when_not_implemented() {
        let foc = MockFocuser {
            temperature_not_implemented: true,
            ..Default::default()
        };
        let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
        let result = handler
            .get_focuser_temperature(Parameters(FocuserIdParams {
                focuser_id: "foc".into(),
            }))
            .await
            .unwrap();
        let json = ok_text(result);
        assert!(
            json["temperature_c"].is_null(),
            "expected null temperature_c, got {:?}",
            json["temperature_c"]
        );
    }

    /// Any non-`NOT_IMPLEMENTED` failure from `temperature()` propagates
    /// as a tool error rather than being silently coerced to `null`. This
    /// pins the asymmetry between "device says I don't have one" and
    /// "device tried to read but the read itself failed".
    #[tokio::test]
    async fn test_get_focuser_temperature_sensor_fails() {
        let foc = MockFocuser {
            fail_temperature: true,
            ..Default::default()
        };
        let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
        let result = handler
            .get_focuser_temperature(Parameters(FocuserIdParams {
                focuser_id: "foc".into(),
            }))
            .await;
        assert_tool_error(result, "failed to read focuser temperature");
    }
}
