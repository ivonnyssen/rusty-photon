//! Sentinel Dashboard - Leptos frontend
//!
//! Reactive web UI for monitoring observatory device states.

pub mod api;
pub mod app;
pub mod components;

pub use app::App;

/// Hydration entry point for WASM client
#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    leptos::mount::hydrate_body(App);
}
