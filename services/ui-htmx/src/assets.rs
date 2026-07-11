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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used)]
mod tests {
    use axum::response::IntoResponse;

    /// The content-type is what makes the browser apply/execute an asset —
    /// pin it (and that the embedded bytes are the expected artifact).
    async fn assert_asset(response: axum::response::Response, content_type: &str, marker: &str) {
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .unwrap(),
            content_type
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.contains(marker), "asset lacks marker {marker:?}");
    }

    #[tokio::test]
    async fn app_css_is_served_as_css() {
        assert_asset(
            super::app_css().await.into_response(),
            "text/css; charset=utf-8",
            ":root",
        )
        .await;
    }

    #[tokio::test]
    async fn htmx_js_is_served_as_javascript() {
        assert_asset(
            super::htmx_js().await.into_response(),
            "application/javascript; charset=utf-8",
            "htmx",
        )
        .await;
    }

    #[tokio::test]
    async fn htmx_sse_extension_is_served_as_javascript() {
        assert_asset(
            super::htmx_sse_js().await.into_response(),
            "application/javascript; charset=utf-8",
            "sse",
        )
        .await;
    }
}
