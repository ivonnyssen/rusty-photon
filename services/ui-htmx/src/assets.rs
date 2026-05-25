//! Static assets, embedded into the binary via `include_str!` so the BFF ships
//! as a single executable (the Pi 5 deployment story). No npm, no CDN.

use axum::http::header;
use axum::response::IntoResponse;

/// Dark-theme stylesheet (design tokens from the chosen activity-stream mock).
const APP_CSS: &str = include_str!("../assets/app.css");

/// Pinned HTMX bundle (htmx.org@2.0.4).
const HTMX_JS: &str = include_str!("../assets/htmx.min.js");

pub async fn app_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], APP_CSS)
}

pub async fn htmx_js() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        HTMX_JS,
    )
}
