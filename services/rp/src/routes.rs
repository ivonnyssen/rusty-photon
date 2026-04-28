use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use ndarray::Array2;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::StreamableHttpServerConfig;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use serde_json::Value;
use tracing::debug;

use crate::document::DocumentStore;
use crate::equipment::EquipmentRegistry;
use crate::imaging::{self, CachedPixels, ImageCache};
use crate::mcp::McpHandler;
use crate::session::SessionManager;

/// ASCOM `TransmissionElementType` code for `u16` payloads.
const TRANSMISSION_U16: i32 = 8;
/// ASCOM `TransmissionElementType` code for `i32` payloads.
const TRANSMISSION_I32: i32 = 2;
/// ASCOM `ImageElementType` is always `Int32` (the logical type required by
/// the Alpaca API). The transmission type may differ.
const IMAGE_ELEMENT_I32: i32 = 2;
const IMAGEBYTES_HEADER_LEN: usize = 44;

#[derive(Clone)]
pub struct AppState {
    pub equipment: Arc<EquipmentRegistry>,
    pub mcp: McpHandler,
    pub session: Arc<SessionManager>,
    pub documents: DocumentStore,
    pub image_cache: ImageCache,
}

pub fn build_router(state: AppState) -> Router {
    let mcp_handler = state.mcp.clone();
    let mut mcp_config = StreamableHttpServerConfig::default();
    mcp_config.json_response = true;
    let mcp_service = StreamableHttpService::new(
        move || Ok(mcp_handler.clone()),
        Arc::new(LocalSessionManager::default()),
        mcp_config,
    );

    Router::new()
        .route("/health", get(health))
        .route("/api/equipment", get(get_equipment))
        .nest_service("/mcp", mcp_service)
        .route("/api/session/start", post(session_start))
        .route("/api/session/stop", post(session_stop))
        .route("/api/session/status", get(session_status))
        .route(
            "/api/plugins/{workflow_id}/complete",
            post(workflow_complete),
        )
        .route("/api/documents/{document_id}", get(get_document))
        .route("/api/images/{document_id}", get(get_image_metadata))
        .route("/api/images/{document_id}/pixels", get(get_image_pixels))
        .with_state(state)
}

async fn health() -> &'static str {
    "Hello World, I am healthy!"
}

async fn get_equipment(State(state): State<AppState>) -> Json<Value> {
    let status = state.equipment.status();
    Json(serde_json::to_value(status).unwrap_or_default())
}

async fn session_start(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    match state.session.start().await {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err(e) => (StatusCode::CONFLICT, Json(serde_json::json!({"error": e}))),
    }
}

async fn session_stop(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    match state.session.stop().await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"status": "stopped"})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e})),
        ),
    }
}

async fn session_status(State(state): State<AppState>) -> Json<Value> {
    let status = state.session.status().await;
    Json(serde_json::json!({"status": status}))
}

async fn workflow_complete(
    State(state): State<AppState>,
    Path(workflow_id): Path<String>,
) -> StatusCode {
    debug!(workflow_id = %workflow_id, "received workflow completion");
    state.session.workflow_complete(&workflow_id).await;
    StatusCode::OK
}

async fn get_document(
    State(state): State<AppState>,
    Path(document_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    match state.documents.get(&document_id).await {
        Some(doc) => match serde_json::to_value(&doc) {
            Ok(v) => (StatusCode::OK, Json(v)),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            ),
        },
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("document not found: {}", document_id)})),
        ),
    }
}

async fn get_image_metadata(
    State(state): State<AppState>,
    Path(document_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    let in_cache = state.image_cache.get(&document_id).is_some();
    match state.documents.get(&document_id).await {
        Some(doc) => {
            let bitpix = match state.image_cache.get(&document_id) {
                Some(cached) => match cached.pixels {
                    CachedPixels::U16(_) => 16,
                    CachedPixels::I32(_) => 32,
                },
                // Cache miss: FITS-on-disk is always written as i32.
                None => 32,
            };
            let body = serde_json::json!({
                "document_id": document_id,
                "width": doc.width,
                "height": doc.height,
                "bitpix": bitpix,
                "fits_path": doc.file_path,
                "in_cache": in_cache,
                "document_url": format!("/api/documents/{}", document_id),
            });
            (StatusCode::OK, Json(body))
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("document not found: {}", document_id)})),
        ),
    }
}

