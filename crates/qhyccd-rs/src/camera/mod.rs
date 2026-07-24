mod configuration;
mod imaging;
mod info;
mod lifecycle;
mod parameters;
mod readout_modes;

use std::sync::Arc;

#[cfg(not(feature = "simulation"))]
use crate::backend::HandleCell;

#[cfg(feature = "simulation")]
use crate::simulation::{self, SimulatedCameraState};
#[cfg(feature = "simulation")]
use parking_lot::RwLock;

/// The representation of a camera. It is constructed by the SDK and can be used to
/// interact with the camera.
///
/// The real/simulated backend is chosen at **compile time** by the `simulation`
/// feature (matching the sibling `zwo-rs` / `svbony-rs` crates): without it a
/// `Camera` owns a shared FFI [`HandleCell`](crate::backend::HandleCell); with
/// it, a shared `SimulatedCameraState`. Both are shared via `Arc` so a `Camera`
/// stays `Clone` and its filter wheel drives the *same* device. Two cameras are
/// equal when they share an `id`, regardless of the live backend.
#[derive(Debug, Clone)]
pub struct Camera {
    id: String,
    /// Real backend (compiled without `simulation`): the shared open-handle cell.
    /// Closed on the last clone's drop (see [`HandleCell`](crate::backend::HandleCell)).
    #[cfg(not(feature = "simulation"))]
    handle: Arc<HandleCell>,
    /// Simulated backend (compiled with `simulation`): the shared mutable device
    /// state, so a `Camera` clone and its filter wheel act on one simulated device.
    #[cfg(feature = "simulation")]
    state: Arc<RwLock<SimulatedCameraState>>,
}

impl PartialEq for Camera {
    /// Two cameras are equal when they share an `id`, regardless of the live
    /// backend handle / simulated state (id-only equality — the former
    /// `#[partial_eq(skip)]` on the backend field).
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Camera {
    /// Creates a new instance of the camera. The Sdk automatically finds all cameras and provides them in it's cameras() iterator. Creating
    /// a camera manually should only be needed for rare cases.
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk, Camera};
    /// let camera = Camera::new("camera id from sdk".to_string());
    /// println!("Camera: {:?}", camera);
    /// ```
    #[cfg(not(feature = "simulation"))]
    pub fn new(id: String) -> Self {
        Self {
            id,
            handle: Arc::new(HandleCell::new()),
        }
    }

    /// Creates a new simulated camera instance
    ///
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::Camera;
    /// use qhyccd_rs::simulation::SimulatedCameraConfig;
    ///
    /// let config = SimulatedCameraConfig::default()
    ///     .with_filter_wheel(5)
    ///     .with_cooler();
    /// let camera = Camera::new_simulated(config);
    /// ```
    #[cfg(feature = "simulation")]
    pub fn new_simulated(config: simulation::SimulatedCameraConfig) -> Self {
        let id = config.id.clone();
        Self {
            id,
            state: Arc::new(RwLock::new(SimulatedCameraState::new(config))),
        }
    }

    /// Returns true if this is a simulated camera. Constant per build: every
    /// camera is simulated with the `simulation` feature, and none without it.
    #[cfg(feature = "simulation")]
    pub fn is_simulated(&self) -> bool {
        true
    }

    /// Returns true if this is a simulated camera (always false without the
    /// `simulation` feature).
    #[cfg(not(feature = "simulation"))]
    pub fn is_simulated(&self) -> bool {
        false
    }

    /// Returns the id of the camera
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::Sdk;
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// println!("Camera id: {}", camera.id());
    /// ```
    pub fn id(&self) -> &str {
        self.id.as_str()
    }
}
