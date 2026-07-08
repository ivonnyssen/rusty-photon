use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use super::super::handler::McpHandler;
use super::super::{tool_error, tool_success};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResolveTargetParams {
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AltAzParams {
    pub ra: f64,
    pub dec: f64,
    #[serde(default)]
    pub time: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TransitParams {
    pub ra: f64,
    pub dec: f64,
    pub date: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RiseSetParams {
    pub ra: f64,
    pub dec: f64,
    pub date: String,
    pub min_alt_degrees: f64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeridianFlipParams {
    pub ra: f64,
    pub dec: f64,
    #[serde(default)]
    pub time: Option<String>,
    /// One of `"east"`, `"west"`, `"unknown"`. v1 ignores the value
    /// (the meridian-flip primitive is symmetric in side-of-pier) but
    /// requires the field be present, with a `"unknown"` default.
    #[serde(default = "default_side_of_pier")]
    pub side_of_pier: String,
}

fn default_side_of_pier() -> String {
    "unknown".to_string()
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TimeOnlyParams {
    #[serde(default)]
    pub time: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TwilightParams {
    pub date: String,
    /// One of `"civil"`, `"nautical"`, `"astronomical"`.
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
    /// Catalog name or alias. Mutually exclusive with `ra`+`dec`.
    #[serde(default)]
    pub target_name: Option<String>,
    /// Direct ICRS RA in decimal hours. Must be paired with `dec`.
    #[serde(default)]
    pub ra: Option<f64>,
    /// Direct ICRS Dec in decimal degrees. Must be paired with `ra`.
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
pub struct RecordExposureParams {
    /// Name of a configured `targets[]` entry.
    pub target: String,
    /// Filter of the completed frame. Omit (or pass `null` / `""`)
    /// for an unfiltered frame.
    #[serde(default)]
    pub filter: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetSessionProgressParams {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMeridianStatusParams {
    #[serde(default)]
    pub time: Option<String>,
}

#[tool_router(router = tool_router_planner, vis = "pub")]
impl McpHandler {
    // -------------------------------------------------------------------
    // Catalog lookup
    // -------------------------------------------------------------------

    #[tool(
        description = "Resolve a deep-sky object name to ICRS coordinates from \
                       the embedded Messier + NGC + IC catalogue. Case- and \
                       whitespace-insensitive; common-name aliases are honoured. \
                       Returns ra_hours / dec_degrees / object_type / magnitude / \
                       size_arcmin on hit, or a structured not-found payload with \
                       the top three fuzzy suggestions on miss."
    )]
    pub(crate) async fn resolve_target(
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
                Ok(CallToolResult::error(vec![ContentBlock::text(
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
    // Ephemeris primitives — see docs/services/rp.md
    // §"Primitive vs. Convenience MCP Tools"
    // -------------------------------------------------------------------

    #[tool(description = "Topocentric altitude/azimuth for an ICRS target. \
                       Refraction modelled with default amateur conditions. \
                       Requires the deployment's `site` block.")]
    pub(crate) async fn compute_alt_az(
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
            Ok(v) => Ok(CallToolResult::success(vec![ContentBlock::text(
                v.to_string(),
            )])),
            Err(e) => Ok(tool_error!("{}", e)),
        }
    }

    #[tool(description = "UT of upper transit on a given UTC date. Requires `site`.")]
    pub(crate) async fn compute_transit(
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
        Ok(CallToolResult::success(vec![ContentBlock::text(
            v.to_string(),
        )]))
    }

    #[tool(
        description = "Rise / set times above min_alt_degrees on a given UTC date. \
                       null bounds for circumpolar always-up or always-down. \
                       Requires `site`."
    )]
    pub(crate) async fn compute_rise_set(
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
        Ok(CallToolResult::success(vec![ContentBlock::text(
            v.to_string(),
        )]))
    }

    #[tool(
        description = "Time-to-flip (seconds) until the target next reaches the meridian \
                       (HA = 0). v1 ignores side_of_pier but accepts it for forward \
                       compatibility. Requires `site`."
    )]
    pub(crate) async fn compute_meridian_flip(
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
        Ok(CallToolResult::success(vec![ContentBlock::text(
            v.to_string(),
        )]))
    }

    #[tool(
        description = "Geocentric astrometric Sun position + topocentric alt/az. Requires `site`."
    )]
    pub(crate) async fn get_sun_position(
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
        Ok(CallToolResult::success(vec![ContentBlock::text(
            v.to_string(),
        )]))
    }

    #[tool(
        description = "Civil / nautical / astronomical twilight bounds for the local \
                       night that covers `date` (UTC). null bound at high latitudes \
                       where the Sun never crosses the threshold. Requires `site`."
    )]
    pub(crate) async fn get_twilight(
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
        Ok(CallToolResult::success(vec![ContentBlock::text(
            v.to_string(),
        )]))
    }

    #[tool(
        description = "Geocentric Moon position + topocentric alt/az + Sun-Moon \
                       elongation (phase) + illuminated fraction. Requires `site`."
    )]
    pub(crate) async fn get_moon_position(
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
        Ok(CallToolResult::success(vec![ContentBlock::text(
            v.to_string(),
        )]))
    }

    #[tool(
        description = "Angular separation (degrees) between an ICRS target and the \
                       Moon. Geocentric — does not depend on `site`."
    )]
    pub(crate) async fn compute_moon_separation(
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
        Ok(CallToolResult::success(vec![ContentBlock::text(
            v.to_string(),
        )]))
    }

    #[tool(description = "Local apparent sidereal time at the configured site. Requires `site`.")]
    pub(crate) async fn get_local_sidereal_time(
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
        Ok(CallToolResult::success(vec![ContentBlock::text(
            v.to_string(),
        )]))
    }

    // -------------------------------------------------------------------
    // Convenience tools (get_target_status, get_next_target,
    // get_meridian_status) — see docs/services/rp.md §"Dynamic Planner"
    // -------------------------------------------------------------------

    #[tool(description = "Sky position + progress for a target. Accepts either \
                       target_name (resolved via the embedded catalog) or a \
                       raw ra/dec pair. progress is the per-filter \
                       {completed, goal} map from the record_exposure \
                       counters when target_name (as given or \
                       catalog-resolved) matches a configured targets[] \
                       entry, null otherwise. Requires `site`.")]
    pub(crate) async fn get_target_status(
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
                    return Ok(CallToolResult::error(vec![ContentBlock::text(
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
        // The progress map is keyed by configured target names, which
        // are free-form, while the catalog normalises "m 31" → "M 31"
        // — so match the caller's `target_name` as given first, then
        // its catalog-resolved form (`name`). The ra/dec form has no
        // name to match and reports progress: null.
        let progress = match params.target_name.as_deref().and_then(|raw| {
            self.targets
                .iter()
                .find(|t| t.name == raw || t.name == name)
        }) {
            Some(configured) => {
                let store = self.progress.lock().unwrap_or_else(|e| e.into_inner());
                crate::planner::convenience::target_progress_view(configured, &store)
            }
            None => serde_json::Value::Null,
        };
        match crate::planner::convenience::target_status_view(
            site,
            target,
            &name,
            time,
            self.default_min_altitude_degrees,
            progress,
        ) {
            Ok(v) => Ok(CallToolResult::success(vec![ContentBlock::text(
                v.to_string(),
            )])),
            Err(e) => Ok(tool_error!("{}", e)),
        }
    }

    #[tool(
        description = "Recommend the next target from `targets[]` config based on \
                       altitude / approaching transit / integration progress / \
                       sun-elevation gating. filter and duration_secs are the \
                       recommended target's first incomplete exposures[] entry \
                       per the record_exposure counters (null when it has no \
                       plan). Returns target=null and a structured reason \
                       (no_targets_configured / all_below_min_altitude / \
                       wait_for_twilight / end_of_session) when no candidate is \
                       viable: wait_for_twilight = the Sun is brighter than \
                       astronomical dusk and not rising (evening — wait and \
                       re-ask); end_of_session = brighter and rising (dawn) or \
                       every target's integration goal is met — the session is \
                       over. Requires `site`."
    )]
    pub(crate) async fn get_next_target(
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
        let rec = {
            let progress = self.progress.lock().unwrap_or_else(|e| e.into_inner());
            crate::planner::decision::next_target(
                &eph,
                site,
                time,
                &self.targets,
                self.default_min_altitude_degrees,
                &progress,
            )
        };
        let v = crate::planner::convenience::next_target_view(rec);
        Ok(CallToolResult::success(vec![ContentBlock::text(
            v.to_string(),
        )]))
    }

    #[tool(
        description = "Record one completed frame against a configured targets[] \
                       entry's per-filter counter and return it: {target, \
                       filter, completed, goal}. goal is the summed `count` \
                       for that filter in the target's exposures[] plan (null \
                       when the filter is not in the plan or any matching \
                       entry has no count). Omit filter (or pass null / \"\") \
                       for an unfiltered frame. The counters drive \
                       get_next_target's plan rotation, target balancing, and \
                       the all-goals-met end_of_session; they reset when a \
                       fresh session starts."
    )]
    pub(crate) async fn record_exposure(
        &self,
        Parameters(params): Parameters<RecordExposureParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        // Recording against an unknown name would count frames the
        // planner can never see — a typo'd orchestrator config should
        // fail loudly, not silently disable progress tracking.
        let Some(target) = self.targets.iter().find(|t| t.name == params.target) else {
            return Ok(tool_error!(
                "unknown target `{}`: not a configured targets[] entry",
                params.target
            ));
        };
        let key = crate::planner::progress::filter_key(params.filter.as_deref());
        let completed = {
            let mut store = self.progress.lock().unwrap_or_else(|e| e.into_inner());
            store.record(&target.name, params.filter.as_deref())
        };
        let goal = crate::planner::progress::SessionProgress::goal_for(target, &key);
        Ok(tool_success!({
            "target": target.name,
            "filter": if key.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(key) },
            "completed": completed,
            "goal": goal,
        }))
    }

    #[tool(
        description = "Full progress overview from the record_exposure counters: \
                       target name -> filter -> {completed, goal} for every \
                       configured targets[] entry (the unfiltered slot appears \
                       under the empty-string key; a recorded filter outside \
                       the target's plan has goal null)."
    )]
    pub(crate) async fn get_session_progress(
        &self,
        Parameters(_params): Parameters<GetSessionProgressParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let store = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        let v = crate::planner::convenience::session_progress_view(&self.targets, &store);
        Ok(CallToolResult::success(vec![ContentBlock::text(
            v.to_string(),
        )]))
    }

    #[tool(
        description = "Time-to-flip plus side-of-pier for the mount's current \
                       pointing. Reads RA/Dec/SideOfPier from the configured \
                       mount, then runs the meridian-flip primitive. Requires \
                       `site` and a connected mount."
    )]
    pub(crate) async fn get_meridian_status(
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
        Ok(CallToolResult::success(vec![ContentBlock::text(
            v.to_string(),
        )]))
    }
}