async fn get_image_pixels(
    State(state): State<AppState>,
    Path(document_id): Path<String>,
) -> Response {
    if let Some(cached) = state.image_cache.get(&document_id) {
        let (width, height) = (cached.width, cached.height);
        let body = match &cached.pixels {
            CachedPixels::U16(arr) => imagebytes(width, height, TRANSMISSION_U16, |buf| {
                buf.reserve(arr.len() * 2);
                for &v in arr.iter() {
                    buf.extend_from_slice(&v.to_le_bytes());
                }
            }),
            CachedPixels::I32(arr) => imagebytes(width, height, TRANSMISSION_I32, |buf| {
                buf.reserve(arr.len() * 4);
                for &v in arr.iter() {
                    buf.extend_from_slice(&v.to_le_bytes());
                }
            }),
        };
        return imagebytes_response(body);
    }

    // Cache miss: fall back to reading the FITS file from disk.
    let doc = match state.documents.get(&document_id).await {
        Some(d) => d,
        None => {
            return not_found(format!("document not found: {}", document_id));
        }
    };
    let path = doc.file_path.clone();
    let read = tokio::task::spawn_blocking(move || imaging::read_fits_pixels(&path)).await;
    match read {
        Ok(Ok((pixels, width, height))) => {
            let arr = match Array2::from_shape_vec((width as usize, height as usize), pixels) {
                Ok(a) => a,
                Err(e) => {
                    return server_error(format!("FITS shape mismatch: {}", e));
                }
            };
            let body = imagebytes(width, height, TRANSMISSION_I32, |buf| {
                buf.reserve(arr.len() * 4);
                for &v in arr.iter() {
                    buf.extend_from_slice(&v.to_le_bytes());
                }
            });
            imagebytes_response(body)
        }
        Ok(Err(e)) => not_found(format!("failed to read FITS: {}", e)),
        Err(e) => server_error(format!("task error: {}", e)),
    }
}

/// Build an Alpaca ImageBytes payload (44-byte header + raw little-endian
/// pixel bytes). The header layout mirrors `ascom-alpaca`'s
/// `ImageBytesMetadata` (which is `pub(crate)` upstream, so we replicate
/// it here). All fields are i32 little-endian.
fn imagebytes(
    width: u32,
    height: u32,
    transmission_element_type: i32,
    write_pixels: impl FnOnce(&mut Vec<u8>),
) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::with_capacity(IMAGEBYTES_HEADER_LEN);
    let header_fields: [i32; 11] = [
        1,                            // metadata_version
        0,                            // error_number
        0,                            // client_transaction_id
        0,                            // server_transaction_id
        IMAGEBYTES_HEADER_LEN as i32, // data_start
        IMAGE_ELEMENT_I32,            // image_element_type (logical, always Int32)
        transmission_element_type,    // transmission_element_type
        2,                            // rank
        width as i32,                 // dimension_1
        height as i32,                // dimension_2
        0,                            // dimension_3
    ];
    for f in &header_fields {
        buf.extend_from_slice(&f.to_le_bytes());
    }
    debug_assert_eq!(buf.len(), IMAGEBYTES_HEADER_LEN);
    write_pixels(&mut buf);
    buf
}

fn imagebytes_response(body: Vec<u8>) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/imagebytes")],
        body,
    )
        .into_response()
}

fn not_found(msg: String) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": msg})),
    )
        .into_response()
}

fn server_error(msg: String) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": msg})),
    )
        .into_response()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn imagebytes_header_layout_u16() {
        let body = imagebytes(4, 3, TRANSMISSION_U16, |buf| {
            buf.extend_from_slice(&[1u8, 2, 3, 4]);
        });
        assert_eq!(body.len(), IMAGEBYTES_HEADER_LEN + 4);
        // metadata_version = 1
        assert_eq!(&body[0..4], &1i32.to_le_bytes());
        // data_start = 44
        assert_eq!(&body[16..20], &44i32.to_le_bytes());
        // image_element_type = 2
        assert_eq!(&body[20..24], &2i32.to_le_bytes());
        // transmission_element_type = 8 (U16)
        assert_eq!(&body[24..28], &8i32.to_le_bytes());
        // rank = 2
        assert_eq!(&body[28..32], &2i32.to_le_bytes());
        // dim1 = 4
        assert_eq!(&body[32..36], &4i32.to_le_bytes());
        // dim2 = 3
        assert_eq!(&body[36..40], &3i32.to_le_bytes());
        // dim3 = 0
        assert_eq!(&body[40..44], &0i32.to_le_bytes());
        // payload
        assert_eq!(&body[44..48], &[1u8, 2, 3, 4]);
    }

    #[test]
    fn imagebytes_header_layout_i32() {
        let body = imagebytes(2, 2, TRANSMISSION_I32, |_| {});
        assert_eq!(body.len(), IMAGEBYTES_HEADER_LEN);
        assert_eq!(&body[24..28], &2i32.to_le_bytes());
    }
}
