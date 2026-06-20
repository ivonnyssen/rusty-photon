use std::sync::{Arc, RwLock};

#[cfg(feature = "simulation")]
use crate::simulation::SimulatedCameraState;

#[derive(Debug, PartialEq, Copy, Clone)]
pub(crate) struct QHYCCDHandle {
    pub ptr: *const std::ffi::c_void,
}

// SAFETY: the struct holds a raw pointer (`*const c_void`), which makes it
// `!Send + !Sync` by default — so these impls are REQUIRED for `Camera`
// (`CameraBackend::Real { handle: Arc<RwLock<Option<QHYCCDHandle>>> }`) to be
// `Send + Sync`, which it must be to move across the async runtime / blocking
// threads. The pointer is an opaque QHYCCD SDK handle that we never dereference
// in Rust; all access to the underlying device goes through the SDK's C calls,
// serialized behind the `Arc<RwLock<…>>` above (and the driver funnels SDK calls
// for one handle through a single owner), so sharing the handle across threads is
// sound.
unsafe impl Send for QHYCCDHandle {}
unsafe impl Sync for QHYCCDHandle {}

/// Internal backend for camera operations
#[derive(Debug)]
pub(crate) enum CameraBackend {
    /// Real hardware camera using FFI calls
    Real {
        handle: Arc<RwLock<Option<QHYCCDHandle>>>,
    },
    /// Simulated camera for testing
    #[cfg(feature = "simulation")]
    Simulated {
        state: Arc<RwLock<SimulatedCameraState>>,
    },
}

impl Clone for CameraBackend {
    fn clone(&self) -> Self {
        match self {
            CameraBackend::Real { handle } => CameraBackend::Real {
                handle: Arc::clone(handle),
            },
            #[cfg(feature = "simulation")]
            CameraBackend::Simulated { state } => CameraBackend::Simulated {
                state: Arc::clone(state),
            },
        }
    }
}

impl PartialEq for CameraBackend {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (CameraBackend::Real { .. }, CameraBackend::Real { .. }) => true,
            #[cfg(feature = "simulation")]
            (CameraBackend::Simulated { .. }, CameraBackend::Simulated { .. }) => true,
            #[allow(unreachable_patterns)]
            _ => false,
        }
    }
}

macro_rules! read_lock {
    ($var:expr, $wrap:expr) => {{
        use eyre::WrapErr as _;
        $var.read()
            .map_err(|err| {
                tracing::error!(error = ?err);
                eyre!("Could not acquire read lock on camera handle")
            })
            .and_then(|lock| match *lock {
                Some(handle) => Ok(handle.ptr),
                None => {
                    tracing::error!(error = ?CameraNotOpenError);
                    Err(eyre!(CameraNotOpenError))
                }
            })
            .wrap_err($wrap)
    }};
}

pub(crate) use read_lock;
