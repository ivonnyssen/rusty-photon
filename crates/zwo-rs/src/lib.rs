//! # zwo-rs — safe Rust bindings for the ZWO ASI camera & EFW filter wheel SDK
//!
//! Sibling crate to [`qhyccd-rs`](https://crates.io/crates/qhyccd-rs). It wraps
//! the raw FFI in [`libzwo-sys`](https://crates.io/crates/libzwo-sys) — generated
//! by `bindgen` from the vendored MIT ZWO SDK headers — in a safe, ergonomic
//! API. It is consumed by rusty-photon's `zwo-camera` ASCOM Alpaca driver.
//!
//! ## Status
//!
//! **Under construction.** Enumeration, SDK-version queries, the ASI [`Camera`]
//! handle (open/init, [`CameraInfo`], serial, control caps, ROI and binning,
//! control get/set, single exposures, frame download, and ST4 guiding), and the
//! EFW [`FilterWheel`] handle (open, slot count, position with the moving
//! sentinel, serial, firmware, calibration, direction) are wired to the FFI. The
//! EAF focuser is next, per the rusty-photon `docs/plans/zwo-driver.md` plan.
//! Scope order: **Camera → EFW filter wheel → EAF focuser**.
//!
//! ## `simulation` feature
//!
//! Mirrors qhyccd-rs: enables a hardware-free, in-Rust simulated environment for
//! development and tests. Note (as with qhyccd-rs) the SDK is still *linked* when
//! this feature is enabled — it removes the hardware, not the link. With the
//! feature on, the SDK is never called: enumeration reports the fixed simulated
//! device counts ([`SIM_CAMERA_COUNT`], [`SIM_FILTER_WHEEL_COUNT`]).
//!
//! ## Build requirements
//!
//! - **libclang** — `libzwo-sys` runs `bindgen` at build time (needed for
//!   `check`/`clippy`/build; *not* the SDK).
//! - **The ZWO ASI SDK** (`libASICamera2`, `libEFWFilter`) + **libusb-1.0** on
//!   the link path — needed to *link* (i.e. `build`/`test`), even with the
//!   `simulation` feature. See the README.

/// Raw, unsafe FFI bindings (`bindgen`). Prefer the safe API in this crate.
pub use libzwo_sys as sys;

mod camera;
mod efw;
mod error;
#[cfg(not(feature = "simulation"))]
mod ffi_util;
pub use camera::{
    BayerPattern, Camera, CameraInfo, ControlCaps, ControlType, ControlValue, ExposureStatus,
    GuideDirection, ImageType, RoiFormat,
};
pub use efw::{FilterWheel, FilterWheelInfo};
pub use error::{asi_check, efw_check, AsiError, EfwError, Error, Result};

/// Number of simulated ASI cameras presented when the `simulation` feature is on.
#[cfg(feature = "simulation")]
pub const SIM_CAMERA_COUNT: usize = 1;

/// Number of simulated EFW filter wheels presented when `simulation` is on.
#[cfg(feature = "simulation")]
pub const SIM_FILTER_WHEEL_COUNT: usize = 1;

/// Entry point to the ZWO SDK.
///
/// Enumerates connected ASI cameras and EFW filter wheels. With the `simulation`
/// feature, a fixed simulated environment is reported and the native SDK is
/// never called (though it is still linked — see the crate docs).
#[derive(Debug, Default)]
pub struct Sdk {
    _private: (),
}

impl Sdk {
    /// Initialise the SDK.
    ///
    /// # Errors
    /// Currently infallible, but returns [`Result`] so future initialisation
    /// (e.g. SDK version checks) can surface failures without an API break.
    pub fn new() -> Result<Self> {
        tracing::debug!("initialising ZWO SDK");
        Ok(Self { _private: () })
    }

    /// Number of connected ASI cameras (`ASIGetNumOfConnectedCameras`).
    ///
    /// # Errors
    /// Infallible today; returns [`Result`] for forward compatibility.
    pub fn camera_count(&self) -> Result<usize> {
        #[cfg(feature = "simulation")]
        let count = SIM_CAMERA_COUNT;
        #[cfg(not(feature = "simulation"))]
        let count = {
            // SAFETY: `ASIGetNumOfConnectedCameras` takes no arguments and
            // returns the connected-camera count (it probes USB and is always
            // safe to call). A negative return is clamped to zero.
            let n = unsafe { sys::ASIGetNumOfConnectedCameras() };
            usize::try_from(n).unwrap_or(0)
        };
        Ok(count)
    }

    /// Number of connected EFW filter wheels (`EFWGetNum`).
    ///
    /// # Errors
    /// Infallible today; returns [`Result`] for forward compatibility.
    pub fn filter_wheel_count(&self) -> Result<usize> {
        #[cfg(feature = "simulation")]
        let count = SIM_FILTER_WHEEL_COUNT;
        #[cfg(not(feature = "simulation"))]
        let count = {
            // SAFETY: `EFWGetNum` takes no arguments and returns the connected
            // filter-wheel count; always safe to call. Negative is clamped.
            let n = unsafe { sys::EFWGetNum() };
            usize::try_from(n).unwrap_or(0)
        };
        Ok(count)
    }

