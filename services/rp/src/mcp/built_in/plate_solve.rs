use std::time::Duration;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use tracing::debug;

use super::super::handler::McpHandler;
use super::super::{tool_error, tool_success};

/// Nested pointing-hint object passed to `plate_solve`. The
/// both-or-neither contract for ra/dec is structural — supplying
/// only one of `ra_deg` / `dec_deg` is a serde-level deserialization
/// error rather than a runtime check. Field names carry units to
/// preempt the Alpaca-hours / wrapper-degrees gotcha at the call
/// site.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PointingHint {
    pub ra_deg: f64,
    pub dec_deg: f64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PlateSolveParams {
    /// Exposure-document id; resolved through the unified image+document
    /// cache. Wins over `image_path` when both are supplied. Either this
    /// or `image_path` must be present.
    #[serde(default)]
    pub document_id: Option<String>,
    /// FITS file on disk (read by the wrapper). Either this or
    /// `document_id` must be present.
    #[serde(default)]
    pub image_path: Option<String>,
    /// Explicit pointing hint (decimal degrees). Mutually exclusive with
    /// `use_mount_hints=true`.
    #[serde(default)]
    pub pointing_hint: Option<PointingHint>,
    /// When `true`, source the pointing hint from the configured mount.
    /// Mutually exclusive with `pointing_hint`. Requires a connected
    /// mount.
    #[serde(default)]
    pub use_mount_hints: Option<bool>,
    /// Image FOV hint in decimal degrees (matches ASTAP's `-fov`).
    #[serde(default)]
    pub fov_hint_deg: Option<f64>,
    /// Per-call search radius override. Wins over the rp-config default.
    #[serde(default)]
    pub search_radius_deg: Option<f64>,
    /// Per-call solve timeout (humantime string). Forwarded to the
    /// wrapper's `timeout` field.
    #[serde(default, with = "humantime_serde::option")]
    #[schemars(with = "Option<String>")]
    pub timeout: Option<Duration>,
}

#[tool_router(router = tool_router_plate_solve, vis = "pub")]
impl McpHandler {
    #[tool(
        description = "Plate-solve a captured frame by proxying to the plate-solver rp-managed service. Accepts document_id or image_path; document_id wins. Hints are explicit (pointing_hint object) or sourced from the mount via use_mount_hints=true. Persists a `wcs` section to the exposure document when one resolves."
    )]
    pub(crate) async fn plate_solve(
        &self,
        Parameters(params): Parameters<PlateSolveParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        if params.document_id.is_none() && params.image_path.is_none() {
            return Ok(tool_error!(
                "missing required argument: provide either document_id or image_path"
            ));
        }

        // Hint validation: pointing_hint and use_mount_hints=true are
        // mutually exclusive. Validated up front so a caller mixing
        // both gets the same error regardless of mount state.
        let use_mount_hints = params.use_mount_hints.unwrap_or(false);
        if params.pointing_hint.is_some() && use_mount_hints {
            return Ok(tool_error!(
                "plate_solve: provide explicit pointing_hint or use_mount_hints, not both"
            ));
        }

        let client = match self.plate_solver.as_ref() {
            Some(c) => c.clone(),
            None => return Ok(tool_error!("plate_solve: plate solver not configured")),
        };

        // Resolve fits_path: document_id wins when both supplied.
        let (fits_path, target_doc_id) = match params.document_id.as_deref() {
            Some(doc_id) => match self.image_cache.resolve_document(doc_id).await {
                Some(doc) => (doc.file_path.clone(), Some(doc_id.to_string())),
                None => return Ok(tool_error!("plate_solve: document not found: {}", doc_id)),
            },
            None => {
                let path = params
                    .image_path
                    .as_deref()
                    .expect("image_path is Some when document_id is None (validated above)");
                (path.to_string(), None)
            }
        };

        // Resolve hints. The wrapper takes flat ra_hint/dec_hint in
        // decimal degrees; rp converts the mount's Alpaca hours by
        // ×15 here so the conversion lives in exactly one place.
        let (ra_hint, dec_hint) = if let Some(p) = params.pointing_hint.as_ref() {
            (Some(p.ra_deg), Some(p.dec_deg))
        } else if use_mount_hints {
            match self.read_mount_hints_for_plate_solve().await {
                Ok((ra_deg, dec_deg)) => (Some(ra_deg), Some(dec_deg)),
                Err(e) => {
                    return Ok(tool_error!(
                        "plate_solve: use_mount_hints requested but {}",
                        e
                    ))
                }
            }
        } else {
            (None, None)
        };

        // search_radius_deg: per-call value > config default > absent.
        let search_radius_deg = params
            .search_radius_deg
            .or(self.plate_solver_default_search_radius_deg);

        let request = rp_plate_solver::SolveRequest {
            fits_path: fits_path.clone(),
            ra_hint,
            dec_hint,
            fov_hint_deg: params.fov_hint_deg,
            search_radius_deg,
            timeout: params.timeout,
        };

        let outcome = match client.solve(request).await {
            Ok(o) => o,
            Err(rp_plate_solver::SolveError::ServiceUnreachable(reason)) => {
                return Ok(tool_error!("plate_solve: service unreachable: {}", reason));
            }
            Err(rp_plate_solver::SolveError::Wrapper {
                code,
                message,
                details,
            }) => {
                if details.is_null() {
                    return Ok(tool_error!("plate_solve: {}: {}", code, message));
                }
                return Ok(tool_error!(
                    "plate_solve: {}: {} (details: {})",
                    code,
                    message,
                    details
                ));
            }
            Err(rp_plate_solver::SolveError::Internal(reason)) => {
                return Ok(tool_error!("plate_solve: internal: {}", reason));
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

        Ok(tool_success!({
            "ra_center": outcome.ra_center,
            "dec_center": outcome.dec_center,
            "pixel_scale_arcsec": outcome.pixel_scale_arcsec,
            "rotation_deg": outcome.rotation_deg,
            "solver": outcome.solver,
        }))
    }
}
