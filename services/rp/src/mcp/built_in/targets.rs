//! Target store MCP tools (`docs/services/rp.md` § Target Store):
//! `add_target` / `get_target` / `list_targets` / `update_target` /
//! `delete_target` / `set_goals`, backed by [`rp_targets::TargetStore`].
//!
//! Coexists with the legacy `targets[]` planner tools
//! ([`super::planner`]) during the P1 migration — see
//! `crate::config::target_store` for how the two share the `targets`
//! config key. Progress derivation here always reports `good: 0,
//! total: 0`: the on-disk frame scan needs both the grading plugin's
//! sidecar shape and `capture`'s target linkage, neither of which has
//! landed yet (`docs/crates/rp-targets.md` § MVP scope).

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use rp_targets::{AcquisitionGoal, Target, TargetSlug, TargetStore};

use crate::equipment::EquipmentRegistry;
use crate::planner::goal_wire::{format_exposure, parse_goal, GoalWire};

use super::super::handler::McpHandler;
use super::super::{tool_error, tool_success};

/// Coordinates are the same object when within Decision 3's dedup
/// tolerance (~10 arcmin, `docs/plans/planetarium-target-import.md`).
/// A flat, `cos(dec)`-weighted approximation rather than a full
/// great-circle formula — at this scale the two are indistinguishable,
/// and every case this guards against (a genuine re-add vs. a framing
/// a degree away) is nowhere near the boundary.
const DEDUP_TOLERANCE_DEGREES: f64 = 10.0 / 60.0;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddTargetParams {
    /// Catalog name, resolved via `resolve_target`. Mutually exclusive
    /// with `display_name`.
    #[serde(default)]
    pub catalog_ref: Option<String>,
    /// Custom target name. Requires `ra_hours` + `dec_degrees`.
    /// Mutually exclusive with `catalog_ref`.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Required with `display_name`; optional with `catalog_ref` to
    /// override the catalog centroid with a precisely-framed pointing.
    #[serde(default)]
    pub ra_hours: Option<f64>,
    #[serde(default)]
    pub dec_degrees: Option<f64>,
    #[serde(default = "default_active")]
    pub active: bool,
    /// Defaults to `targets.default_goals` from config when omitted.
    #[serde(default)]
    pub goals: Option<Vec<GoalWire>>,
    /// Per-target scheduling overrides (Decision 9 — altitude-gating
    /// parity, `docs/plans/planetarium-target-import.md`). Omitted
    /// fields fall back to `targets.default_scheduling` from config.
    #[serde(default)]
    pub scheduling: Option<SchedulingWire>,
    #[serde(default)]
    pub notes: Option<String>,
}

fn default_active() -> bool {
    true
}

/// The wire shape of `add_target`/`update_target`'s `scheduling`
/// parameter — field-for-field [`rp_targets::SchedulingConstraints`],
/// restated here (rather than deriving `JsonSchema` on the crate type
/// directly) because `rp-targets` carries no `schemars` dependency, the
/// same reasoning [`GoalWire`] follows for goals.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
pub struct SchedulingWire {
    #[serde(default)]
    pub min_altitude_degrees: Option<f64>,
    #[serde(default)]
    pub min_moon_separation_degrees: Option<f64>,
    #[serde(default)]
    pub max_moon_illumination_fraction: Option<f64>,
    #[serde(default)]
    pub meridian_window_hours: Option<f64>,
}

