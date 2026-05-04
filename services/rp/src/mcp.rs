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
pub struct SlewParams {
    /// Target right ascension in decimal hours, in `[0.0, 24.0)`.
    /// Required (validated in body for deterministic error ordering).
    #[serde(default)]
    pub ra: Option<f64>,
    /// Target declination in decimal degrees, in `[-90.0, 90.0]`.
    /// Required (validated in body).
    #[serde(default)]
    pub dec: Option<f64>,
    /// Optional per-call override for the post-`Slewing == false`
    /// settle. `None` uses `mount.settle_after_slew` from config (which
    /// itself defaults to zero). `Some("0s")` skips settle even when
    /// the config sets a non-zero default.
    #[serde(default, with = "humantime_serde")]
    #[schemars(with = "Option<String>")]
    pub settle_after: Option<Duration>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SyncMountParams {
    /// Right ascension in decimal hours, in `[0.0, 24.0)`. Required
    /// (validated in body).
    #[serde(default)]
    pub ra: Option<f64>,
    /// Declination in decimal degrees, in `[-90.0, 90.0]`. Required
    /// (validated in body).
    #[serde(default)]
    pub dec: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetTrackingParams {
    /// New tracking state. `true` enables sidereal tracking; `false`
    /// disables it.
    pub enabled: bool,
}

/// Empty parameter struct for `get_tracking` — the tool takes no input.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetTrackingParams {}

/// Empty parameter struct for `get_mount_position` — the tool takes no input.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMountPositionParams {}

/// Empty parameter struct for `park` — the tool takes no input.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ParkParams {}

/// Empty parameter struct for `unpark` — the tool takes no input.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct UnparkParams {}

/// Empty parameter struct for `get_park_state` — the tool takes no input.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetParkStateParams {}

/// Empty parameter struct for `abort_slew` — the tool takes no input.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct AbortSlewParams {}

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

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResolveTargetParams {
    /// Object name to resolve against the embedded Messier + NGC + IC
    /// catalogue. Case- and whitespace-insensitive; common spellings
    /// (`"M 31"`, `"M31"`, `"m 31"`, `"Messier 31"`) all collide on
    /// the same key. Common-name aliases (`"Andromeda Galaxy"`,
    /// `"Crab Nebula"`) are honoured.
    pub name: String,
}

// --- Ephemeris primitives ---

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AltAzParams {
    /// Right ascension, hours `[0, 24)`, ICRS / J2000.
    pub ra: f64,
    /// Declination, degrees `[-90, 90]`, ICRS / J2000.
    pub dec: f64,
    /// UTC timestamp as RFC3339 (e.g. `"2026-05-03T22:00:00Z"`).
    /// Defaults to the server's wall clock if omitted.
    #[serde(default)]
    pub time: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TransitParams {
    pub ra: f64,
    pub dec: f64,
    /// UTC date as `YYYY-MM-DD`. The returned UT is the upper transit
    /// during this UTC date (for practical observing latitudes).
    pub date: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RiseSetParams {
    pub ra: f64,
    pub dec: f64,
    /// UTC date as `YYYY-MM-DD`.
    pub date: String,
    /// Altitude threshold (degrees). Standard amateur-rig minimum is
    /// 20°; horizon-touching rise/set uses 0° (refraction-naive).
    pub min_alt_degrees: f64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeridianFlipParams {
    pub ra: f64,
    pub dec: f64,
    /// UTC timestamp as RFC3339; defaults to "now".
    #[serde(default)]
    pub time: Option<String>,
    /// Mount's current side of pier: `"east"`, `"west"`, or
    /// `"unknown"`. v1 ignores the value but accepts it for forward
    /// compatibility.
    #[serde(default = "default_side_of_pier")]
    pub side_of_pier: String,
}

fn default_side_of_pier() -> String {
    "unknown".to_string()
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TimeOnlyParams {
    /// UTC timestamp as RFC3339; defaults to "now".
    #[serde(default)]
    pub time: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TwilightParams {
    /// UTC date as `YYYY-MM-DD` for the local night that covers it.
    pub date: String,
    /// `"civil"` (-6°), `"nautical"` (-12°), or `"astronomical"`
    /// (-18°).
    pub kind: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MoonSeparationParams {
    pub ra: f64,
    pub dec: f64,
    #[serde(default)]
    pub time: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetTargetStatusParams {
    /// Catalog name (`"M 31"`, `"NGC 224"`, common-name aliases) or
    /// a raw RA/Dec via the alternate form below — exactly one of
    /// `target_name` or (`ra` + `dec`) must be supplied.
    #[serde(default)]
    pub target_name: Option<String>,
    #[serde(default)]
    pub ra: Option<f64>,
    #[serde(default)]
    pub dec: Option<f64>,
    #[serde(default)]
    pub time: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetNextTargetParams {
    #[serde(default)]
    pub time: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMeridianStatusParams {
    #[serde(default)]
    pub time: Option<String>,
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
    /// Configured observer site, if any. `None` when the deployment
    /// has no `site` block (camera-only / flats rigs); ephemeris
    /// tools that require a site (`compute_alt_az`, `get_twilight`,
    /// etc.) error cleanly in that case.
    pub site: Option<rp_ephemeris::Site>,
    /// Targets parsed from `Config.targets` for the planner
    /// convenience tools. Empty when `targets[]` is absent or none
    /// of its rows carry the required `name` / `ra_hours` /
    /// `dec_degrees` fields.
    pub targets: Vec<crate::planner::decision::PlannerTarget>,
    /// Planner-wide minimum altitude default (degrees). Read from
    /// `Config.planner.min_altitude_degrees`, falling back to 20°
    /// when omitted.
    pub default_min_altitude_degrees: f64,
}

impl McpHandler {
    pub fn new(
        equipment: Arc<EquipmentRegistry>,
        event_bus: Arc<EventBus>,
        session_config: SessionConfig,
        image_cache: ImageCache,
        site: Option<rp_ephemeris::Site>,
    ) -> Self {
        Self {
            equipment,
            event_bus,
            session_config,
            image_cache,
            site,
            targets: Vec::new(),
            default_min_altitude_degrees: 20.0,
        }
    }

    /// Wire planner inputs after construction. The lib.rs build path
    /// calls this with the parsed `targets[]` JSON and
    /// `planner.min_altitude_degrees` (defaulting to 20°). Tests
    /// can leave the defaults as-is.
    pub fn with_planner_config(
        mut self,
        targets: Vec<crate::planner::decision::PlannerTarget>,
        default_min_altitude_degrees: f64,
    ) -> Self {
        self.targets = targets;
        self.default_min_altitude_degrees = default_min_altitude_degrees;
        self
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
}

/// Outcome variants for [`poll_slewing_until_idle`].
enum PollIdleError {
    /// Deadline expired with `slewing()` still returning `true`.
    Timeout,
    /// `slewing()` itself returned an Alpaca error.
    Read(ascom_alpaca::ASCOMError),
}

/// Poll `mount.slewing()` every 100 ms until it returns `false`,
/// bounded by a 300 s deadline. Shared by `do_slew_blocking` and
/// `do_park_blocking`. Caller decides what to do on
/// [`PollIdleError::Timeout`] (e.g. best-effort `abort_slew()` for
/// `slew`; surface the timeout for `park`).
async fn poll_slewing_until_idle(
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
    // Mount tools
    //
    // The mount is singular per `rp` deployment (piggyback rigs share
    // one mount across multiple optical trains). Tools take no
    // `mount_id` / `telescope_id` parameter — there is nothing to
    // disambiguate.
    // -------------------------------------------------------------------

    #[tool(
        description = "Slew the mount to equatorial coordinates (RA hours, Dec degrees). Blocks until the mount reports Slewing == false plus the configured / per-call settle. Tracking must be on before calling — propagates the Alpaca error otherwise."
    )]
    async fn slew(
        &self,
        Parameters(params): Parameters<SlewParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        // Body validation in input order (ra → dec → settle_after) so
        // the error message points at the first missing or out-of-range
        // field. Same convention as `auto_focus` / `measure_basic`.
        let ra = match params.ra {
            Some(v) => v,
            None => return Ok(tool_error!("missing required parameter: ra")),
        };
        if !(0.0..24.0).contains(&ra) {
            return Ok(tool_error!(
                "ra out of range: {} (must be in [0.0, 24.0))",
                ra
            ));
        }
        let dec = match params.dec {
            Some(v) => v,
            None => return Ok(tool_error!("missing required parameter: dec")),
        };
        if !(-90.0..=90.0).contains(&dec) {
            return Ok(tool_error!(
                "dec out of range: {} (must be in [-90.0, 90.0])",
                dec
            ));
        }

        // Resolve `settle_after`: explicit per-call value wins; otherwise
        // pull the mount's configured default (or zero if no mount is
        // configured — `do_slew_blocking` below calls `resolve_mount`
        // and surfaces the "no mount configured" error in that case).
        let settle_after = match params.settle_after {
            Some(d) => d,
            None => match self.equipment.find_mount() {
                Some(entry) => entry.config.settle_after_slew.unwrap_or_default(),
                None => Duration::default(),
            },
        };

        match self.do_slew_blocking(ra, dec, settle_after).await {
            Ok((actual_ra, actual_dec)) => Ok(tool_success!({
                "actual_ra": actual_ra,
                "actual_dec": actual_dec,
            })),
            Err(e) => Ok(tool_error!("{}", e)),
        }
    }

    #[tool(
        description = "Sync the mount's reported position to the given equatorial coordinates (RA hours, Dec degrees). Immediate; no polling."
    )]
    async fn sync_mount(
        &self,
        Parameters(params): Parameters<SyncMountParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let ra = match params.ra {
            Some(v) => v,
            None => return Ok(tool_error!("missing required parameter: ra")),
        };
        if !(0.0..24.0).contains(&ra) {
            return Ok(tool_error!(
                "ra out of range: {} (must be in [0.0, 24.0))",
                ra
            ));
        }
        let dec = match params.dec {
            Some(v) => v,
            None => return Ok(tool_error!("missing required parameter: dec")),
        };
        if !(-90.0..=90.0).contains(&dec) {
            return Ok(tool_error!(
                "dec out of range: {} (must be in [-90.0, 90.0])",
                dec
            ));
        }

        let (_entry, mount) = match self.resolve_mount() {
            Ok(p) => p,
            Err(e) => return Ok(tool_error!("{}", e)),
        };

        debug!(ra, dec, "syncing mount");
        match mount.sync_to_coordinates(ra, dec).await {
            Ok(()) => Ok(tool_success!({})),
            Err(e) => Ok(tool_error!("failed to sync mount: {}", e)),
        }
    }

    #[tool(description = "Read the mount's current pointing as RA (hours) / Dec (degrees).")]
    async fn get_mount_position(
        &self,
        Parameters(_params): Parameters<GetMountPositionParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (_entry, mount) = match self.resolve_mount() {
            Ok(p) => p,
            Err(e) => return Ok(tool_error!("{}", e)),
        };

        let ra = match mount.right_ascension().await {
            Ok(v) => v,
            Err(e) => return Ok(tool_error!("failed to read mount right_ascension: {}", e)),
        };
        let dec = match mount.declination().await {
            Ok(v) => v,
            Err(e) => return Ok(tool_error!("failed to read mount declination: {}", e)),
        };

        Ok(tool_success!({
            "ra": ra,
            "dec": dec,
        }))
    }

    #[tool(
        description = "Read the mount's tracking state and CanSetTracking capability. Fails loud if the Tracking read errors."
    )]
    async fn get_tracking(
        &self,
        Parameters(_params): Parameters<GetTrackingParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (_entry, mount) = match self.resolve_mount() {
            Ok(p) => p,
            Err(e) => return Ok(tool_error!("{}", e)),
        };

        let tracking = match mount.tracking().await {
            Ok(v) => v,
            Err(e) => return Ok(tool_error!("failed to read mount tracking: {}", e)),
        };
        let can_set_tracking = match mount.can_set_tracking().await {
            Ok(v) => v,
            Err(e) => return Ok(tool_error!("failed to read mount can_set_tracking: {}", e)),
        };

        Ok(tool_success!({
            "tracking": tracking,
            "can_set_tracking": can_set_tracking,
        }))
    }

    #[tool(description = "Enable or disable the mount's sidereal tracking drive.")]
    async fn set_tracking(
        &self,
        Parameters(params): Parameters<SetTrackingParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (_entry, mount) = match self.resolve_mount() {
            Ok(p) => p,
            Err(e) => return Ok(tool_error!("{}", e)),
        };

        debug!(enabled = params.enabled, "setting mount tracking");
        match mount.set_tracking(params.enabled).await {
            Ok(()) => Ok(tool_success!({})),
            Err(e) => Ok(tool_error!("failed to set tracking: {}", e)),
        }
    }

    #[tool(
        description = "Park the mount: invoke ASCOM Park, poll Slewing until idle (300 s deadline), and verify AtPark == true. Per ASCOM, a successful park clears Tracking. Unlike slew, park does NOT auto-abort on timeout — call abort_slew explicitly to interrupt a stuck park."
    )]
    async fn park(
        &self,
        Parameters(_params): Parameters<ParkParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self.do_park_blocking().await {
            Ok(()) => Ok(tool_success!({})),
            Err(e) => Ok(tool_error!("{}", e)),
        }
    }

    #[tool(
        description = "Unpark the mount. Returns immediately (no Slewing poll — most drivers just clear the AtPark flag). Does NOT auto-enable Tracking; call set_tracking explicitly before slewing."
    )]
    async fn unpark(
        &self,
        Parameters(_params): Parameters<UnparkParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (_entry, mount) = match self.resolve_mount() {
            Ok(p) => p,
            Err(e) => return Ok(tool_error!("{}", e)),
        };

        debug!("unparking mount");
        match mount.unpark().await {
            Ok(()) => Ok(tool_success!({})),
            Err(e) => Ok(tool_error!("failed to unpark: {}", e)),
        }
    }

    #[tool(
        description = "Read the mount's park state and capabilities: AtPark, CanPark, CanUnpark. Fails loud on the AtPark read error (the load-bearing field)."
    )]
    async fn get_park_state(
        &self,
        Parameters(_params): Parameters<GetParkStateParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (_entry, mount) = match self.resolve_mount() {
            Ok(p) => p,
            Err(e) => return Ok(tool_error!("{}", e)),
        };

        let at_park = match mount.at_park().await {
            Ok(v) => v,
            Err(e) => return Ok(tool_error!("failed to read mount at_park: {}", e)),
        };
        let can_park = match mount.can_park().await {
            Ok(v) => v,
            Err(e) => return Ok(tool_error!("failed to read mount can_park: {}", e)),
        };
        let can_unpark = match mount.can_unpark().await {
            Ok(v) => v,
            Err(e) => return Ok(tool_error!("failed to read mount can_unpark: {}", e)),
        };

        Ok(tool_success!({
            "at_park": at_park,
            "can_park": can_park,
            "can_unpark": can_unpark,
        }))
    }

