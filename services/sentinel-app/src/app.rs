//! Main App component

use crate::components::history_table::HistoryTable;
use crate::components::monitor_table::MonitorTable;
use leptos::prelude::*;

/// Root application component
#[component]
pub fn App() -> impl IntoView {
    view! {
        <main style="font-family: system-ui, sans-serif; max-width: 960px; margin: 0 auto; padding: 1rem;">
            <h1>"Sentinel Dashboard"</h1>
            <MonitorTable />
            <HistoryTable />
        </main>
    }
}
