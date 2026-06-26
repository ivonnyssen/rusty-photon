//! The SDK seam: a thin trait over the blocking `touptek-rs` [`Camera`] surface
//! the ASCOM device drives, plus a production wrapper.
//!
//! Why a seam: it (1) collapses [`touptek_rs::Error`] into a typed
//! [`BackendError`] at one boundary, (2) lets the ASCOM device hold an
//! `Arc<dyn CameraHandle>` so later unit tests can substitute a mock that forces
//! paths the `touptek-rs` simulation cannot (a model without ST4, a mid-exposure
//! SDK error) without hardware, and (3) keeps the open/close lifecycle in one
//! place. `touptek-rs`'s [`Camera`](touptek_rs::Camera) is RAII (open =
//! [`touptek_rs::Sdk::open`], close = drop) and `Send + !Sync`, so the production
//! handle keeps it behind a `parking_lot::Mutex` and re-opens on connect from the
//! cached enumeration `index`.
//!
//! Phase C scope: identity + the open/close connection lifecycle. The capture /
//! control / cooling / pulse-guide surface is added in Phase E (see
//! `docs/plans/touptek-driver.md`).

use parking_lot::Mutex;
use touptek_rs::CameraInfo;

/// A `touptek-rs` SDK call failed. Carries the underlying message; the ASCOM
/// device decides the `ASCOMError` per call site (the SDK error kind does not map
/// 1:1 to an ASCOM code).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct BackendError(pub String);

/// Collapse a [`touptek_rs::Error`] into the typed seam error. The seam keeps only
/// the message string (each call site picks the right `ASCOMError` code); this
/// `From` impl lets `?` convert SDK errors automatically.
impl From<touptek_rs::Error> for BackendError {
    fn from(err: touptek_rs::Error) -> Self {
        Self(err.to_string())
    }
}

pub type BackendResult<T> = std::result::Result<T, BackendError>;

/// The blocking camera operations the ASCOM `Camera` device drives. Every method
/// is synchronous (the SDK is blocking C FFI); the device offloads them onto
/// `spawn_blocking`. Phase C exposes only identity + the open/close lifecycle.
pub trait CameraHandle: std::fmt::Debug + Send + Sync {
    /// The stable ASCOM `UniqueID` (id-derived; read once at enumeration).
    fn unique_id(&self) -> String;

    /// The camera's enumeration [`CameraInfo`] (cached; no open required).
    fn info(&self) -> CameraInfo;

    fn is_open(&self) -> bool;
    fn open(&self) -> BackendResult<()>;
    fn close(&self) -> BackendResult<()>;
}

// --- production wrapper over touptek-rs ------------------------------------------

/// Production [`CameraHandle`] over a real (or `touptek-rs`-simulated) camera.
///
/// Holds the [`touptek_rs::Sdk`] (a ZST) and the enumeration `index` so it can
/// re-open the RAII [`touptek_rs::Camera`] on connect; the open handle lives
/// behind a `Mutex<Option<…>>` because `Camera` is `Send + !Sync`.
#[derive(derive_more::Debug)]
pub struct TouptekCameraHandle {
    #[debug(skip)]
    sdk: touptek_rs::Sdk,
    index: usize,
    info: CameraInfo,
    unique_id: String,
    // `touptek_rs::Camera` is not `Debug`, so the field is skipped.
    #[debug(skip)]
    camera: Mutex<Option<touptek_rs::Camera>>,
}

impl TouptekCameraHandle {
    /// Build a handle for the camera at enumeration `index`, with its cached
    /// [`CameraInfo`] and the id-derived `unique_id` read at enumeration.
    pub fn new(sdk: touptek_rs::Sdk, index: usize, info: CameraInfo, unique_id: String) -> Self {
        Self {
            sdk,
            index,
            info,
            unique_id,
            camera: Mutex::new(None),
        }
    }
}

impl CameraHandle for TouptekCameraHandle {
    fn unique_id(&self) -> String {
        self.unique_id.clone()
    }

    fn info(&self) -> CameraInfo {
        self.info.clone()
    }

    fn is_open(&self) -> bool {
        self.camera.lock().is_some()
    }

    fn open(&self) -> BackendResult<()> {
        let mut guard = self.camera.lock();
        if guard.is_none() {
            *guard = Some(self.sdk.open(self.index)?);
        }
        Ok(())
    }

    fn close(&self) -> BackendResult<()> {
        // Dropping the `Camera` calls `Toupcam_Stop` + `Toupcam_Close`.
        *self.camera.lock() = None;
        Ok(())
    }
}

// --- tests -----------------------------------------------------------------------

/// Exercise the *production* [`TouptekCameraHandle`] against the `touptek-rs`
/// simulation backend (covers the real SDK wrapper that the BDD suite otherwise
/// reaches only via the spawned binary).
#[cfg(all(test, feature = "simulation"))]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod handle_tests {
    use super::*;

    fn sim_handle() -> TouptekCameraHandle {
        let sdk = touptek_rs::Sdk::new().expect("simulation SDK");
        let info = sdk.enumerate().expect("enumerate")[0].clone();
        TouptekCameraHandle::new(sdk, 0, info, "TOUPTEK:Sim:sim-0".to_string())
    }

    #[test]
    fn production_handle_round_trips_against_the_sim_sdk() {
        let handle = sim_handle();
        assert_eq!(handle.unique_id(), "TOUPTEK:Sim:sim-0");
        assert_eq!(handle.info().id, "sim-0");
        // Open/close lifecycle.
        assert!(!handle.is_open());
        handle.open().unwrap();
        assert!(handle.is_open());
        // Idempotent open is a no-op.
        handle.open().unwrap();
        assert!(handle.is_open());
        handle.close().unwrap();
        assert!(!handle.is_open());
    }
}
