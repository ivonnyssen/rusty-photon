//! Status badge component

use leptos::prelude::*;

/// A colored badge showing Safe (green), Unsafe (red), or Unknown (gray)
#[component]
pub fn StatusBadge(state: String) -> impl IntoView {
    let (color, bg) = match state.as_str() {
        "Safe" => ("#155724", "#d4edda"),
        "Unsafe" => ("#721c24", "#f8d7da"),
        _ => ("#383d41", "#e2e3e5"),
    };

    let style = format!(
        "display: inline-block; padding: 0.25em 0.6em; border-radius: 0.25rem; \
         font-size: 0.85em; font-weight: 600; color: {}; background-color: {};",
        color, bg
    );

    view! {
        <span style=style>{state}</span>
    }
}