    #[tool(
        description = "Abort an in-progress mount slew or park. Per ASCOM, only valid while Slewing == true; the natural Alpaca error propagates otherwise."
    )]
    async fn abort_slew(
        &self,
        Parameters(_params): Parameters<AbortSlewParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let (_entry, mount) = match self.resolve_mount() {
            Ok(p) => p,
            Err(e) => return Ok(tool_error!("{}", e)),
        };

        debug!("aborting mount slew");
        match mount.abort_slew().await {
            Ok(()) => Ok(tool_success!({})),
            Err(e) => Ok(tool_error!("failed to abort slew: {}", e)),
        }
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

        // Read the current focuser position + temperature exactly once
        // each (per the Contract algorithm step 1) and thread the values
        // through to both `focus_started` *and* `run_auto_focus` so the
        // event payload and the result's `temperature_c`/sweep-grid
        // origin can never disagree. Temperature is informational only:
        // any read failure (NOT_IMPLEMENTED or transient) becomes
        // `temperature_c: null`; we don't abort an auto-focus run over
        // a missing thermistor.
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
        };

        match imaging::tools::auto_focus::run_auto_focus(
            &adapter,
            &adapter,
            &adapter,
            bounds,
            starting_position,
            starting_temperature_c,
            af_params,
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

    // -------------------------------------------------------------------
    // Planner: catalog lookup
    // -------------------------------------------------------------------

    #[tool(
        description = "Resolve a deep-sky object name to ICRS coordinates from \
                       the embedded Messier + NGC + IC catalogue. Case- and \
                       whitespace-insensitive; common-name aliases are honoured. \
                       Returns ra_hours / dec_degrees / object_type / magnitude / \
                       size_arcmin on hit, or a structured not-found payload with \
                       the top three fuzzy suggestions on miss."
    )]
    async fn resolve_target(
        &self,
        Parameters(params): Parameters<ResolveTargetParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match crate::planner::catalog::resolve(&params.name) {
            crate::planner::catalog::ResolveOutcome::Resolved(view) => Ok(tool_success!({
                "name": view.name,
                "object_type": view.object_type,
                "ra_hours": view.ra_hours,
                "dec_degrees": view.dec_degrees,
                "magnitude": view.magnitude,
                "size_arcmin": view.size_arcmin,
            })),
            crate::planner::catalog::ResolveOutcome::NotFound { suggestions } => {
                // CallToolResult::error carries text content; we embed
                // a small JSON payload so a planner plugin can pick out
                // suggestions without string parsing.
                Ok(CallToolResult::error(vec![Content::text(
                    serde_json::json!({
                        "error": "target_not_found",
                        "name": params.name,
                        "suggestions": suggestions,
                    })
                    .to_string(),
                )]))
            }
        }
    }

    // -------------------------------------------------------------------
    // Planner: ephemeris primitives — see docs/services/rp.md
    // §"Primitive vs. Convenience MCP Tools"
    // -------------------------------------------------------------------

    #[tool(description = "Topocentric altitude/azimuth for an ICRS target. \
                       Refraction modelled with default amateur conditions. \
                       Requires the deployment's `site` block.")]
    async fn compute_alt_az(
        &self,
        Parameters(params): Parameters<AltAzParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let site = match self.site.as_ref() {
            Some(s) => s,
            None => {
                return Ok(tool_error!(
                    "{}",
                    crate::planner::primitives::site_required_error()
                ))
            }
        };
        let target = match crate::planner::primitives::validate_icrs(params.ra, params.dec) {
            Ok(c) => c,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let time = match crate::planner::primitives::parse_time_or_now(params.time.as_deref()) {
            Ok(t) => t,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        match crate::planner::primitives::compute_alt_az(site, target, time) {
            Ok(v) => Ok(CallToolResult::success(vec![Content::text(v.to_string())])),
            Err(e) => Ok(tool_error!("{}", e)),
        }
    }

    #[tool(description = "UT of upper transit on a given UTC date. Requires `site`.")]
    async fn compute_transit(
        &self,
        Parameters(params): Parameters<TransitParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let site = match self.site.as_ref() {
            Some(s) => s,
            None => {
                return Ok(tool_error!(
                    "{}",
                    crate::planner::primitives::site_required_error()
                ))
            }
        };
        let target = match crate::planner::primitives::validate_icrs(params.ra, params.dec) {
            Ok(c) => c,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let date = match crate::planner::primitives::parse_date(&params.date) {
            Ok(d) => d,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let v = crate::planner::primitives::compute_transit(site, target, date);
        Ok(CallToolResult::success(vec![Content::text(v.to_string())]))
    }

    #[tool(
        description = "Rise / set times above min_alt_degrees on a given UTC date. \
                       null bounds for circumpolar always-up or always-down. \
                       Requires `site`."
    )]
    async fn compute_rise_set(
        &self,
        Parameters(params): Parameters<RiseSetParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let site = match self.site.as_ref() {
            Some(s) => s,
            None => {
                return Ok(tool_error!(
                    "{}",
                    crate::planner::primitives::site_required_error()
                ))
            }
        };
        let target = match crate::planner::primitives::validate_icrs(params.ra, params.dec) {
            Ok(c) => c,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let date = match crate::planner::primitives::parse_date(&params.date) {
            Ok(d) => d,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        if !(-90.0..=90.0).contains(&params.min_alt_degrees) {
            return Ok(tool_error!(
                "min_alt_degrees must be in [-90, 90]; got {}",
                params.min_alt_degrees
            ));
        }
        let v = crate::planner::primitives::compute_rise_set(
            site,
            target,
            date,
            params.min_alt_degrees,
        );
        Ok(CallToolResult::success(vec![Content::text(v.to_string())]))
    }

    #[tool(
        description = "Time-to-flip (seconds) until the target next reaches the meridian \
                       (HA = 0). v1 ignores side_of_pier but accepts it for forward \
                       compatibility. Requires `site`."
    )]
    async fn compute_meridian_flip(
        &self,
        Parameters(params): Parameters<MeridianFlipParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let site = match self.site.as_ref() {
            Some(s) => s,
            None => {
                return Ok(tool_error!(
                    "{}",
                    crate::planner::primitives::site_required_error()
                ))
            }
        };
        let target = match crate::planner::primitives::validate_icrs(params.ra, params.dec) {
            Ok(c) => c,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let time = match crate::planner::primitives::parse_time_or_now(params.time.as_deref()) {
            Ok(t) => t,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let side = match crate::planner::primitives::parse_side_of_pier(&params.side_of_pier) {
            Ok(s) => s,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let v = crate::planner::primitives::compute_meridian_flip(site, target, time, side);
        Ok(CallToolResult::success(vec![Content::text(v.to_string())]))
    }

    #[tool(
        description = "Geocentric astrometric Sun position + topocentric alt/az. Requires `site`."
    )]
    async fn get_sun_position(
        &self,
        Parameters(params): Parameters<TimeOnlyParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let site = match self.site.as_ref() {
            Some(s) => s,
            None => {
                return Ok(tool_error!(
                    "{}",
                    crate::planner::primitives::site_required_error()
                ))
            }
        };
        let time = match crate::planner::primitives::parse_time_or_now(params.time.as_deref()) {
            Ok(t) => t,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let v = crate::planner::primitives::get_sun_position(site, time);
        Ok(CallToolResult::success(vec![Content::text(v.to_string())]))
    }

    #[tool(
        description = "Civil / nautical / astronomical twilight bounds for the local \
                       night that covers `date` (UTC). null bound at high latitudes \
                       where the Sun never crosses the threshold. Requires `site`."
    )]
    async fn get_twilight(
        &self,
        Parameters(params): Parameters<TwilightParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let site = match self.site.as_ref() {
            Some(s) => s,
            None => {
                return Ok(tool_error!(
                    "{}",
                    crate::planner::primitives::site_required_error()
                ))
            }
        };
        let date = match crate::planner::primitives::parse_date(&params.date) {
            Ok(d) => d,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let kind = match crate::planner::primitives::parse_twilight_kind(&params.kind) {
            Ok(k) => k,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let v = crate::planner::primitives::get_twilight(site, date, kind);
        Ok(CallToolResult::success(vec![Content::text(v.to_string())]))
    }

    #[tool(
        description = "Geocentric Moon position + topocentric alt/az + Sun-Moon \
                       elongation (phase) + illuminated fraction. Requires `site`."
    )]
    async fn get_moon_position(
        &self,
        Parameters(params): Parameters<TimeOnlyParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let site = match self.site.as_ref() {
            Some(s) => s,
            None => {
                return Ok(tool_error!(
                    "{}",
                    crate::planner::primitives::site_required_error()
                ))
            }
        };
        let time = match crate::planner::primitives::parse_time_or_now(params.time.as_deref()) {
            Ok(t) => t,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let v = crate::planner::primitives::get_moon_position(site, time);
        Ok(CallToolResult::success(vec![Content::text(v.to_string())]))
    }

    #[tool(
        description = "Angular separation (degrees) between an ICRS target and the \
                       Moon. Geocentric — does not depend on `site`."
    )]
    async fn compute_moon_separation(
        &self,
        Parameters(params): Parameters<MoonSeparationParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let target = match crate::planner::primitives::validate_icrs(params.ra, params.dec) {
            Ok(c) => c,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let time = match crate::planner::primitives::parse_time_or_now(params.time.as_deref()) {
            Ok(t) => t,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let v = crate::planner::primitives::compute_moon_separation(target, time);
        Ok(CallToolResult::success(vec![Content::text(v.to_string())]))
    }

    #[tool(description = "Local apparent sidereal time at the configured site. Requires `site`.")]
    async fn get_local_sidereal_time(
        &self,
        Parameters(params): Parameters<TimeOnlyParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let site = match self.site.as_ref() {
            Some(s) => s,
            None => {
                return Ok(tool_error!(
                    "{}",
                    crate::planner::primitives::site_required_error()
                ))
            }
        };
        let time = match crate::planner::primitives::parse_time_or_now(params.time.as_deref()) {
            Ok(t) => t,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let v = crate::planner::primitives::get_local_sidereal_time(site, time);
        Ok(CallToolResult::success(vec![Content::text(v.to_string())]))
    }

    // -------------------------------------------------------------------
    // Planner: convenience tools (get_target_status, get_next_target,
    // get_meridian_status) — see docs/services/rp.md §"Dynamic Planner"
    // -------------------------------------------------------------------

    #[tool(description = "Sky position + progress for a target. Accepts either \
                       target_name (resolved via the embedded catalog) or a \
                       raw ra/dec pair. progress is null in v1 (per-target \
                       record_exposure tracking is not yet wired into the \
                       planner). Requires `site`.")]
    async fn get_target_status(
        &self,
        Parameters(params): Parameters<GetTargetStatusParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let site = match self.site.as_ref() {
            Some(s) => s,
            None => {
                return Ok(tool_error!(
                    "{}",
                    crate::planner::primitives::site_required_error()
                ))
            }
        };
        let time = match crate::planner::primitives::parse_time_or_now(params.time.as_deref()) {
            Ok(t) => t,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let (target, name) = match (params.target_name.as_ref(), params.ra, params.dec) {
            (Some(name), None, None) => match crate::planner::catalog::resolve(name) {
                crate::planner::catalog::ResolveOutcome::Resolved(view) => (
                    rp_ephemeris::IcrsCoord {
                        ra_hours: view.ra_hours,
                        dec_degrees: view.dec_degrees,
                    },
                    view.name,
                ),
                crate::planner::catalog::ResolveOutcome::NotFound { suggestions } => {
                    return Ok(CallToolResult::error(vec![Content::text(
                        serde_json::json!({
                            "error": "target_not_found",
                            "name": name,
                            "suggestions": suggestions,
                        })
                        .to_string(),
                    )]));
                }
            },
            (None, Some(ra), Some(dec)) => {
                match crate::planner::primitives::validate_icrs(ra, dec) {
                    Ok(c) => (c, format!("ICRS({ra:.4}, {dec:.4})")),
                    Err(e) => return Ok(tool_error!("{}", e)),
                }
            }
            _ => {
                return Ok(tool_error!(
                    "supply exactly one of `target_name` or (`ra` + `dec`)"
                ))
            }
        };
        match crate::planner::convenience::target_status_view(
            site,
            target,
            &name,
            time,
            self.default_min_altitude_degrees,
        ) {
            Ok(v) => Ok(CallToolResult::success(vec![Content::text(v.to_string())])),
            Err(e) => Ok(tool_error!("{}", e)),
        }
    }

    #[tool(
        description = "Recommend the next target from `targets[]` config based on \
                       altitude / approaching transit / sun-elevation gating. \
                       Returns target=null and a structured reason \
                       (no_targets_configured / all_below_min_altitude / \
                       wait_for_twilight / end_of_session) when no candidate is \
                       viable. Requires `site`."
    )]
    async fn get_next_target(
        &self,
        Parameters(params): Parameters<GetNextTargetParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let site = match self.site.as_ref() {
            Some(s) => s,
            None => {
                return Ok(tool_error!(
                    "{}",
                    crate::planner::primitives::site_required_error()
                ))
            }
        };
        let time = match crate::planner::primitives::parse_time_or_now(params.time.as_deref()) {
            Ok(t) => t,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let eph = rp_ephemeris::ErfarsEphemeris::new();
        let rec = crate::planner::decision::next_target(
            &eph,
            site,
            time,
            &self.targets,
            self.default_min_altitude_degrees,
        );
        let v = crate::planner::convenience::next_target_view(rec);
        Ok(CallToolResult::success(vec![Content::text(v.to_string())]))
    }

    #[tool(
        description = "Time-to-flip plus side-of-pier for the mount's current \
                       pointing. Reads RA/Dec/SideOfPier from the configured \
                       mount, then runs the meridian-flip primitive. Requires \
                       `site` and a connected mount."
    )]
    async fn get_meridian_status(
        &self,
        Parameters(params): Parameters<GetMeridianStatusParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let site = match self.site.as_ref() {
            Some(s) => s,
            None => {
                return Ok(tool_error!(
                    "{}",
                    crate::planner::primitives::site_required_error()
                ))
            }
        };
        let time = match crate::planner::primitives::parse_time_or_now(params.time.as_deref()) {
            Ok(t) => t,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let (_entry, mount) = match self.resolve_mount() {
            Ok(p) => p,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let ra = match mount.right_ascension().await {
            Ok(v) => v,
            Err(e) => return Ok(tool_error!("failed to read mount right_ascension: {}", e)),
        };
        let dec = match mount.declination().await {
            Ok(v) => v,
            Err(e) => return Ok(tool_error!("failed to read mount declination: {}", e)),
        };
        // ASCOM SideOfPier returns `NOT_IMPLEMENTED` on mounts that
        // don't expose the property — treat that specifically as
        // `Unknown` so the flip ETA still surfaces. Any other read
        // failure (network error, transient Alpaca issue, etc.) is
        // surfaced loudly: a "valid-looking but stale" payload is
        // worse than a clean error the operator can act on.
        let side = match mount.side_of_pier().await {
            Ok(ascom_alpaca::api::telescope::PierSide::East) => rp_ephemeris::SideOfPier::East,
            Ok(ascom_alpaca::api::telescope::PierSide::West) => rp_ephemeris::SideOfPier::West,
            Ok(_) => rp_ephemeris::SideOfPier::Unknown,
            Err(e) if e.code == ascom_alpaca::ASCOMErrorCode::NOT_IMPLEMENTED => {
                rp_ephemeris::SideOfPier::Unknown
            }
            Err(e) => return Ok(tool_error!("failed to read mount side_of_pier: {}", e)),
        };
        let v = crate::planner::convenience::meridian_status_view(site, ra, dec, time, side);
        Ok(CallToolResult::success(vec![Content::text(v.to_string())]))
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
}

#[async_trait::async_trait]
impl imaging::tools::auto_focus::FocuserOps for AutoFocusAdapter<'_> {
    async fn move_to(&self, position: i32) -> std::result::Result<i32, String> {
        self.handler
            .do_move_focuser_blocking(&self.focuser_id, position)
            .await
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
        fail_pixel_size: bool,
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
            if self.fail_pixel_size {
                return Err(ASCOMError::invalid_operation("pixel size unavailable"));
            }
            Ok(3.76)
        }

        async fn pixel_size_y(&self) -> ascom_alpaca::ASCOMResult<f64> {
            if self.fail_pixel_size {
                return Err(ASCOMError::invalid_operation("pixel size unavailable"));
            }
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
    // MockTelescope — single configurable mock for Telescope (mount).
    //
    // Defaults are "happy path" (capable, tracking on, returns a fixed
    // RA/Dec). Set fail_* fields to inject errors per test, or set
    // tracking_value / can_set_tracking_value / ra_value / dec_value to
    // shape the read responses.
    // -----------------------------------------------------------------------

    struct MockTelescope {
        fail_slew: bool,
        fail_slewing_poll: bool,
        fail_sync: bool,
        fail_right_ascension: bool,
        fail_declination: bool,
        fail_tracking: bool,
        fail_can_set_tracking: bool,
        fail_set_tracking: bool,
        fail_park: bool,
        fail_unpark: bool,
        fail_at_park: bool,
        fail_can_park: bool,
        fail_can_unpark: bool,
        fail_abort_slew: bool,
        /// `slewing()` returns `true` forever — drives the timeout path.
        stuck_slewing: bool,
        tracking_value: bool,
        can_set_tracking_value: bool,
        at_park_value: bool,
        can_park_value: bool,
        can_unpark_value: bool,
        ra_value: f64,
        dec_value: f64,
    }

    impl Default for MockTelescope {
        fn default() -> Self {
            Self {
                fail_slew: false,
                fail_slewing_poll: false,
                fail_sync: false,
                fail_right_ascension: false,
                fail_declination: false,
                fail_tracking: false,
                fail_can_set_tracking: false,
                fail_set_tracking: false,
                fail_park: false,
                fail_unpark: false,
                fail_at_park: false,
                fail_can_park: false,
                fail_can_unpark: false,
                fail_abort_slew: false,
                stuck_slewing: false,
                tracking_value: true,
                can_set_tracking_value: true,
                at_park_value: false,
                can_park_value: true,
                can_unpark_value: true,
                ra_value: 0.0,
                dec_value: 0.0,
            }
        }
    }

    impl_mock_device!(MockTelescope);

    #[async_trait::async_trait]
    impl ascom_alpaca::api::Telescope for MockTelescope {
        async fn at_home(&self) -> ascom_alpaca::ASCOMResult<bool> {
            Ok(false)
        }

        async fn at_park(&self) -> ascom_alpaca::ASCOMResult<bool> {
            if self.fail_at_park {
                return Err(ASCOMError::invalid_operation("at_park read failed"));
            }
            Ok(self.at_park_value)
        }

        async fn can_park(&self) -> ascom_alpaca::ASCOMResult<bool> {
            if self.fail_can_park {
                return Err(ASCOMError::invalid_operation("can_park read failed"));
            }
            Ok(self.can_park_value)
        }

        async fn can_unpark(&self) -> ascom_alpaca::ASCOMResult<bool> {
            if self.fail_can_unpark {
                return Err(ASCOMError::invalid_operation("can_unpark read failed"));
            }
            Ok(self.can_unpark_value)
        }

        async fn park(&self) -> ascom_alpaca::ASCOMResult<()> {
            if self.fail_park {
                return Err(ASCOMError::invalid_operation("park failed"));
            }
            Ok(())
        }

        async fn unpark(&self) -> ascom_alpaca::ASCOMResult<()> {
            if self.fail_unpark {
                return Err(ASCOMError::invalid_operation("unpark failed"));
            }
            Ok(())
        }

        async fn declination(&self) -> ascom_alpaca::ASCOMResult<f64> {
            if self.fail_declination {
                return Err(ASCOMError::invalid_operation("encoder fault"));
            }
            Ok(self.dec_value)
        }

        async fn declination_rate(&self) -> ascom_alpaca::ASCOMResult<f64> {
            Ok(0.0)
        }

        async fn equatorial_system(
            &self,
        ) -> ascom_alpaca::ASCOMResult<ascom_alpaca::api::telescope::EquatorialCoordinateType>
        {
            Ok(ascom_alpaca::api::telescope::EquatorialCoordinateType::J2000)
        }

        async fn right_ascension(&self) -> ascom_alpaca::ASCOMResult<f64> {
            if self.fail_right_ascension {
                return Err(ASCOMError::invalid_operation("encoder fault"));
            }
            Ok(self.ra_value)
        }

        async fn right_ascension_rate(&self) -> ascom_alpaca::ASCOMResult<f64> {
            Ok(0.0)
        }

        async fn sidereal_time(&self) -> ascom_alpaca::ASCOMResult<f64> {
            Ok(0.0)
        }

        async fn tracking(&self) -> ascom_alpaca::ASCOMResult<bool> {
            if self.fail_tracking {
                return Err(ASCOMError::invalid_operation("tracking read failed"));
            }
            Ok(self.tracking_value)
        }

        async fn set_tracking(&self, _: bool) -> ascom_alpaca::ASCOMResult<()> {
            if self.fail_set_tracking {
                return Err(ASCOMError::invalid_operation("CanSetTracking is false"));
            }
            Ok(())
        }

        async fn can_set_tracking(&self) -> ascom_alpaca::ASCOMResult<bool> {
            if self.fail_can_set_tracking {
                return Err(ASCOMError::invalid_operation("capability read failed"));
            }
            Ok(self.can_set_tracking_value)
        }

        async fn tracking_rate(
            &self,
        ) -> ascom_alpaca::ASCOMResult<ascom_alpaca::api::telescope::DriveRate> {
            Ok(ascom_alpaca::api::telescope::DriveRate::Sidereal)
        }

        async fn axis_rates(
            &self,
            _axis: ascom_alpaca::api::telescope::TelescopeAxis,
        ) -> ascom_alpaca::ASCOMResult<Vec<std::ops::RangeInclusive<f64>>> {
            Ok(vec![])
        }

        async fn utc_date(&self) -> ascom_alpaca::ASCOMResult<std::time::SystemTime> {
            Ok(std::time::SystemTime::UNIX_EPOCH)
        }

        async fn slewing(&self) -> ascom_alpaca::ASCOMResult<bool> {
            if self.fail_slewing_poll {
                return Err(ASCOMError::invalid_operation("slewing poll failed"));
            }
            Ok(self.stuck_slewing)
        }

        async fn abort_slew(&self) -> ascom_alpaca::ASCOMResult<()> {
            if self.fail_abort_slew {
                return Err(ASCOMError::invalid_operation("abort_slew failed"));
            }
            Ok(())
        }

        async fn slew_to_coordinates_async(
            &self,
            _ra: f64,
            _dec: f64,
        ) -> ascom_alpaca::ASCOMResult<()> {
            if self.fail_slew {
                return Err(ASCOMError::invalid_operation("Tracking is off"));
            }
            Ok(())
        }

        async fn sync_to_coordinates(&self, _ra: f64, _dec: f64) -> ascom_alpaca::ASCOMResult<()> {
            if self.fail_sync {
                return Err(ASCOMError::invalid_operation("sync failed"));
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
            None,
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
        camera_registry_with_focal_length(cam, None)
    }

    fn camera_registry_with_focal_length(
        cam: Arc<dyn ascom_alpaca::api::Camera>,
        focal_length_mm: Option<f64>,
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
                    focal_length_mm,
                    auth: None,
                },
                device: Some(cam),
            }],
            filter_wheels: vec![],
            cover_calibrators: vec![],
            focusers: vec![],
            mount: None,
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
            mount: None,
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
            mount: None,
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
            mount: None,
        }
    }

    fn mount_registry(
        mount: Arc<dyn ascom_alpaca::api::Telescope>,
        settle_after_slew: Option<Duration>,
    ) -> crate::equipment::EquipmentRegistry {
        crate::equipment::EquipmentRegistry {
            cameras: vec![],
            filter_wheels: vec![],
            cover_calibrators: vec![],
            focusers: vec![],
            mount: Some(crate::equipment::MountEntry {
                connected: true,
                config: crate::config::MountConfig {
                    alpaca_url: "http://localhost:1".to_string(),
                    device_number: 0,
                    settle_after_slew,
                    auth: None,
                },
                device: Some(mount),
            }),
        }
    }

    fn empty_registry() -> crate::equipment::EquipmentRegistry {
        crate::equipment::EquipmentRegistry {
            cameras: vec![],
            filter_wheels: vec![],
            cover_calibrators: vec![],
            focusers: vec![],
            mount: None,
        }
    }

    fn disconnected_mount_registry() -> crate::equipment::EquipmentRegistry {
        crate::equipment::EquipmentRegistry {
            cameras: vec![],
            filter_wheels: vec![],
            cover_calibrators: vec![],
            focusers: vec![],
            mount: Some(crate::equipment::MountEntry {
                connected: false,
                config: crate::config::MountConfig {
                    alpaca_url: "http://localhost:1".to_string(),
                    device_number: 0,
                    settle_after_slew: None,
                    auth: None,
                },
                device: None,
            }),
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
            None,
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
            None,
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
            None,
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
    // capture — optics block in sidecar
    // -----------------------------------------------------------------------

    async fn capture_and_read_sidecar(
        registry: crate::equipment::EquipmentRegistry,
    ) -> ExposureDocument {
        let temp = tempfile::tempdir().unwrap();
        let cache = ImageCache::new(64, 4, std::path::PathBuf::from("/nonexistent"));
        let handler = McpHandler::new(
            Arc::new(registry),
            Arc::new(crate::events::EventBus::from_config(&[])),
            SessionConfig {
                data_directory: temp.path().to_string_lossy().to_string(),
            },
            cache,
            None,
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
        let image_path = json["image_path"].as_str().unwrap().to_string();
        let sidecar = persistence::sidecar_path(&image_path);
        let doc = persistence::read_sidecar_sync(&sidecar).unwrap();
        // Explicit drop pins the TempDir lifetime past the sidecar read
        // — without it the borrow checker is happy but the temp dir could
        // be cleaned up at any drop point the optimizer chose.
        drop(temp);
        doc
    }

    #[tokio::test]
    async fn test_capture_persists_optics_when_focal_length_configured() {
        // Mock returns 3.76 µm pixels and 1024×1024 sensor; with a 1000 mm
        // focal length the derivation gives:
        //   pixel_scale = 206.265 × 3.76 / 1000 ≈ 0.7755564 arcsec/px
        //   fov         = 0.7755564 × 1024 / 3600 ≈ 0.220603 deg
        let cam = MockCamera::default();
        let registry = camera_registry_with_focal_length(Arc::new(cam), Some(1000.0));
        let doc = capture_and_read_sidecar(registry).await;
        let optics = doc.optics.expect("optics block should be present");
        assert_eq!(optics.focal_length_mm, 1000.0);
        assert_eq!(optics.pixel_size_x_um, 3.76);
        assert_eq!(optics.pixel_size_y_um, 3.76);
        assert_eq!(optics.sensor_width_px, 1024);
        assert_eq!(optics.sensor_height_px, 1024);
        assert!(
            (optics.pixel_scale_x_arcsec_per_pixel - 0.7755564).abs() < 1e-6,
            "pixel_scale_x = {}",
            optics.pixel_scale_x_arcsec_per_pixel
        );
        assert!(
            (optics.fov_height_deg - 0.220603).abs() < 1e-4,
            "fov_height_deg = {}",
            optics.fov_height_deg
        );
    }

    #[tokio::test]
    async fn test_capture_omits_optics_when_focal_length_missing() {
        let cam = MockCamera::default();
        let registry = camera_registry_with_focal_length(Arc::new(cam), None);
        let doc = capture_and_read_sidecar(registry).await;
        assert!(
            doc.optics.is_none(),
            "optics must be omitted when focal_length_mm is not configured"
        );
    }

    #[tokio::test]
    async fn test_capture_omits_optics_when_pixel_size_unavailable() {
        let cam = MockCamera {
            fail_pixel_size: true,
            ..Default::default()
        };
        let registry = camera_registry_with_focal_length(Arc::new(cam), Some(1000.0));
        let doc = capture_and_read_sidecar(registry).await;
        assert!(
            doc.optics.is_none(),
            "optics must be omitted when pixel size read fails"
        );
    }

    #[tokio::test]
    async fn test_capture_omits_optics_when_sensor_size_unavailable() {
        let cam = MockCamera {
            fail_camera_size: true,
            ..Default::default()
        };
        let registry = camera_registry_with_focal_length(Arc::new(cam), Some(1000.0));
        let doc = capture_and_read_sidecar(registry).await;
        assert!(
            doc.optics.is_none(),
            "optics must be omitted when sensor size read fails"
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
                mount: None,
            }),
            Arc::new(crate::events::EventBus::from_config(&[])),
            SessionConfig {
                data_directory: temp.path().to_string_lossy().to_string(),
            },
            cache.clone(),
            None,
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
            optics: None,
            sections: serde_json::Map::new(),
        };
        let cached = CachedPixels::from_i32_pixels(vec![1, 2, 3, 4], (2, 2), 65535);

        handler
            .persist_capture_artifact(doc, cached, Some(65535))
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
            mount: None,
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
            mount: None,
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
            mount: None,
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

    // -----------------------------------------------------------------------
    // Mount tool tests — slew / sync_mount / get_mount_position /
    // get_tracking / set_tracking. Singular mount, no mount_id parameter.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_slew_success() {
        let mount = MockTelescope {
            ra_value: 10.6847,
            dec_value: 41.2689,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler
            .slew(Parameters(SlewParams {
                ra: Some(10.6847),
                dec: Some(41.2689),
                settle_after: None,
            }))
            .await
            .unwrap();
        let json = ok_text(result);
        assert_eq!(json["actual_ra"], 10.6847);
        assert_eq!(json["actual_dec"], 41.2689);
    }

    #[tokio::test]
    async fn test_slew_no_mount_configured() {
        let handler = test_handler(empty_registry());
        let result = handler
            .slew(Parameters(SlewParams {
                ra: Some(0.0),
                dec: Some(0.0),
                settle_after: None,
            }))
            .await;
        assert_tool_error(result, "no mount configured");
    }

    #[tokio::test]
    async fn test_slew_mount_not_connected() {
        let handler = test_handler(disconnected_mount_registry());
        let result = handler
            .slew(Parameters(SlewParams {
                ra: Some(0.0),
                dec: Some(0.0),
                settle_after: None,
            }))
            .await;
        assert_tool_error(result, "mount not connected");
    }

    #[tokio::test]
    async fn test_slew_missing_ra() {
        let handler = test_handler(mount_registry(Arc::new(MockTelescope::default()), None));
        let result = handler
            .slew(Parameters(SlewParams {
                ra: None,
                dec: Some(0.0),
                settle_after: None,
            }))
            .await;
        assert_tool_error(result, "missing required parameter: ra");
    }

    #[tokio::test]
    async fn test_slew_missing_dec() {
        let handler = test_handler(mount_registry(Arc::new(MockTelescope::default()), None));
        let result = handler
            .slew(Parameters(SlewParams {
                ra: Some(0.0),
                dec: None,
                settle_after: None,
            }))
            .await;
        assert_tool_error(result, "missing required parameter: dec");
    }

    #[tokio::test]
    async fn test_slew_ra_out_of_range() {
        let handler = test_handler(mount_registry(Arc::new(MockTelescope::default()), None));
        let result = handler
            .slew(Parameters(SlewParams {
                ra: Some(25.0),
                dec: Some(0.0),
                settle_after: None,
            }))
            .await;
        assert_tool_error(result, "ra out of range");
    }

    #[tokio::test]
    async fn test_slew_dec_out_of_range() {
        let handler = test_handler(mount_registry(Arc::new(MockTelescope::default()), None));
        let result = handler
            .slew(Parameters(SlewParams {
                ra: Some(0.0),
                dec: Some(91.0),
                settle_after: None,
            }))
            .await;
        assert_tool_error(result, "dec out of range");
    }

    /// Models the ASCOM `InvalidOperationException` that fires when
    /// `Tracking == false` and the caller invokes
    /// `SlewToCoordinatesAsync` — the natural error path the design
    /// explicitly chose over a magical `ensure_tracking` parameter.
    #[tokio::test]
    async fn test_slew_alpaca_error_propagates() {
        let mount = MockTelescope {
            fail_slew: true,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler
            .slew(Parameters(SlewParams {
                ra: Some(0.0),
                dec: Some(0.0),
                settle_after: None,
            }))
            .await;
        assert_tool_error(result, "failed to slew");
    }

    /// Drives the timeout escalation path: `slewing()` returns `true`
    /// indefinitely, the 300 s deadline expires, `abort_slew()` is
    /// called (best-effort, ignored result), and the tool returns the
    /// timeout error. `start_paused` lets tokio auto-advance virtual
    /// time so the test runs in real-time milliseconds.
    #[tokio::test(start_paused = true)]
    async fn test_slew_timeout_returns_error_after_abort() {
        let mount = MockTelescope {
            stuck_slewing: true,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler
            .slew(Parameters(SlewParams {
                ra: Some(0.0),
                dec: Some(0.0),
                settle_after: None,
            }))
            .await;
        assert_tool_error(result, "timeout waiting for mount to settle");
    }

    /// Per-call `settle_after` overrides the config default. Passes
    /// `Duration::ZERO` to skip an otherwise-non-zero config value;
    /// behavior of the actual sleep is exercised in BDD where
    /// wall-clock timing is observable.
    #[tokio::test]
    async fn test_slew_per_call_settle_overrides_config() {
        let mount = MockTelescope::default();
        let handler = test_handler(mount_registry(
            Arc::new(mount),
            Some(Duration::from_secs(60)),
        ));
        let result = handler
            .slew(Parameters(SlewParams {
                ra: Some(0.0),
                dec: Some(0.0),
                settle_after: Some(Duration::ZERO),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_sync_mount_success() {
        let mount = MockTelescope::default();
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler
            .sync_mount(Parameters(SyncMountParams {
                ra: Some(0.0),
                dec: Some(0.0),
            }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_sync_mount_no_mount_configured() {
        let handler = test_handler(empty_registry());
        let result = handler
            .sync_mount(Parameters(SyncMountParams {
                ra: Some(0.0),
                dec: Some(0.0),
            }))
            .await;
        assert_tool_error(result, "no mount configured");
    }

    #[tokio::test]
    async fn test_sync_mount_alpaca_error() {
        let mount = MockTelescope {
            fail_sync: true,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler
            .sync_mount(Parameters(SyncMountParams {
                ra: Some(0.0),
                dec: Some(0.0),
            }))
            .await;
        assert_tool_error(result, "failed to sync mount");
    }

    #[tokio::test]
    async fn test_sync_mount_ra_out_of_range() {
        let handler = test_handler(mount_registry(Arc::new(MockTelescope::default()), None));
        let result = handler
            .sync_mount(Parameters(SyncMountParams {
                ra: Some(-1.0),
                dec: Some(0.0),
            }))
            .await;
        assert_tool_error(result, "ra out of range");
    }

    #[tokio::test]
    async fn test_get_mount_position_returns_ra_dec() {
        let mount = MockTelescope {
            ra_value: 12.5,
            dec_value: -23.4,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler
            .get_mount_position(Parameters(GetMountPositionParams {}))
            .await
            .unwrap();
        let json = ok_text(result);
        assert_eq!(json["ra"], 12.5);
        assert_eq!(json["dec"], -23.4);
    }

    #[tokio::test]
    async fn test_get_mount_position_no_mount() {
        let handler = test_handler(empty_registry());
        let result = handler
            .get_mount_position(Parameters(GetMountPositionParams {}))
            .await;
        assert_tool_error(result, "no mount configured");
    }

    #[tokio::test]
    async fn test_get_mount_position_ra_read_fails() {
        let mount = MockTelescope {
            fail_right_ascension: true,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler
            .get_mount_position(Parameters(GetMountPositionParams {}))
            .await;
        assert_tool_error(result, "failed to read mount right_ascension");
    }

    #[tokio::test]
    async fn test_get_tracking_returns_state_and_capability() {
        let mount = MockTelescope {
            tracking_value: true,
            can_set_tracking_value: true,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler
            .get_tracking(Parameters(GetTrackingParams {}))
            .await
            .unwrap();
        let json = ok_text(result);
        assert_eq!(json["tracking"], true);
        assert_eq!(json["can_set_tracking"], true);
    }

    /// Mount that reports `CanSetTracking == false` — surfaces in the
    /// tool result rather than failing the call. Workflows can read
    /// the field and decide whether to continue.
    #[tokio::test]
    async fn test_get_tracking_surfaces_can_set_tracking_false() {
        let mount = MockTelescope {
            tracking_value: false,
            can_set_tracking_value: false,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler
            .get_tracking(Parameters(GetTrackingParams {}))
            .await
            .unwrap();
        let json = ok_text(result);
        assert_eq!(json["tracking"], false);
        assert_eq!(json["can_set_tracking"], false);
    }

    /// Per the design decision: fail loud on `Tracking` read errors;
    /// don't try to half-succeed by returning `can_set_tracking` alone.
    #[tokio::test]
    async fn test_get_tracking_fails_when_tracking_read_errors() {
        let mount = MockTelescope {
            fail_tracking: true,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler.get_tracking(Parameters(GetTrackingParams {})).await;
        assert_tool_error(result, "failed to read mount tracking");
    }

    #[tokio::test]
    async fn test_set_tracking_enables() {
        let mount = MockTelescope::default();
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler
            .set_tracking(Parameters(SetTrackingParams { enabled: true }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_set_tracking_disables() {
        let mount = MockTelescope::default();
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler
            .set_tracking(Parameters(SetTrackingParams { enabled: false }))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    /// Models a mount that responds to `set_tracking` with an
    /// `InvalidOperationException` (e.g. `CanSetTracking == false`).
    /// The error propagates with the friendly prefix.
    #[tokio::test]
    async fn test_set_tracking_alpaca_error() {
        let mount = MockTelescope {
            fail_set_tracking: true,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler
            .set_tracking(Parameters(SetTrackingParams { enabled: true }))
            .await;
        assert_tool_error(result, "failed to set tracking");
    }

    // -----------------------------------------------------------------------
    // Mount park / unpark / get_park_state / abort_slew tests.
    // Singular mount, no params on any of these tools.
    // -----------------------------------------------------------------------

    /// Mock's `at_park()` returns true immediately, so the polling
    /// loop exits on the first iteration.
    #[tokio::test]
    async fn test_park_success() {
        let mount = MockTelescope {
            at_park_value: true,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler.park(Parameters(ParkParams {})).await.unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_park_no_mount_configured() {
        let handler = test_handler(empty_registry());
        let result = handler.park(Parameters(ParkParams {})).await;
        assert_tool_error(result, "no mount configured");
    }

    #[tokio::test]
    async fn test_park_mount_not_connected() {
        let handler = test_handler(disconnected_mount_registry());
        let result = handler.park(Parameters(ParkParams {})).await;
        assert_tool_error(result, "mount not connected");
    }

    #[tokio::test]
    async fn test_park_alpaca_error_propagates() {
        let mount = MockTelescope {
            fail_park: true,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler.park(Parameters(ParkParams {})).await;
        assert_tool_error(result, "failed to park");
    }

    /// 300 s deadline expires while `at_park()` keeps returning `false`.
    /// Unlike `slew`, `park` does NOT auto-abort — it surfaces the
    /// timeout and lets the caller decide. `start_paused` lets tokio
    /// auto-advance virtual time so the test runs in real-time
    /// milliseconds.
    #[tokio::test(start_paused = true)]
    async fn test_park_timeout_does_not_auto_abort() {
        let mount = MockTelescope {
            at_park_value: false,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler.park(Parameters(ParkParams {})).await;
        assert_tool_error(result, "timeout waiting for mount to park");
    }

    #[tokio::test]
    async fn test_unpark_success() {
        let mount = MockTelescope {
            at_park_value: true,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler.unpark(Parameters(UnparkParams {})).await.unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_unpark_no_mount_configured() {
        let handler = test_handler(empty_registry());
        let result = handler.unpark(Parameters(UnparkParams {})).await;
        assert_tool_error(result, "no mount configured");
    }

    #[tokio::test]
    async fn test_unpark_alpaca_error() {
        let mount = MockTelescope {
            fail_unpark: true,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler.unpark(Parameters(UnparkParams {})).await;
        assert_tool_error(result, "failed to unpark");
    }

    #[tokio::test]
    async fn test_get_park_state_returns_all_fields() {
        let mount = MockTelescope {
            at_park_value: true,
            can_park_value: true,
            can_unpark_value: true,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler
            .get_park_state(Parameters(GetParkStateParams {}))
            .await
            .unwrap();
        let json = ok_text(result);
        assert_eq!(json["at_park"], true);
        assert_eq!(json["can_park"], true);
        assert_eq!(json["can_unpark"], true);
    }

    /// Per the design decision: fail loud on `at_park` read errors;
    /// don't try to half-succeed by returning `can_park` alone.
    #[tokio::test]
    async fn test_get_park_state_fails_when_at_park_read_errors() {
        let mount = MockTelescope {
            fail_at_park: true,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler
            .get_park_state(Parameters(GetParkStateParams {}))
            .await;
        assert_tool_error(result, "failed to read mount at_park");
    }

    #[tokio::test]
    async fn test_get_park_state_fails_when_can_park_read_errors() {
        let mount = MockTelescope {
            fail_can_park: true,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler
            .get_park_state(Parameters(GetParkStateParams {}))
            .await;
        assert_tool_error(result, "failed to read mount can_park");
    }

    #[tokio::test]
    async fn test_get_park_state_fails_when_can_unpark_read_errors() {
        let mount = MockTelescope {
            fail_can_unpark: true,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler
            .get_park_state(Parameters(GetParkStateParams {}))
            .await;
        assert_tool_error(result, "failed to read mount can_unpark");
    }

    /// `park()` succeeds, but the very first `at_park()` poll errors
    /// — covers the `Err` arm of the polling loop. The previous
    /// implementation polled `Slewing` and then verified `AtPark`
    /// separately; now both arms collapse into the single at_park
    /// poll error path.
    #[tokio::test]
    async fn test_park_at_park_poll_fails() {
        let mount = MockTelescope {
            fail_at_park: true,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler.park(Parameters(ParkParams {})).await;
        assert_tool_error(result, "error polling mount at_park");
    }

    /// `do_slew_blocking` polls `Slewing`; this covers the
    /// `PollIdleError::Read` arm.
    #[tokio::test]
    async fn test_slew_polling_error_propagates() {
        let mount = MockTelescope {
            fail_slewing_poll: true,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler
            .slew(Parameters(SlewParams {
                ra: Some(0.0),
                dec: Some(0.0),
                settle_after: None,
            }))
            .await;
        assert_tool_error(result, "error polling mount slewing");
    }

    #[tokio::test]
    async fn test_abort_slew_success() {
        let mount = MockTelescope::default();
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler
            .abort_slew(Parameters(AbortSlewParams {}))
            .await
            .unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn test_abort_slew_no_mount_configured() {
        let handler = test_handler(empty_registry());
        let result = handler.abort_slew(Parameters(AbortSlewParams {})).await;
        assert_tool_error(result, "no mount configured");
    }

    /// Models a mount that returns `InvalidOperation` from `abort_slew`
    /// (e.g. when not currently slewing). The error propagates with
    /// the friendly prefix.
    #[tokio::test]
    async fn test_abort_slew_alpaca_error() {
        let mount = MockTelescope {
            fail_abort_slew: true,
            ..Default::default()
        };
        let handler = test_handler(mount_registry(Arc::new(mount), None));
        let result = handler.abort_slew(Parameters(AbortSlewParams {})).await;
        assert_tool_error(result, "failed to abort slew");
    }

    // -----------------------------------------------------------------------
    // Planner tools — error paths
    //
    // The new ephemeris/planner tools added in Phases 5-7 share two common
    // failure shapes: missing site config (10 of the 12 tools require it)
    // and parameter validation (range / format). One unit test per branch
    // is enough to pin the wiring; the math itself is covered by the
    // primitives.rs / decision.rs unit tests.
    // -----------------------------------------------------------------------

    fn test_handler_with_site(site: rp_ephemeris::Site) -> McpHandler {
        McpHandler::new(
            Arc::new(empty_registry()),
            Arc::new(crate::events::EventBus::from_config(&[])),
            SessionConfig {
                data_directory: std::env::temp_dir()
                    .join("rp-planner-unit-test")
                    .to_string_lossy()
                    .to_string(),
            },
            ImageCache::new(64, 4, std::path::PathBuf::from("/nonexistent")),
            Some(site),
        )
    }

    fn test_site() -> rp_ephemeris::Site {
        rp_ephemeris::Site::new(51.0786, -0.2944).unwrap()
    }

    #[tokio::test]
    async fn compute_alt_az_errors_when_site_absent() {
        let h = test_handler(empty_registry());
        let r = h
            .compute_alt_az(Parameters(AltAzParams {
                ra: 0.7,
                dec: 41.0,
                time: None,
            }))
            .await;
        assert_tool_error(r, "site not configured");
    }

    #[tokio::test]
    async fn compute_alt_az_errors_on_out_of_range_inputs() {
        let h = test_handler_with_site(test_site());
        let r = h
            .compute_alt_az(Parameters(AltAzParams {
                ra: 30.0,
                dec: 0.0,
                time: None,
            }))
            .await;
        assert_tool_error(r, "ra_hours");
    }

    #[tokio::test]
    async fn compute_alt_az_errors_on_bad_time() {
        let h = test_handler_with_site(test_site());
        let r = h
            .compute_alt_az(Parameters(AltAzParams {
                ra: 0.0,
                dec: 0.0,
                time: Some("not a time".into()),
            }))
            .await;
        assert_tool_error(r, "RFC3339");
    }

    #[tokio::test]
    async fn compute_transit_errors_when_site_absent() {
        let h = test_handler(empty_registry());
        let r = h
            .compute_transit(Parameters(TransitParams {
                ra: 0.0,
                dec: 0.0,
                date: "2026-05-03".into(),
            }))
            .await;
        assert_tool_error(r, "site not configured");
    }

    #[tokio::test]
    async fn compute_transit_errors_on_bad_date() {
        let h = test_handler_with_site(test_site());
        let r = h
            .compute_transit(Parameters(TransitParams {
                ra: 0.0,
                dec: 0.0,
                date: "tomorrow".into(),
            }))
            .await;
        assert_tool_error(r, "YYYY-MM-DD");
    }

    #[tokio::test]
    async fn compute_rise_set_errors_on_out_of_range_min_alt() {
        let h = test_handler_with_site(test_site());
        let r = h
            .compute_rise_set(Parameters(RiseSetParams {
                ra: 0.0,
                dec: 0.0,
                date: "2026-05-03".into(),
                min_alt_degrees: 200.0,
            }))
            .await;
        assert_tool_error(r, "min_alt_degrees");
    }

    #[tokio::test]
    async fn compute_rise_set_errors_when_site_absent() {
        let h = test_handler(empty_registry());
        let r = h
            .compute_rise_set(Parameters(RiseSetParams {
                ra: 0.0,
                dec: 0.0,
                date: "2026-05-03".into(),
                min_alt_degrees: 0.0,
            }))
            .await;
        assert_tool_error(r, "site not configured");
    }

    #[tokio::test]
    async fn compute_meridian_flip_errors_when_site_absent() {
        let h = test_handler(empty_registry());
        let r = h
            .compute_meridian_flip(Parameters(MeridianFlipParams {
                ra: 0.0,
                dec: 0.0,
                time: None,
                side_of_pier: "unknown".into(),
            }))
            .await;
        assert_tool_error(r, "site not configured");
    }

    #[tokio::test]
    async fn compute_meridian_flip_errors_on_bad_side_of_pier() {
        let h = test_handler_with_site(test_site());
        let r = h
            .compute_meridian_flip(Parameters(MeridianFlipParams {
                ra: 0.0,
                dec: 0.0,
                time: None,
                side_of_pier: "middle".into(),
            }))
            .await;
        assert_tool_error(r, "side_of_pier");
    }

    #[tokio::test]
    async fn get_sun_position_errors_when_site_absent() {
        let h = test_handler(empty_registry());
        let r = h
            .get_sun_position(Parameters(TimeOnlyParams { time: None }))
            .await;
        assert_tool_error(r, "site not configured");
    }

    #[tokio::test]
    async fn get_twilight_errors_when_site_absent() {
        let h = test_handler(empty_registry());
        let r = h
            .get_twilight(Parameters(TwilightParams {
                date: "2026-12-21".into(),
                kind: "civil".into(),
            }))
            .await;
        assert_tool_error(r, "site not configured");
    }

    #[tokio::test]
    async fn get_moon_position_errors_when_site_absent() {
        let h = test_handler(empty_registry());
        let r = h
            .get_moon_position(Parameters(TimeOnlyParams { time: None }))
            .await;
        assert_tool_error(r, "site not configured");
    }

    #[tokio::test]
    async fn compute_moon_separation_errors_on_bad_inputs() {
        let h = test_handler(empty_registry());
        let r = h
            .compute_moon_separation(Parameters(MoonSeparationParams {
                ra: 100.0,
                dec: 0.0,
                time: None,
            }))
            .await;
        assert_tool_error(r, "ra_hours");
    }

    #[tokio::test]
    async fn get_local_sidereal_time_errors_when_site_absent() {
        let h = test_handler(empty_registry());
        let r = h
            .get_local_sidereal_time(Parameters(TimeOnlyParams { time: None }))
            .await;
        assert_tool_error(r, "site not configured");
    }

    #[tokio::test]
    async fn get_target_status_errors_when_site_absent() {
        let h = test_handler(empty_registry());
        let r = h
            .get_target_status(Parameters(GetTargetStatusParams {
                target_name: Some("M 31".into()),
                ra: None,
                dec: None,
                time: None,
            }))
            .await;
        assert_tool_error(r, "site not configured");
    }

    #[tokio::test]
    async fn get_target_status_errors_on_unknown_name() {
        let h = test_handler_with_site(test_site());
        let r = h
            .get_target_status(Parameters(GetTargetStatusParams {
                target_name: Some("M 999".into()),
                ra: None,
                dec: None,
                time: None,
            }))
            .await;
        // The catalog miss path returns a structured `target_not_found`
        // payload as a CallToolResult::error.
        let call_result = r.expect("tool returned protocol error");
        assert!(call_result.is_error.unwrap_or(false));
        let text = call_result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap();
        assert!(text.contains("target_not_found"), "got: {text}");
    }

    #[tokio::test]
    async fn get_target_status_errors_when_neither_name_nor_radec_supplied() {
        let h = test_handler_with_site(test_site());
        let r = h
            .get_target_status(Parameters(GetTargetStatusParams {
                target_name: None,
                ra: None,
                dec: None,
                time: None,
            }))
            .await;
        assert_tool_error(r, "supply exactly one");
    }

    #[tokio::test]
    async fn get_target_status_accepts_radec_form() {
        let h = test_handler_with_site(test_site());
        let r = h
            .get_target_status(Parameters(GetTargetStatusParams {
                target_name: None,
                ra: Some(2.5301944),
                dec: Some(89.2641111),
                time: None,
            }))
            .await
            .expect("tool returned protocol error");
        assert!(!r.is_error.unwrap_or(false), "expected success");
    }

    #[tokio::test]
    async fn get_next_target_errors_when_site_absent() {
        let h = test_handler(empty_registry());
        let r = h
            .get_next_target(Parameters(GetNextTargetParams { time: None }))
            .await;
        assert_tool_error(r, "site not configured");
    }

    #[tokio::test]
    async fn get_next_target_with_no_targets_returns_no_targets_configured() {
        let h = test_handler_with_site(test_site());
        let r = h
            .get_next_target(Parameters(GetNextTargetParams { time: None }))
            .await
            .expect("tool returned protocol error");
        let text = r
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap();
        assert!(text.contains("no_targets_configured"), "got: {text}");
    }

    #[tokio::test]
    async fn get_meridian_status_errors_when_site_absent() {
        let h = test_handler(empty_registry());
        let r = h
            .get_meridian_status(Parameters(GetMeridianStatusParams { time: None }))
            .await;
        assert_tool_error(r, "site not configured");
    }

    #[tokio::test]
    async fn get_meridian_status_errors_when_mount_absent() {
        let h = test_handler_with_site(test_site());
        let r = h
            .get_meridian_status(Parameters(GetMeridianStatusParams { time: None }))
            .await;
        // empty_registry has no mount, so `resolve_mount` returns the
        // standard "mount not configured" error.
        assert_tool_error(r, "mount");
    }

    // -----------------------------------------------------------------------
    // Planner tools — happy paths (cover the success-return arms in mcp.rs;
    // value correctness is covered by primitives.rs / convenience.rs unit
    // tests).
    // -----------------------------------------------------------------------

    fn handler_with_site_and_mount() -> McpHandler {
        let mock = MockTelescope::default();
        let mount_cfg = crate::config::MountConfig {
            alpaca_url: "http://unused".into(),
            device_number: 0,
            settle_after_slew: None,
            auth: None,
        };
        // Skip the connect-time HTTP fetch by hand-building a registry
        // with the mock device wired in directly.
        let registry = crate::equipment::EquipmentRegistry {
            cameras: vec![],
            filter_wheels: vec![],
            cover_calibrators: vec![],
            focusers: vec![],
            mount: Some(crate::equipment::MountEntry {
                connected: true,
                config: mount_cfg,
                device: Some(Arc::new(mock)),
            }),
        };
        McpHandler::new(
            Arc::new(registry),
            Arc::new(crate::events::EventBus::from_config(&[])),
            SessionConfig {
                data_directory: std::env::temp_dir()
                    .join("rp-planner-happy-test")
                    .to_string_lossy()
                    .to_string(),
            },
            ImageCache::new(64, 4, std::path::PathBuf::from("/nonexistent")),
            Some(test_site()),
        )
    }

    /// Yank the JSON payload from a successful CallToolResult.
    fn ok_json(r: Result<CallToolResult, rmcp::ErrorData>) -> serde_json::Value {
        let call_result = r.expect("tool returned protocol error");
        assert!(
            !call_result.is_error.unwrap_or(false),
            "expected success, got error: {:?}",
            call_result
        );
        let text = call_result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .expect("expected text content");
        serde_json::from_str(&text).expect("response was not valid JSON")
    }

    const TEST_TIME: &str = "2026-05-03T22:00:00Z";

    #[tokio::test]
    async fn compute_alt_az_happy_path() {
        let h = test_handler_with_site(test_site());
        let v = ok_json(
            h.compute_alt_az(Parameters(AltAzParams {
                ra: 2.5301944,
                dec: 89.2641111,
                time: Some(TEST_TIME.into()),
            }))
            .await,
        );
        assert!(v["altitude_degrees"].as_f64().is_some());
        assert!(v["azimuth_degrees"].as_f64().is_some());
    }

    #[tokio::test]
    async fn compute_transit_happy_path() {
        let h = test_handler_with_site(test_site());
        let v = ok_json(
            h.compute_transit(Parameters(TransitParams {
                ra: 0.7123,
                dec: 41.27,
                date: "2026-11-01".into(),
            }))
            .await,
        );
        assert!(v.get("transit_utc").is_some());
    }

    #[tokio::test]
    async fn compute_rise_set_happy_path() {
        let h = test_handler_with_site(test_site());
        let v = ok_json(
            h.compute_rise_set(Parameters(RiseSetParams {
                ra: 0.7123,
                dec: 41.27,
                date: "2026-11-01".into(),
                min_alt_degrees: 30.0,
            }))
            .await,
        );
        assert!(v.get("rise_utc").is_some());
        assert!(v.get("set_utc").is_some());
    }

    #[tokio::test]
    async fn compute_meridian_flip_happy_path() {
        let h = test_handler_with_site(test_site());
        let v = ok_json(
            h.compute_meridian_flip(Parameters(MeridianFlipParams {
                ra: 0.7123,
                dec: 41.27,
                time: Some(TEST_TIME.into()),
                side_of_pier: "east".into(),
            }))
            .await,
        );
        assert!(v["time_to_flip_seconds"].as_i64().is_some());
    }

    #[tokio::test]
    async fn get_sun_position_happy_path() {
        let h = test_handler_with_site(test_site());
        let v = ok_json(
            h.get_sun_position(Parameters(TimeOnlyParams {
                time: Some(TEST_TIME.into()),
            }))
            .await,
        );
        assert!(v["ra_hours"].as_f64().is_some());
        assert!(v["dec_degrees"].as_f64().is_some());
    }

    #[tokio::test]
    async fn get_twilight_happy_path() {
        let h = test_handler_with_site(test_site());
        let v = ok_json(
            h.get_twilight(Parameters(TwilightParams {
                date: "2026-12-21".into(),
                kind: "civil".into(),
            }))
            .await,
        );
        assert_eq!(v["kind"], "civil");
    }

    #[tokio::test]
    async fn get_moon_position_happy_path() {
        let h = test_handler_with_site(test_site());
        let v = ok_json(
            h.get_moon_position(Parameters(TimeOnlyParams {
                time: Some(TEST_TIME.into()),
            }))
            .await,
        );
        assert!(v["phase_degrees"].as_f64().is_some());
        assert!(v["illumination_fraction"].as_f64().is_some());
    }

    #[tokio::test]
    async fn compute_moon_separation_happy_path() {
        let h = test_handler_with_site(test_site());
        let v = ok_json(
            h.compute_moon_separation(Parameters(MoonSeparationParams {
                ra: 0.7123,
                dec: 41.27,
                time: Some(TEST_TIME.into()),
            }))
            .await,
        );
        let sep = v["separation_degrees"].as_f64().unwrap();
        assert!((0.0..=180.0).contains(&sep));
    }

    #[tokio::test]
    async fn get_local_sidereal_time_happy_path() {
        let h = test_handler_with_site(test_site());
        let v = ok_json(
            h.get_local_sidereal_time(Parameters(TimeOnlyParams {
                time: Some(TEST_TIME.into()),
            }))
            .await,
        );
        let lst = v["lst_hours"].as_f64().unwrap();
        assert!((0.0..24.0).contains(&lst));
    }

    #[tokio::test]
    async fn get_target_status_happy_path_via_catalog() {
        let h = test_handler_with_site(test_site());
        let v = ok_json(
            h.get_target_status(Parameters(GetTargetStatusParams {
                target_name: Some("M 31".into()),
                ra: None,
                dec: None,
                time: Some(TEST_TIME.into()),
            }))
            .await,
        );
        assert_eq!(v["target_name"], "M 31");
        assert!(v["altitude_degrees"].as_f64().is_some());
    }

    #[tokio::test]
    async fn get_meridian_status_happy_path() {
        // MockTelescope doesn't implement side_of_pier, which returns
        // NOT_IMPLEMENTED — get_meridian_status maps that to "unknown"
        // and surfaces the JSON. Exercises the success arm + the
        // NOT_IMPLEMENTED → Unknown branch in one shot.
        let h = handler_with_site_and_mount();
        let v = ok_json(
            h.get_meridian_status(Parameters(GetMeridianStatusParams {
                time: Some(TEST_TIME.into()),
            }))
            .await,
        );
        assert!(v["time_to_flip_seconds"].is_number());
        assert_eq!(v["side_of_pier"], "unknown");
        assert!(v["mount_ra_hours"].as_f64().is_some());
    }
}
