//! Real-hardware handle machinery for [`Camera`](crate::Camera).
//!
//! Compiled only **without** the `simulation` feature (`lib.rs` gates the whole
//! module): a simulated camera holds a `SimulatedCameraState` instead of a
//! [`HandleCell`], and every camera method forks at compile time on the feature
//! (see the sibling `zwo-rs` / `svbony-rs` crates). This module therefore never
//! touches the simulation types and always calls the real `libqhyccd-sys` FFI.

use parking_lot::RwLock;

#[derive(Debug, PartialEq, Copy, Clone)]
pub(crate) struct QHYCCDHandle {
    pub ptr: *const std::ffi::c_void,
}

// SAFETY: the struct holds a raw pointer (`*const c_void`), which makes it
// `!Send + !Sync` by default — so these impls are REQUIRED for `Camera`
// (whose real backend is `Arc<HandleCell>`) to be `Send + Sync`, which it must be
// to move across the async runtime / blocking threads. The pointer is an opaque
// QHYCCD SDK handle that is never dereferenced in Rust.
//
// This type does NOT itself serialize concurrent SDK calls on one handle: the
// `parking_lot::RwLock` inside `HandleCell` only guards the `Option<handle>`
// (open/close), and `read_lock!` copies the pointer out and releases the guard
// *before* the FFI call. So soundness of concurrent calls on a shared `Camera`
// relies on synchronization provided by the caller and/or the QHYCCD SDK being
// thread-safe per handle. The qhy-camera driver provides it: every SDK call runs
// on `spawn_blocking` with a single logical owner per device, so calls on one
// handle are not made concurrently.
unsafe impl Send for QHYCCDHandle {}
unsafe impl Sync for QHYCCDHandle {}

/// RAII owner of one open QHYCCD device handle. Wraps the `RwLock<Option<..>>`
/// cell so a [`Drop`] can close the device when the last strong reference is
/// released — the zwo/svbony convention (`Camera: Drop`), which this crate
/// previously lacked (a dropped-open camera leaked the handle). The cell is
/// shared as `Arc<HandleCell>` by a [`Camera`](crate::Camera) and its clones —
/// including the filter wheel, which a QHY CFW is driven through the *same*
/// camera handle — so `CloseQHYCCD` runs exactly once, on the last clone's drop.
///
/// `Drop` closes only an *open* handle (`Some`), so an explicit
/// [`Camera::close`](crate::Camera::close) (which `take()`s the `Option`) makes
/// it a no-op.
#[derive(Debug)]
pub(crate) struct HandleCell {
    inner: RwLock<Option<QHYCCDHandle>>,
}

impl HandleCell {
    /// A fresh, unopened handle cell.
    pub(crate) fn new() -> Self {
        Self {
            inner: RwLock::new(None),
        }
    }
}

// Transparent access to the underlying lock so every existing `handle.read()` /
// `handle.write()` / `read_lock!(handle)` call site keeps working unchanged.
impl std::ops::Deref for HandleCell {
    type Target = RwLock<Option<QHYCCDHandle>>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Drop for HandleCell {
    fn drop(&mut self) {
        // Best-effort last-drop close. `get_mut()` is contention-free — we hold
        // the only strong reference at drop. Closing a `None` handle is skipped,
        // so a prior explicit `close()` (or `Sdk::drop`'s pre-release close)
        // makes this a no-op rather than a double-close.
        if let Some(handle) = self.inner.get_mut().take() {
            match unsafe { crate::sys::CloseQHYCCD(handle.ptr) } {
                crate::sys::QHYCCD_SUCCESS => {}
                _ => {
                    tracing::error!(
                        error = ?crate::QHYError::Sdk { op: "close_camera" },
                        "failed to close camera handle on drop"
                    );
                }
            }
        }
    }
}

macro_rules! read_lock {
    ($var:expr) => {{
        // `parking_lot::RwLock` cannot be poisoned, so the only failure is an
        // unopened handle (`None`) — reported as `CameraNotOpen`, the accurate
        // cause, matching the simulation backend (which returns it when the camera
        // is closed) instead of a misleading operation-specific error.
        match *$var.read() {
            Some(handle) => Ok::<*const std::ffi::c_void, $crate::QHYError>(handle.ptr),
            None => {
                tracing::error!(error = ?$crate::QHYError::CameraNotOpen);
                Err($crate::QHYError::CameraNotOpen)
            }
        }
    }};
}

pub(crate) use read_lock;
