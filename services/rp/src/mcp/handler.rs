//! `McpHandler` — the type that owns rp's MCP state and on which all
//! `#[tool]`-annotated methods live. Per-category tools live in
//! sibling submodules under [`super::built_in`]; each declares its own
//! `#[tool_router(router = tool_router_<category>, vis = "pub")]`
//! impl block on this type. [`McpHandler::new`] merges those
//! per-category routers via the `+` operator on
//! [`rmcp::handler::server::router::tool::ToolRouter`].

use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;

use crate::equipment::EquipmentRegistry;
use crate::events::EventBus;
use crate::persistence::ImageCache;
use crate::session::SessionConfig;

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
    /// The `record_exposure` counters (rp.md §"Session Persistence"
    /// `progress` map, in-memory). Behind an `Arc` so every clone of
    /// the handler — rmcp clones it per MCP connection — shares one
    /// store, and so `SessionManager::start` can clear it when a
    /// fresh session begins. Lock with
    /// `.lock().unwrap_or_else(|e| e.into_inner())` (the event-bus
    /// convention) and never hold it across an `.await`.
    pub progress: Arc<std::sync::Mutex<crate::planner::progress::SessionProgress>>,
    /// Optional plate-solver HTTP client. `None` ⇒ `plate_solve`
    /// MCP tool returns "plate solver not configured". Wired by
    /// `with_plate_solver` from the `plate_solver` block in rp
    /// config.
    pub plate_solver: Option<Arc<dyn rp_plate_solver::PlateSolveClient>>,
    /// Operator-set default applied when the per-call
    /// `search_radius_deg` parameter is omitted. Mirrors
    /// `PlateSolverConfig::default_search_radius_deg`.
    pub plate_solver_default_search_radius_deg: Option<f64>,
    /// Per-rig estimates sizing the advisory `center_on_target` deadline
    /// (§2.5) carried on `centering_started`. Wired by
    /// `with_centering_config` from the `centering` block in rp config;
    /// tests use `CenteringConfig::default()`.
    pub centering: crate::config::CenteringConfig,
    /// Merged tool catalog. Built by summing per-category routers
    /// in [`McpHandler::new`]; consumed by the
    /// `#[tool_handler(router = self.tool_router)]` ServerHandler
    /// impl in [`super`].
    pub tool_router: ToolRouter<Self>,
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
            progress: Arc::new(std::sync::Mutex::new(
                crate::planner::progress::SessionProgress::default(),
            )),
            plate_solver: None,
            plate_solver_default_search_radius_deg: None,
            centering: crate::config::CenteringConfig::default(),
            // Pattern (c) merge: each `built_in/<category>.rs`
            // declares a `#[tool_router(router = tool_router_<name>,
            // vis = "pub")]` block whose generated associated function
            // returns the per-category `ToolRouter<Self>`. The
            // `ToolRouter` type implements `Add` so we sum them into
            // one merged catalog. Adding a new tool category =
            // append one `+ Self::tool_router_<name>()` here.
            tool_router: Self::tool_router_camera()
                + Self::tool_router_imaging()
                + Self::tool_router_filter_wheel()
                + Self::tool_router_cover_calibrator()
                + Self::tool_router_focuser()
                + Self::tool_router_mount()
                + Self::tool_router_auto_focus()
                + Self::tool_router_plate_solve()
                + Self::tool_router_center_on_target()
                + Self::tool_router_planner(),
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

    /// Share the `record_exposure` counters with the rest of the
    /// process (lib.rs passes the same `Arc` to `SessionManager` so a
    /// fresh session start clears them). Tests that only exercise the
    /// tools can keep the private store `new()` creates.
    pub fn with_progress_store(
        mut self,
        store: Arc<std::sync::Mutex<crate::planner::progress::SessionProgress>>,
    ) -> Self {
        self.progress = store;
        self
    }

    /// Wire the plate-solver HTTP client + operator-set search-radius
    /// default. `None` for `client` keeps the MCP tool reporting
    /// "not configured"; `None` for the radius means the wrapper
    /// falls through to ASTAP's own default when the per-call
    /// parameter is also omitted.
    pub fn with_plate_solver(
        mut self,
        client: Option<Arc<dyn rp_plate_solver::PlateSolveClient>>,
        default_search_radius_deg: Option<f64>,
    ) -> Self {
        self.plate_solver = client;
        self.plate_solver_default_search_radius_deg = default_search_radius_deg;
        self
    }

    /// Wire the per-rig centering estimates (§2.5) from the `centering`
    /// config block. The lib.rs build path calls this with
    /// `config.centering`; tests leave the default.
    pub fn with_centering_config(mut self, centering: crate::config::CenteringConfig) -> Self {
        self.centering = centering;
        self
    }
}
