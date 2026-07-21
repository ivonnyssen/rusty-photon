//! The SDK seam: a thin trait over the blocking `svbony-rs` `Camera` surface
//! the ASCOM device drives, plus a production wrapper and a test mock.
//!
//! Mirrors `zwo-camera`'s `backend.rs` seam pattern: it (1) collapses
//! [`svbony_rs::Error`] into a typed [`BackendError`] at one boundary, (2)
//! lets the ASCOM device hold an `Arc<dyn CameraHandle>` so unit tests can
//! substitute a mock without hardware, and (3) keeps the open/close
//! lifecycle in one place. `svbony-rs`'s `Camera` is RAII (open =
//! [`svbony_rs::Sdk::open_camera`], close = drop) and `Send + !Sync`, so the
//! production handle keeps it behind a `parking_lot::Mutex` and re-opens on
//! connect from the cached enumeration `index`.
//!
//! **Phase C/D scope.** Only the connection-lifecycle surface (open/close/
//! is_open/info/unique_id) is wired here — enough to back the real `Device`
//! impl in `camera.rs`. The exposure/ROI/gain/cooling seam methods land in
//! Phase E alongside the real `Camera` trait implementation; see
//! `docs/services/svbony-camera.md` "Delivery phasing".

use parking_lot::Mutex;
use svbony_rs::CameraInfo;

/// A `svbony-rs` SDK call failed. Carries the underlying message; the ASCOM
/// device decides the `ASCOMError` per call site.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct BackendError(pub String);

/// Collapse a [`svbony_rs::Error`] into the typed seam error.
impl From<svbony_rs::Error> for BackendError {
    fn from(err: svbony_rs::Error) -> Self {
        Self(err.to_string())
    }
}

pub type BackendResult<T> = std::result::Result<T, BackendError>;

/// The blocking camera operations the ASCOM `Camera` device drives. Every
/// method is synchronous (the SDK is blocking C FFI); callers offload SDK
/// calls onto `spawn_blocking`.
pub trait CameraHandle: std::fmt::Debug + Send + Sync {
    /// The stable ASCOM `UniqueID` (serial-derived; read once at enumeration).
    fn unique_id(&self) -> String;

    /// The camera's enumeration [`CameraInfo`] (cached; no open required).
    fn info(&self) -> CameraInfo;

    fn is_open(&self) -> bool;
    fn open(&self) -> BackendResult<()>;
    fn close(&self) -> BackendResult<()>;
}

// --- production wrapper over svbony-rs ------------------------------------

/// Production [`CameraHandle`] over a real (or `svbony-rs`-simulated) camera.
///
/// Holds the [`svbony_rs::Sdk`] (a ZST) and the enumeration `index` so it can
/// re-open the RAII [`svbony_rs::Camera`] on connect; the open handle lives
/// behind a `Mutex<Option<…>>` because `Camera` is `Send + !Sync`.
#[derive(Debug)]
pub struct SvbonyCameraHandle {
    sdk: svbony_rs::Sdk,
    index: usize,
    info: CameraInfo,
    unique_id: String,
    camera: Mutex<Option<svbony_rs::Camera>>,
}

impl SvbonyCameraHandle {
    /// Build a handle for the camera at enumeration `index`, with its cached
    /// [`CameraInfo`] and the serial-derived `unique_id` read at enumeration.
    pub fn new(sdk: svbony_rs::Sdk, index: usize, info: CameraInfo, unique_id: String) -> Self {
        Self {
            sdk,
            index,
            info,
            unique_id,
            camera: Mutex::new(None),
        }
    }
}

impl CameraHandle for SvbonyCameraHandle {
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
            *guard = Some(self.sdk.open_camera(self.index)?);
        }
        Ok(())
    }

    fn close(&self) -> BackendResult<()> {
        // Dropping the `Camera` calls `SVBCloseCamera`.
        *self.camera.lock() = None;
        Ok(())
    }
}

#[cfg(all(test, feature = "simulation"))]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod handle_tests {
    use super::*;

    fn sim_handle() -> SvbonyCameraHandle {
        let sdk = svbony_rs::Sdk::new().expect("simulation SDK");
        let info = sdk.cameras().expect("enumerate")[0].clone();
        SvbonyCameraHandle::new(sdk, 0, info, "SVBONY:Sim:0a1b2c3d4e5f6071".to_string())
    }

    #[test]
    fn production_handle_round_trips_against_the_sim_sdk() {
        let handle = sim_handle();
        assert_eq!(handle.unique_id(), "SVBONY:Sim:0a1b2c3d4e5f6071");
        assert!(!handle.info().friendly_name.is_empty());
        assert!(!handle.is_open());
        handle.open().unwrap();
        assert!(handle.is_open());
        handle.close().unwrap();
        assert!(!handle.is_open());
    }
}

/// A configurable in-memory [`CameraHandle`] for the crate's unit tests, so
/// `camera.rs`'s connection-lifecycle logic is exercised without hardware.
#[cfg(test)]
pub(crate) mod mock {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn default_info() -> CameraInfo {
        CameraInfo {
            id: 0,
            friendly_name: "SV605CC-Simulated".to_string(),
            serial: "SVB0123456789AB".to_string(),
            port_type: "USB3".to_string(),
            device_id: 0,
        }
    }

    #[derive(Debug)]
    pub(crate) struct MockCameraHandle {
        unique_id: String,
        info: CameraInfo,
        open: AtomicBool,
        /// Force the next `open()` call to fail (C2's open-failure branch).
        pub fail_open: AtomicBool,
    }

    impl Default for MockCameraHandle {
        fn default() -> Self {
            Self {
                unique_id: "SVBONY:SV605CC-Simulated:SVB0123456789AB".to_string(),
                info: default_info(),
                open: AtomicBool::new(false),
                fail_open: AtomicBool::new(false),
            }
        }
    }

    impl CameraHandle for MockCameraHandle {
        fn unique_id(&self) -> String {
            self.unique_id.clone()
        }

        fn info(&self) -> CameraInfo {
            self.info.clone()
        }

        fn is_open(&self) -> bool {
            self.open.load(Ordering::SeqCst)
        }

        fn open(&self) -> BackendResult<()> {
            if self.fail_open.load(Ordering::SeqCst) {
                return Err(BackendError("simulated open failure".to_string()));
            }
            self.open.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn close(&self) -> BackendResult<()> {
            self.open.store(false, Ordering::SeqCst);
            Ok(())
        }
    }
}
