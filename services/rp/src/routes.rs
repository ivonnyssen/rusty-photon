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
    // Single cache lookup: `get` mutates LRU order, so calling it twice would
    // double-bump this entry to MRU.
    let cached = state.image_cache.get(&document_id);
    let in_cache = cached.is_some();
    match state.documents.get(&document_id).await {
        Some(doc) => {
            let bitpix = match cached.as_ref() {
                Some(c) => match &c.pixels {
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
        Ok(Err(e)) => server_error(format!("failed to read FITS: {}", e)),
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
    use crate::document::ExposureDocument;
    use crate::events::EventBus;
    use crate::imaging::CachedImage;
    use std::path::PathBuf;

    fn test_app_state(image_cache: ImageCache, documents: DocumentStore) -> AppState {
        let event_bus = Arc::new(EventBus::from_config(&[]));
        let equipment = Arc::new(crate::equipment::EquipmentRegistry {
            cameras: vec![],
            filter_wheels: vec![],
            cover_calibrators: vec![],
        });
        let session = Arc::new(crate::session::SessionManager::new(event_bus.clone(), &[]));
        let mcp = McpHandler::new(
            equipment.clone(),
            event_bus.clone(),
            crate::session::SessionConfig {
                data_directory: "/tmp".to_string(),
            },
            image_cache.clone(),
            documents.clone(),
        );
        AppState {
            equipment,
            mcp,
            session,
            documents,
            image_cache,
        }
    }

    fn cached_u16(arr: ndarray::Array2<u16>) -> CachedImage {
        let (w, h) = arr.dim();
        CachedImage {
            pixels: CachedPixels::U16(arr),
            width: w as u32,
            height: h as u32,
            fits_path: PathBuf::from("/tmp/fake.fits"),
            max_adu: 65535,
        }
    }

    fn cached_i32(arr: ndarray::Array2<i32>) -> CachedImage {
        let (w, h) = arr.dim();
        CachedImage {
            pixels: CachedPixels::I32(arr),
            width: w as u32,
            height: h as u32,
            fits_path: PathBuf::from("/tmp/fake.fits"),
            max_adu: 1 << 20,
        }
    }

    fn doc_at(file_path: &str) -> ExposureDocument {
        ExposureDocument {
            id: "doc-1".to_string(),
            captured_at: "2026-04-30T00:00:00Z".to_string(),
            file_path: file_path.to_string(),
            width: 2,
            height: 2,
            camera_id: None,
            duration: None,
            max_adu: None,
            sections: serde_json::Map::new(),
        }
    }

    async fn body_bytes(response: Response) -> Vec<u8> {
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    async fn store_with_doc_at(file_path: &str) -> DocumentStore {
        let store = DocumentStore::new();
        store.create(doc_at(file_path)).await.unwrap();
        store
    }

    #[tokio::test]
    async fn pixels_serves_u16_from_cache() {
        let cache = ImageCache::new(64, 4);
        cache.insert(
            "doc-1".to_string(),
            cached_u16(ndarray::Array2::from_shape_vec((2, 2), vec![1u16, 2, 3, 4]).unwrap()),
        );
        let response = get_image_pixels(
            State(test_app_state(cache, DocumentStore::new())),
            Path("doc-1".to_string()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/imagebytes"
        );
        let body = body_bytes(response).await;
        assert_eq!(body.len(), IMAGEBYTES_HEADER_LEN + 4 * 2);
        assert_eq!(&body[24..28], &TRANSMISSION_U16.to_le_bytes());
        assert_eq!(&body[44..52], &[1, 0, 2, 0, 3, 0, 4, 0]);
    }

    #[tokio::test]
    async fn pixels_serves_i32_from_cache() {
        let cache = ImageCache::new(64, 4);
        cache.insert(
            "doc-1".to_string(),
            cached_i32(ndarray::Array2::from_shape_vec((2, 2), vec![1i32, 2, 3, 4]).unwrap()),
        );
        let response = get_image_pixels(
            State(test_app_state(cache, DocumentStore::new())),
            Path("doc-1".to_string()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/imagebytes"
        );
        let body = body_bytes(response).await;
        assert_eq!(body.len(), IMAGEBYTES_HEADER_LEN + 4 * 4);
        assert_eq!(&body[24..28], &TRANSMISSION_I32.to_le_bytes());
        let mut expected = Vec::new();
        for v in [1i32, 2, 3, 4] {
            expected.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(&body[44..], &expected[..]);
    }

    #[tokio::test]
    async fn pixels_falls_back_to_fits_on_cache_miss() {
        let dir = tempfile::tempdir().unwrap();
        let fits_path = dir.path().join("img.fits").to_string_lossy().into_owned();
        let pixels = vec![10i32, 20, 30, 40];
        crate::imaging::write_fits(&fits_path, &pixels, 2, 2, "test-doc-id")
            .await
            .unwrap();
        let store = store_with_doc_at(&fits_path).await;

        let response = get_image_pixels(
            State(test_app_state(ImageCache::new(64, 4), store)),
            Path("doc-1".to_string()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_bytes(response).await;
        assert_eq!(body.len(), IMAGEBYTES_HEADER_LEN + 4 * 4);
        // FITS fallback always serves I32 (FITS-on-disk is i32 per write_fits).
        assert_eq!(&body[24..28], &TRANSMISSION_I32.to_le_bytes());
        let mut expected = Vec::new();
        for v in pixels {
            expected.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(&body[44..], &expected[..]);
    }

    #[tokio::test]
    async fn pixels_returns_500_when_fits_read_fails() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir
            .path()
            .join("missing.fits")
            .to_string_lossy()
            .into_owned();
        let store = store_with_doc_at(&missing).await;

        let response = get_image_pixels(
            State(test_app_state(ImageCache::new(64, 4), store)),
            Path("doc-1".to_string()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_bytes(response).await;
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert!(
            v["error"]
                .as_str()
                .expect("error field is a string")
                .contains("failed to read FITS"),
            "got: {:?}",
            v
        );
    }

    #[tokio::test]
    async fn metadata_reports_bitpix_16_for_u16_cached() {
        let dir = tempfile::tempdir().unwrap();
        let fits_path = dir.path().join("img.fits").to_string_lossy().into_owned();
        let cache = ImageCache::new(64, 4);
        cache.insert(
            "doc-1".to_string(),
            cached_u16(ndarray::Array2::from_elem((2, 2), 0u16)),
        );
        let store = store_with_doc_at(&fits_path).await;

        let (status, Json(body)) = get_image_metadata(
            State(test_app_state(cache, store)),
            Path("doc-1".to_string()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["bitpix"], 16);
        assert_eq!(body["in_cache"], true);
    }

    #[tokio::test]
    async fn metadata_reports_bitpix_32_for_i32_cached() {
        let dir = tempfile::tempdir().unwrap();
        let fits_path = dir.path().join("img.fits").to_string_lossy().into_owned();
        let cache = ImageCache::new(64, 4);
        cache.insert(
            "doc-1".to_string(),
            cached_i32(ndarray::Array2::from_elem((2, 2), 0i32)),
        );
        let store = store_with_doc_at(&fits_path).await;

        let (status, Json(body)) = get_image_metadata(
            State(test_app_state(cache, store)),
            Path("doc-1".to_string()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["bitpix"], 32);
        assert_eq!(body["in_cache"], true);
    }

    #[tokio::test]
    async fn metadata_reports_bitpix_32_when_cache_miss() {
        let dir = tempfile::tempdir().unwrap();
        let fits_path = dir.path().join("img.fits").to_string_lossy().into_owned();
        let store = store_with_doc_at(&fits_path).await;

        let (status, Json(body)) = get_image_metadata(
            State(test_app_state(ImageCache::new(64, 4), store)),
            Path("doc-1".to_string()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["bitpix"], 32);
        assert_eq!(body["in_cache"], false);
    }

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