impl From<SchedulingWire> for rp_targets::SchedulingConstraints {
    fn from(w: SchedulingWire) -> Self {
        Self {
            min_altitude_degrees: w.min_altitude_degrees,
            min_moon_separation_degrees: w.min_moon_separation_degrees,
            max_moon_illumination_fraction: w.max_moon_illumination_fraction,
            meridian_window_hours: w.meridian_window_hours,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetTargetParams {
    pub slug: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListTargetsParams {
    #[serde(default)]
    pub active_only: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateTargetParams {
    pub slug: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub ra_hours: Option<f64>,
    #[serde(default)]
    pub dec_degrees: Option<f64>,
    #[serde(default)]
    pub active: Option<bool>,
    #[serde(default)]
    pub priority: Option<i32>,
    /// Replaces the target's scheduling overrides wholesale when
    /// present (not a field-wise merge — omit the whole parameter to
    /// leave the existing overrides untouched).
    #[serde(default)]
    pub scheduling: Option<SchedulingWire>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteTargetParams {
    pub slug: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetGoalsParams {
    pub slug: String,
    pub goals: Vec<GoalWire>,
}

#[tool_router(router = tool_router_targets, vis = "pub")]
impl McpHandler {
    #[tool(description = "Create or upsert a target. Supply exactly one of \
                       catalog_ref (resolved via the embedded catalog) or \
                       display_name + ra_hours + dec_degrees. The slug is \
                       derived from catalog_ref, or a kebab-cased \
                       display_name, and resolved against the store: \
                       absent -> created; present and the same object \
                       (coordinates within ~10 arcmin) -> in-place update \
                       (created: false, slug unchanged); present and a \
                       different object -> a suffixed slug is allocated. \
                       goals[] defaults to targets.default_goals from \
                       config when omitted; every goal's filter must be \
                       in the connected rig's configured filter roster. \
                       scheduling overrides fall back field-wise to \
                       targets.default_scheduling from config when \
                       omitted (Decision 9 — altitude-gating parity).")]
    pub(crate) async fn add_target(
        &self,
        Parameters(params): Parameters<AddTargetParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let Some(store) = self.target_store.as_ref() else {
            return Ok(tool_error!("target store not configured"));
        };

        let (
            display_name,
            ra_hours,
            dec_degrees,
            catalog_ref,
            object_type,
            magnitude,
            size_arcmin,
            base_slug_input,
        ) = match (&params.catalog_ref, &params.display_name) {
            (Some(_), Some(_)) | (None, None) => {
                return Ok(tool_error!(
                    "supply exactly one of catalog_ref or display_name"
                ))
            }
            (Some(cat), None) => {
                let resolved = match crate::planner::catalog::resolve(cat) {
                    crate::planner::catalog::ResolveOutcome::Resolved(v) => v,
                    crate::planner::catalog::ResolveOutcome::NotFound { suggestions } => {
                        return Ok(CallToolResult::error(vec![ContentBlock::text(
                            json!({
                                "error": "target_not_found",
                                "name": cat,
                                "suggestions": suggestions,
                            })
                            .to_string(),
                        )]));
                    }
                };
                let (ra, dec) = match (params.ra_hours, params.dec_degrees) {
                    (Some(ra), Some(dec)) => (ra, dec),
                    (None, None) => (resolved.ra_hours, resolved.dec_degrees),
                    _ => {
                        return Ok(tool_error!(
                            "ra_hours and dec_degrees must both be supplied together"
                        ))
                    }
                };
                (
                    resolved.name.clone(),
                    ra,
                    dec,
                    Some(resolved.name.clone()),
                    Some(resolved.object_type.clone()),
                    resolved.magnitude,
                    resolved.size_arcmin,
                    resolved.name.clone(),
                )
            }
            (None, Some(name)) => {
                let (Some(ra), Some(dec)) = (params.ra_hours, params.dec_degrees) else {
                    return Ok(tool_error!(
                        "the display_name form requires ra_hours and dec_degrees"
                    ));
                };
                (name.clone(), ra, dec, None, None, None, None, name.clone())
            }
        };

        let base_slug_str = match &catalog_ref {
            Some(_) => base_slug_input,
            None => kebab_slug_candidate(&base_slug_input),
        };
        let base_slug = match TargetSlug::new(&base_slug_str) {
            Ok(s) => s,
            Err(e) => return Ok(tool_error!("{}", e)),
        };

        let existing = match store.get_target(&base_slug).await {
            Ok(v) => v,
            Err(e) => return Ok(tool_error!("target store error: {}", e)),
        };
        let (final_slug, created) = match existing {
            None => (base_slug, true),
            Some(existing)
                if same_object(
                    ra_hours,
                    dec_degrees,
                    existing.ra_hours,
                    existing.dec_degrees,
                ) =>
            {
                (base_slug, false)
            }
            Some(_) => match allocate_suffix(store.as_ref(), &base_slug).await {
                Ok(s) => (s, true),
                Err(e) => return Ok(tool_error!("target store error: {}", e)),
            },
        };

        let goals = match &params.goals {
            Some(wire_goals) => {
                let mut parsed = Vec::with_capacity(wire_goals.len());
                for g in wire_goals {
                    match parse_goal(g) {
                        Ok(pg) => parsed.push(pg),
                        Err(e) => return Ok(tool_error!("{}", e)),
                    }
                }
                parsed
            }
            None => self.target_store_defaults.default_goals.clone(),
        };
        if let Err(e) = validate_goal_filters(&self.equipment, &goals) {
            return Ok(tool_error!("{}", e));
        }

        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let target = Target {
            slug: final_slug,
            display_name,
            ra_hours,
            dec_degrees,
            catalog_ref,
            object_type,
            magnitude,
            size_arcmin,
            priority: 0,
            active: params.active,
            goals,
            scheduling: params.scheduling.map(Into::into),
            grading: None,
            notes: params.notes,
            created_at: now.clone(),
            updated_at: now,
        };
        if let Err(e) = store.upsert_target(target.clone()).await {
            return Ok(tool_error!("target store error: {}", e));
        }

        Ok(tool_success!({
            "slug": target.slug.as_str(),
            "created": created,
            "target": target_to_json(&target),
        }))
    }

    #[tool(description = "Fetch one target with derived progress \
                       (per-goal {filter, binning, exposure, good, total, \
                       desired} — good/total are always 0 until the \
                       on-disk frame scan lands).")]
    pub(crate) async fn get_target(
        &self,
        Parameters(params): Parameters<GetTargetParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let Some(store) = self.target_store.as_ref() else {
            return Ok(tool_error!("target store not configured"));
        };
        let slug = match TargetSlug::new(&params.slug) {
            Ok(s) => s,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        match store.get_target(&slug).await {
            Ok(Some(t)) => Ok(tool_success!({
                "target": target_to_json(&t),
                "progress": progress_for(&t),
            })),
            Ok(None) => Ok(tool_error!("no target with slug {:?}", params.slug)),
            Err(e) => Ok(tool_error!("target store error: {}", e)),
        }
    }

    #[tool(description = "List all targets, each with derived progress, \
                       optionally filtered to active == true.")]
    pub(crate) async fn list_targets(
        &self,
        Parameters(params): Parameters<ListTargetsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let Some(store) = self.target_store.as_ref() else {
            return Ok(tool_error!("target store not configured"));
        };
        let mut targets = match store.list_targets().await {
            Ok(v) => v,
            Err(e) => return Ok(tool_error!("target store error: {}", e)),
        };
        if params.active_only == Some(true) {
            targets.retain(|t| t.active);
        }
        let items: Vec<Value> = targets
            .iter()
            .map(|t| {
                let mut v = target_to_json(t);
                v["progress"] = json!(progress_for(t));
                v
            })
            .collect();
        Ok(tool_success!({ "targets": items }))
    }

    #[tool(description = "Edit a target's fields in place. Does not touch \
                       the slug or on-disk frames. Setting active: true is \
                       how a pending target is accepted into rotation. \
                       scheduling, when supplied, replaces the whole \
                       overrides object (not a field-wise merge).")]
    pub(crate) async fn update_target(
        &self,
        Parameters(params): Parameters<UpdateTargetParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let Some(store) = self.target_store.as_ref() else {
            return Ok(tool_error!("target store not configured"));
        };
        let slug = match TargetSlug::new(&params.slug) {
            Ok(s) => s,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let mut target = match store.get_target(&slug).await {
            Ok(Some(t)) => t,
            Ok(None) => return Ok(tool_error!("no target with slug {:?}", params.slug)),
            Err(e) => return Ok(tool_error!("target store error: {}", e)),
        };
        if let Some(v) = params.display_name {
            target.display_name = v;
        }
        if let Some(v) = params.ra_hours {
            target.ra_hours = v;
        }
        if let Some(v) = params.dec_degrees {
            target.dec_degrees = v;
        }
        if let Some(v) = params.active {
            target.active = v;
        }
        if let Some(v) = params.priority {
            target.priority = v;
        }
        if let Some(v) = params.scheduling {
            target.scheduling = Some(v.into());
        }
        if params.notes.is_some() {
            target.notes = params.notes;
        }
        target.updated_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        if let Err(e) = store.upsert_target(target.clone()).await {
            return Ok(tool_error!("target store error: {}", e));
        }
        Ok(tool_success!({ "target": target_to_json(&target) }))
    }

    #[tool(description = "Remove a target's plan row (deleted: false for \
                       an absent slug). Frames already captured under the \
                       slug are left untouched on disk.")]
    pub(crate) async fn delete_target(
        &self,
        Parameters(params): Parameters<DeleteTargetParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let Some(store) = self.target_store.as_ref() else {
            return Ok(tool_error!("target store not configured"));
        };
        let slug = match TargetSlug::new(&params.slug) {
            Ok(s) => s,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        match store.delete_target(&slug).await {
            Ok(deleted) => Ok(tool_success!({ "deleted": deleted })),
            Err(e) => Ok(tool_error!("target store error: {}", e)),
        }
    }

    #[tool(description = "Replace a target's goal set atomically (not a \
                       merge). Same filter-roster validation as \
                       add_target.")]
    pub(crate) async fn set_goals(
        &self,
        Parameters(params): Parameters<SetGoalsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let Some(store) = self.target_store.as_ref() else {
            return Ok(tool_error!("target store not configured"));
        };
        let slug = match TargetSlug::new(&params.slug) {
            Ok(s) => s,
            Err(e) => return Ok(tool_error!("{}", e)),
        };
        let mut goals = Vec::with_capacity(params.goals.len());
        for g in &params.goals {
            match parse_goal(g) {
                Ok(pg) => goals.push(pg),
                Err(e) => return Ok(tool_error!("{}", e)),
            }
        }
        if let Err(e) = validate_goal_filters(&self.equipment, &goals) {
            return Ok(tool_error!("{}", e));
        }
        if let Err(e) = store.set_goals(&slug, goals).await {
            return Ok(tool_error!("target store error: {}", e));
        }
        match store.get_target(&slug).await {
            Ok(Some(t)) => Ok(tool_success!({ "target": target_to_json(&t) })),
            Ok(None) => Ok(tool_error!("no target with slug {:?}", params.slug)),
            Err(e) => Ok(tool_error!("target store error: {}", e)),
        }
    }
}

/// Turns an operator-typed display name into a base-slug candidate:
/// lower-cased words joined with hyphens (`"Comet Test"` -> `"comet-test"`).
/// Distinct from `TargetSlug::new`'s own whitespace-*stripping*
/// normalization (which suits compact catalog names like `"NGC 7000"` ->
/// `"ngc7000"`, per `rp-targets.md` § Identity) — an operator-typed name
/// reads better hyphenated. Catalog adds bypass this and go straight to
/// `TargetSlug::new`.
fn kebab_slug_candidate(display_name: &str) -> String {
    display_name
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

fn same_object(ra1_hours: f64, dec1_degrees: f64, ra2_hours: f64, dec2_degrees: f64) -> bool {
    let ra_deg_diff = (ra1_hours - ra2_hours) * 15.0 * dec1_degrees.to_radians().cos();
    let dec_diff = dec1_degrees - dec2_degrees;
    ra_deg_diff.hypot(dec_diff) < DEDUP_TOLERANCE_DEGREES
}

/// Lowest unused `"{base}-{n}"` for `n` from 2. Terminates by the
/// pigeonhole principle (rp-targets.md § Slug allocation): the store
/// holds finitely many targets, so some suffix is always free.
async fn allocate_suffix(store: &dyn TargetStore, base: &TargetSlug) -> Result<TargetSlug, String> {
    let mut n: u32 = 2;
    loop {
        let candidate = TargetSlug::new(&format!("{base}-{n}")).map_err(|e| e.to_string())?;
        match store.get_target(&candidate).await {
            Ok(None) => return Ok(candidate),
            Ok(Some(_)) => n += 1,
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Every goal's filter must be in the union of every configured filter
/// wheel's declared roster (rp.md § Target Store — validated against
/// config, not live device state, so this never touches hardware). No
/// filter wheel configured at all is permissive (nothing to validate
/// against).
fn validate_goal_filters(
    equipment: &EquipmentRegistry,
    goals: &[AcquisitionGoal],
) -> Result<(), String> {
    let roster: Vec<&str> = equipment
        .filter_wheels
        .iter()
        .flat_map(|fw| fw.config.filters.iter().map(String::as_str))
        .collect();
    if roster.is_empty() {
        return Ok(());
    }
    for g in goals {
        if !roster.contains(&g.filter.as_str()) {
            return Err(format!(
                "goal filter {:?} is not in the configured filter roster {:?}",
                g.filter, roster
            ));
        }
    }
    Ok(())
}

fn target_to_json(t: &Target) -> Value {
    json!({
        "slug": t.slug.as_str(),
        "display_name": t.display_name,
        "ra_hours": t.ra_hours,
        "dec_degrees": t.dec_degrees,
        "catalog_ref": t.catalog_ref,
        "object_type": t.object_type,
        "magnitude": t.magnitude,
        "size_arcmin": t.size_arcmin,
        "priority": t.priority,
        "active": t.active,
        "goals": t.goals.iter().map(crate::planner::goal_wire::goal_to_json).collect::<Vec<_>>(),
        "scheduling": t.scheduling.map(|s| serde_json::to_value(s).unwrap_or(Value::Null)),
        "notes": t.notes,
        "created_at": t.created_at,
        "updated_at": t.updated_at,
    })
}

/// Per-goal `{filter, binning, exposure, good, total, desired}`
/// (rp.md § Progress derivation). `good`/`total` are hard-coded 0 —
/// the on-disk scan this needs is out of scope until the grading
/// plugin's sidecar shape and capture's target linkage both land.
fn progress_for(t: &Target) -> Vec<Value> {
    t.goals
        .iter()
        .map(|g| {
            json!({
                "filter": g.filter,
                "binning": g.binning.to_string(),
                "exposure": format_exposure(g.exposure),
                "good": 0,
                "total": 0,
                "desired": g.desired_count,
            })
        })
        .collect()
}
