//! BDD test world for sentinel service

use std::sync::Arc;

use cucumber::World;
use sentinel::config::TransitionConfig;
use sentinel::monitor::{Monitor, MonitorState};
use sentinel::notifier::Notifier;
use sentinel::state::StateHandle;

#[derive(Debug, Default, World)]
pub struct SentinelWorld {
    // Monitor testing
    pub monitor: Option<Box<dyn Monitor>>,
    pub last_state: Option<MonitorState>,
    pub last_result: Option<sentinel::Result<String>>,

    // Notifier testing
    pub notifier: Option<Box<dyn Notifier>>,
    pub notification_result: Option<sentinel::Result<()>>,

    // Transition testing
    pub transition_monitor_name: Option<String>,
    pub transition_initial_state: Option<MonitorState>,
    pub transition_state: Option<StateHandle>,
    pub transition_rules: Option<Vec<TransitionConfig>>,
    pub transition_notifiers: Option<Vec<Arc<dyn Notifier>>>,
    pub transition_recording_notifier: Option<Arc<dyn Notifier>>,

    // Engine testing
    pub engine_monitors: Option<Vec<Arc<dyn Monitor>>>,
    pub engine_state: Option<StateHandle>,

    // Dashboard testing
    pub dashboard_state: Option<StateHandle>,
    pub dashboard_response_body: Option<String>,
}