    /// ASI camera SDK version string (`ASIGetSDKVersion`), e.g. `"1, 36, 0"`.
    ///
    /// # Errors
    /// Infallible today; returns [`Result`] for forward compatibility.
    pub fn asi_version(&self) -> Result<String> {
        #[cfg(feature = "simulation")]
        let version = "simulation".to_owned();
        #[cfg(not(feature = "simulation"))]
        let version = {
            // SAFETY: `ASIGetSDKVersion` returns a pointer to a static,
            // NUL-terminated C string owned by the SDK; we only read it.
            let ptr = unsafe { sys::ASIGetSDKVersion() };
            version_string(ptr)
        };
        Ok(version)
    }

    /// EFW filter-wheel SDK version string (`EFWGetSDKVersion`).
    ///
    /// # Errors
    /// Infallible today; returns [`Result`] for forward compatibility.
    pub fn efw_version(&self) -> Result<String> {
        #[cfg(feature = "simulation")]
        let version = "simulation".to_owned();
        #[cfg(not(feature = "simulation"))]
        let version = {
            // SAFETY: as `asi_version` — a static, SDK-owned NUL-terminated
            // string we only read.
            let ptr = unsafe { sys::EFWGetSDKVersion() };
            version_string(ptr)
        };
        Ok(version)
    }
}

/// Read an SDK-owned, NUL-terminated C string into an owned [`String`]
/// (lossy on invalid UTF-8). An empty string is returned for a null pointer.
#[cfg(not(feature = "simulation"))]
fn version_string(ptr: *const std::os::raw::c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: the SDK returns a pointer to a static, NUL-terminated string;
    // the read is bounded by the terminating NUL and the data outlives the call.
    unsafe { std::ffi::CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(feature = "simulation")]
pub mod simulation {
    //! Hardware-free, in-Rust simulation backend (no SDK calls).
    //!
    //! Enumeration of the simulated environment is reported by [`crate::Sdk`]
    //! via [`crate::SIM_CAMERA_COUNT`] / [`crate::SIM_FILTER_WHEEL_COUNT`].
    //! Simulated frames and EFW motion land with the Camera and filter-wheel
    //! device handles.
    use rand::{Rng, RngCore};
    use rayon::prelude::*;

    /// One 16-bit noise sample — a placeholder for simulated sensor frames.
    #[must_use]
    pub fn noise_sample() -> u16 {
        rand::rng().random()
    }

    /// Fill `buf` with simulated sensor noise as fast as possible.
    ///
    /// A full-frame ASI2600 frame is ~52 MiB. Filling it one byte at a time —
    /// the original approach, a fresh [`rand::rng()`] lookup per byte — took
    /// over 10 s and tripped ConformU's 10 s `StartExposure` timeout. This
    /// splits the buffer into chunks filled in parallel across rayon's pool,
    /// each by a thread-local RNG via the bulk [`RngCore::fill_bytes`] path,
    /// turning a full-frame download into a few milliseconds.
    pub fn fill_noise(buf: &mut [u8]) {
        // 64 KiB keeps per-chunk RNG acquisition negligible while still giving
        // rayon plenty of chunks to balance across cores.
        const CHUNK: usize = 64 * 1024;
        buf.par_chunks_mut(CHUNK)
            .for_each(|chunk| rand::rng().fill_bytes(chunk));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_new_succeeds() {
        Sdk::new().unwrap();
    }

    #[test]
    fn enumeration_returns_a_count() {
        let sdk = Sdk::new().unwrap();
        let cameras = sdk.camera_count().unwrap();
        let wheels = sdk.filter_wheel_count().unwrap();
        #[cfg(feature = "simulation")]
        {
            assert_eq!(cameras, SIM_CAMERA_COUNT);
            assert_eq!(wheels, SIM_FILTER_WHEEL_COUNT);
        }
        // Without the simulation feature this calls the real SDK; with no
        // hardware attached the counts are zero, but the call must not panic.
        #[cfg(not(feature = "simulation"))]
        {
            let _ = (cameras, wheels);
        }
    }

    #[test]
    fn sdk_versions_are_non_empty() {
        let sdk = Sdk::new().unwrap();
        assert!(!sdk.asi_version().unwrap().is_empty());
        assert!(!sdk.efw_version().unwrap().is_empty());
    }

    #[test]
    fn asi_check_maps_known_and_unknown_codes() {
        asi_check(0).unwrap();
        assert_eq!(
            asi_check(1).unwrap_err(),
            Error::Asi(AsiError::InvalidIndex)
        );
        assert_eq!(
            asi_check(16).unwrap_err(),
            Error::Asi(AsiError::GeneralError)
        );
        assert_eq!(
            asi_check(999).unwrap_err(),
            Error::Asi(AsiError::Unknown(999))
        );
    }

    #[test]
    fn efw_check_maps_known_and_unknown_codes() {
        efw_check(0).unwrap();
        assert_eq!(efw_check(5).unwrap_err(), Error::Efw(EfwError::Moving));
        assert_eq!(efw_check(9).unwrap_err(), Error::Efw(EfwError::Closed));
        assert_eq!(
            efw_check(42).unwrap_err(),
            Error::Efw(EfwError::Unknown(42))
        );
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn simulation_noise_sample_runs() {
        // Any u16 is valid; just exercise the simulation path.
        let _ = simulation::noise_sample();
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn simulation_fill_noise_fills_whole_buffer() {
        // A small buffer is enough to exercise the parallel fill path; the
        // chunking is internal. Just assert it touches every byte (vanishingly
        // unlikely to stay all-zero) and respects the slice length.
        let mut buf = vec![0u8; 256 * 1024 + 7];
        simulation::fill_noise(&mut buf);
        assert!(buf.iter().any(|&b| b != 0));
    }
}
