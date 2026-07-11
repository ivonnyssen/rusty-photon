//! Static assets, embedded into the binary via `include_str!` so the BFF ships
//! as a single executable (the Pi 5 deployment story). No npm, no CDN.

use axum::http::header;
use axum::response::IntoResponse;

/// Dark-theme stylesheet (design tokens from the chosen activity-stream mock).
const APP_CSS: &str = include_str!("../assets/app.css");

/// Pinned HTMX bundle (htmx.org@2.0.4).
const HTMX_JS: &str = include_str!("../assets/htmx.min.js");

/// Pinned HTMX SSE extension (htmx-ext-sse@2.2.3, vendored byte-for-byte —
/// htmx 2.0 split SSE out of core). Loaded only by pages that stream (the
/// activity stream; the `test-sse` fixture page keeps its own copy).
const HTMX_SSE_JS: &str = include_str!("../assets/htmx-ext-sse.js");

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

pub async fn htmx_sse_js() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        HTMX_SSE_JS,
    )
}
