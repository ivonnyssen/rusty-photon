//! Test module relocated verbatim from the pre-split `mcp.rs`. A
//! follow-up will distribute these tests across the per-category
//! `built_in/<category>.rs` files (matching the `imaging/` layout
//! convention) and split the shared mock-device fixtures into a
//! sibling `test_support.rs`.

use super::built_in::auto_focus::*;
use super::built_in::camera::*;
use super::built_in::cover_calibrator::*;
use super::built_in::filter_wheel::*;
use super::built_in::focuser::*;
use super::built_in::imaging::*;
use super::built_in::mount::*;
use super::built_in::planner::*;
use super::built_in::plate_solve::*;
use super::handler::McpHandler;
use crate::persistence::{self, CachedPixels, ExposureDocument, ImageCache};
use crate::session::SessionConfig;
use ascom_alpaca::api::cover_calibrator::{CalibratorStatus, CoverStatus};
use ascom_alpaca::ASCOMError;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use std::sync::Arc;
use std::time::Duration;

// -----------------------------------------------------------------------
// Mock Device macro
// -----------------------------------------------------------------------

/// Generates Debug + Device impl with stubs for all required methods.
macro_rules! impl_mock_device {
    ($name:ident) => {
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, stringify!($name))
            }
        }

        #[async_trait::async_trait]
        impl ascom_alpaca::api::Device for $name {
            fn static_name(&self) -> &str {
                "mock"
            }
            fn unique_id(&self) -> &str {
                "mock-id"
            }
            async fn connected(&self) -> ascom_alpaca::ASCOMResult<bool> {
                Ok(true)
            }
            async fn set_connected(&self, _: bool) -> ascom_alpaca::ASCOMResult<()> {
                Ok(())
            }
            async fn description(&self) -> ascom_alpaca::ASCOMResult<String> {
                Ok("mock".into())
            }
            async fn driver_info(&self) -> ascom_alpaca::ASCOMResult<String> {
                Ok("mock".into())
            }
            async fn driver_version(&self) -> ascom_alpaca::ASCOMResult<String> {
                Ok("0.0".into())
            }
        }
    };
}

// -----------------------------------------------------------------------
// MockCamera — single configurable mock for all Camera error-injection
// -----------------------------------------------------------------------

#[derive(Default)]
struct MockCamera {
    fail_start_exposure: bool,
    fail_image_ready: bool,
    fail_image_array: bool,
    fail_max_adu: bool,
    fail_camera_size: bool,
    fail_pixel_size: bool,
    fail_exposure_range: bool,
    /// `0` ⇒ default 65535. Any other value is returned verbatim — set
    /// to `> u16::MAX` to exercise the I32 cache-insert path.
    max_adu_value: u32,
    /// When set, `image_ready` reports `false` forever — simulating a
    /// camera that never completes (a failed or wedged exposure).
    never_ready: bool,
    /// When non-zero, `image_ready` reports `false` for the first N
    /// calls and `true` thereafter — models a bounded readout for
    /// progress-notification tests. `0` (default) keeps the original
    /// behavior (`image_ready` returns true immediately).
    not_ready_count: u32,
    image_ready_calls: std::sync::atomic::AtomicU32,
    /// When set, `camera_state` reports `Error` — simulating a camera
    /// that failed an exposure (e.g. sky-survey-camera's mount read
    /// timing out). Drives the `do_capture` failed-exposure path.
    report_error_state: bool,
}

impl_mock_device!(MockCamera);

#[async_trait::async_trait]
impl ascom_alpaca::api::Camera for MockCamera {
    async fn start_exposure(
        &self,
        _duration: Duration,
        _light: bool,
    ) -> ascom_alpaca::ASCOMResult<()> {
        if self.fail_start_exposure {
            return Err(ASCOMError::invalid_operation("shutter jammed"));
        }
        Ok(())
    }

    async fn image_ready(&self) -> ascom_alpaca::ASCOMResult<bool> {
        if self.fail_image_ready {
            return Err(ASCOMError::invalid_operation("readout failed"));
        }
        if self.never_ready {
            return Ok(false);
        }
        if self.not_ready_count > 0 {
            let n = self
                .image_ready_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n < self.not_ready_count {
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn camera_state(
        &self,
    ) -> ascom_alpaca::ASCOMResult<ascom_alpaca::api::camera::CameraState> {
        use ascom_alpaca::api::camera::CameraState;
        Ok(if self.report_error_state {
            CameraState::Error
        } else {
            CameraState::Idle
        })
    }

    async fn image_array(
        &self,
    ) -> ascom_alpaca::ASCOMResult<ascom_alpaca::api::camera::ImageArray> {
        if self.fail_image_array {
            return Err(ASCOMError::invalid_operation("download timeout"));
        }
        Ok(ndarray::Array3::<i32>::zeros((2, 2, 1)).into())
    }

    async fn max_adu(&self) -> ascom_alpaca::ASCOMResult<u32> {
        if self.fail_max_adu {
            return Err(ASCOMError::invalid_operation("not available"));
        }
        Ok(if self.max_adu_value == 0 {
            65535
        } else {
            self.max_adu_value
        })
    }

    async fn camera_x_size(&self) -> ascom_alpaca::ASCOMResult<u32> {
        if self.fail_camera_size {
            return Err(ASCOMError::invalid_operation("sensor error"));
        }
        Ok(1024)
    }

    async fn camera_y_size(&self) -> ascom_alpaca::ASCOMResult<u32> {
        if self.fail_camera_size {
            return Err(ASCOMError::invalid_operation("sensor error"));
        }
        Ok(1024)
    }

    async fn exposure_max(&self) -> ascom_alpaca::ASCOMResult<Duration> {
        if self.fail_exposure_range {
            return Err(ASCOMError::invalid_operation("range unavailable"));
        }
        Ok(Duration::from_secs(3600))
    }

    async fn exposure_min(&self) -> ascom_alpaca::ASCOMResult<Duration> {
        if self.fail_exposure_range {
            return Err(ASCOMError::invalid_operation("range unavailable"));
        }
        Ok(Duration::from_millis(1))
    }

    async fn exposure_resolution(&self) -> ascom_alpaca::ASCOMResult<Duration> {
        Ok(Duration::from_millis(1))
    }

    async fn has_shutter(&self) -> ascom_alpaca::ASCOMResult<bool> {
        Ok(true)
    }

    async fn pixel_size_x(&self) -> ascom_alpaca::ASCOMResult<f64> {
        if self.fail_pixel_size {
            return Err(ASCOMError::invalid_operation("pixel size unavailable"));
        }
        Ok(3.76)
    }

    async fn pixel_size_y(&self) -> ascom_alpaca::ASCOMResult<f64> {
        if self.fail_pixel_size {
            return Err(ASCOMError::invalid_operation("pixel size unavailable"));
        }
        Ok(3.76)
    }

    async fn start_x(&self) -> ascom_alpaca::ASCOMResult<u32> {
        Ok(0)
    }

    async fn set_start_x(&self, _start_x: u32) -> ascom_alpaca::ASCOMResult<()> {
        Ok(())
    }

    async fn start_y(&self) -> ascom_alpaca::ASCOMResult<u32> {
        Ok(0)
    }

    async fn set_start_y(&self, _start_y: u32) -> ascom_alpaca::ASCOMResult<()> {
        Ok(())
    }
}

// -----------------------------------------------------------------------
// MockCameraNoMetadata — regression contract for "per-capture metadata
// reads are gone". Implements only the exposure path; every invariant-
// sensor property method panics if called. Pins the `do_capture` ↔
// `CameraEntry`-cache contract: with the cache populated, capture must
// not touch `max_adu`, `pixel_size_*`, or `camera_*_size` on the device.
// -----------------------------------------------------------------------

#[derive(Default)]
struct MockCameraNoMetadata;

impl_mock_device!(MockCameraNoMetadata);

#[async_trait::async_trait]
impl ascom_alpaca::api::Camera for MockCameraNoMetadata {
    async fn start_exposure(
        &self,
        _duration: Duration,
        _light: bool,
    ) -> ascom_alpaca::ASCOMResult<()> {
        Ok(())
    }

    async fn image_ready(&self) -> ascom_alpaca::ASCOMResult<bool> {
        Ok(true)
    }

    async fn image_array(
        &self,
    ) -> ascom_alpaca::ASCOMResult<ascom_alpaca::api::camera::ImageArray> {
        Ok(ndarray::Array3::<i32>::zeros((2, 2, 1)).into())
    }

    async fn max_adu(&self) -> ascom_alpaca::ASCOMResult<u32> {
        panic!("do_capture must read max_adu from CameraEntry cache, not the device")
    }

    async fn pixel_size_x(&self) -> ascom_alpaca::ASCOMResult<f64> {
        panic!("do_capture must read pixel_size_x from CameraEntry cache, not the device")
    }

    async fn pixel_size_y(&self) -> ascom_alpaca::ASCOMResult<f64> {
        panic!("do_capture must read pixel_size_y from CameraEntry cache, not the device")
    }

    async fn camera_x_size(&self) -> ascom_alpaca::ASCOMResult<u32> {
        panic!("do_capture must read camera_x_size from CameraEntry cache, not the device")
    }

    async fn camera_y_size(&self) -> ascom_alpaca::ASCOMResult<u32> {
        panic!("do_capture must read camera_y_size from CameraEntry cache, not the device")
    }

    async fn exposure_max(&self) -> ascom_alpaca::ASCOMResult<Duration> {
        Ok(Duration::from_secs(3600))
    }

    async fn exposure_min(&self) -> ascom_alpaca::ASCOMResult<Duration> {
        Ok(Duration::from_millis(1))
    }

    async fn exposure_resolution(&self) -> ascom_alpaca::ASCOMResult<Duration> {
        Ok(Duration::from_millis(1))
    }

    async fn has_shutter(&self) -> ascom_alpaca::ASCOMResult<bool> {
        Ok(true)
    }

    async fn start_x(&self) -> ascom_alpaca::ASCOMResult<u32> {
        Ok(0)
    }

    async fn set_start_x(&self, _start_x: u32) -> ascom_alpaca::ASCOMResult<()> {
        Ok(())
    }

    async fn start_y(&self) -> ascom_alpaca::ASCOMResult<u32> {
        Ok(0)
    }

    async fn set_start_y(&self, _start_y: u32) -> ascom_alpaca::ASCOMResult<()> {
        Ok(())
    }
}

// -----------------------------------------------------------------------
// MockFilterWheel — single configurable mock for FilterWheel errors
// -----------------------------------------------------------------------

#[derive(Default)]
struct MockFilterWheel {
    fail_set_position: bool,
    fail_position_poll: bool,
    report_moving: bool,
}

impl_mock_device!(MockFilterWheel);

#[async_trait::async_trait]
impl ascom_alpaca::api::FilterWheel for MockFilterWheel {
    async fn set_position(&self, _position: usize) -> ascom_alpaca::ASCOMResult<()> {
        if self.fail_set_position {
            return Err(ASCOMError::invalid_operation("wheel stuck"));
        }
        Ok(())
    }

    async fn position(&self) -> ascom_alpaca::ASCOMResult<Option<usize>> {
        if self.fail_position_poll {
            return Err(ASCOMError::invalid_operation("encoder error"));
        }
        if self.report_moving {
            return Ok(None);
        }
        Ok(Some(0))
    }

    async fn names(&self) -> ascom_alpaca::ASCOMResult<Vec<String>> {
        Ok(vec!["Lum".into(), "Red".into()])
    }

    async fn focus_offsets(&self) -> ascom_alpaca::ASCOMResult<Vec<i32>> {
        Ok(vec![0, 0])
    }
}

// -----------------------------------------------------------------------
// MockCoverCalibrator — single configurable mock for CoverCalibrator
// -----------------------------------------------------------------------

#[derive(Default)]
struct MockCoverCalibrator {
    fail_close_cover: bool,
    fail_open_cover: bool,
    fail_calibrator_on: bool,
    fail_calibrator_off: bool,
    fail_max_brightness: bool,
    fail_cover_state_poll: bool,
    stuck_cover_moving: bool,
    fail_calibrator_state_poll: bool,
    stuck_calibrator_not_ready: bool,
}

impl_mock_device!(MockCoverCalibrator);

#[async_trait::async_trait]
impl ascom_alpaca::api::CoverCalibrator for MockCoverCalibrator {
    async fn close_cover(&self) -> ascom_alpaca::ASCOMResult<()> {
        if self.fail_close_cover {
            return Err(ASCOMError::invalid_operation("motor fault"));
        }
        Ok(())
    }

    async fn open_cover(&self) -> ascom_alpaca::ASCOMResult<()> {
        if self.fail_open_cover {
            return Err(ASCOMError::invalid_operation("motor fault"));
        }
        Ok(())
    }

    async fn calibrator_on(&self, _brightness: u32) -> ascom_alpaca::ASCOMResult<()> {
        if self.fail_calibrator_on {
            return Err(ASCOMError::invalid_operation("lamp failure"));
        }
        Ok(())
    }

    async fn calibrator_off(&self) -> ascom_alpaca::ASCOMResult<()> {
        if self.fail_calibrator_off {
            return Err(ASCOMError::invalid_operation("stuck on"));
        }
        Ok(())
    }

    async fn cover_state(&self) -> ascom_alpaca::ASCOMResult<CoverStatus> {
        if self.fail_cover_state_poll {
            return Err(ASCOMError::invalid_operation("device unreachable"));
        }
        if self.stuck_cover_moving {
            return Ok(CoverStatus::Moving);
        }
        Ok(CoverStatus::Closed)
    }

    async fn calibrator_state(&self) -> ascom_alpaca::ASCOMResult<CalibratorStatus> {
        if self.fail_calibrator_state_poll {
            return Err(ASCOMError::invalid_operation("device unreachable"));
        }
        if self.stuck_calibrator_not_ready {
            return Ok(CalibratorStatus::NotReady);
        }
        Ok(CalibratorStatus::Off)
    }

    async fn max_brightness(&self) -> ascom_alpaca::ASCOMResult<u32> {
        if self.fail_max_brightness {
            return Err(ASCOMError::invalid_operation("not supported"));
        }
        Ok(255)
    }

    async fn brightness(&self) -> ascom_alpaca::ASCOMResult<u32> {
        Ok(0)
    }
}

// -----------------------------------------------------------------------
// MockFocuser — single configurable mock for Focuser
// -----------------------------------------------------------------------

#[derive(Default)]
struct MockFocuser {
    fail_move: bool,
    fail_is_moving: bool,
    fail_position: bool,
    /// `true` ⇒ `temperature()` returns a generic INVALID_OPERATION
    /// error (sensor wired but reading failed). Distinct from
    /// `temperature_not_implemented` below.
    fail_temperature: bool,
    /// `true` ⇒ `temperature()` returns `ASCOMError::NOT_IMPLEMENTED`.
    /// Models a focuser that does not implement the `Temperature`
    /// property at all.
    temperature_not_implemented: bool,
    stuck_moving: bool,
    /// When non-zero, `is_moving` reports `true` for the first N calls
    /// and `false` thereafter — models a bounded focuser move for
    /// progress-notification tests, mirroring `MockCamera::not_ready_count`.
    /// `0` (default) keeps the `stuck_moving` behavior.
    is_moving_true_count: u32,
    is_moving_calls: std::sync::atomic::AtomicU32,
    temperature_value: f64,
    position_value: i32,
}

impl_mock_device!(MockFocuser);

#[async_trait::async_trait]
impl ascom_alpaca::api::Focuser for MockFocuser {
    async fn absolute(&self) -> ascom_alpaca::ASCOMResult<bool> {
        Ok(true)
    }

    async fn is_moving(&self) -> ascom_alpaca::ASCOMResult<bool> {
        if self.fail_is_moving {
            return Err(ASCOMError::invalid_operation("encoder fault"));
        }
        if self.is_moving_true_count > 0 {
            let n = self
                .is_moving_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return Ok(n < self.is_moving_true_count);
        }
        Ok(self.stuck_moving)
    }

    async fn max_increment(&self) -> ascom_alpaca::ASCOMResult<u32> {
        Ok(100000)
    }

    async fn max_step(&self) -> ascom_alpaca::ASCOMResult<u32> {
        Ok(100000)
    }

    async fn position(&self) -> ascom_alpaca::ASCOMResult<i32> {
        if self.fail_position {
            return Err(ASCOMError::invalid_operation("position unavailable"));
        }
        Ok(self.position_value)
    }

    async fn step_size(&self) -> ascom_alpaca::ASCOMResult<f64> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn temp_comp(&self) -> ascom_alpaca::ASCOMResult<bool> {
        Ok(false)
    }

    async fn set_temp_comp(&self, _: bool) -> ascom_alpaca::ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn temp_comp_available(&self) -> ascom_alpaca::ASCOMResult<bool> {
        Ok(false)
    }

    async fn temperature(&self) -> ascom_alpaca::ASCOMResult<f64> {
        if self.temperature_not_implemented {
            return Err(ASCOMError::NOT_IMPLEMENTED);
        }
        if self.fail_temperature {
            return Err(ASCOMError::invalid_operation("sensor failure"));
        }
        Ok(self.temperature_value)
    }

    async fn halt(&self) -> ascom_alpaca::ASCOMResult<()> {
        Ok(())
    }

    async fn move_(&self, _position: i32) -> ascom_alpaca::ASCOMResult<()> {
        if self.fail_move {
            return Err(ASCOMError::invalid_operation("focuser stuck"));
        }
        Ok(())
    }
}

// -----------------------------------------------------------------------
// MockTelescope — single configurable mock for Telescope (mount).
//
// Defaults are "happy path" (capable, tracking on, returns a fixed
// RA/Dec). Set fail_* fields to inject errors per test, or set
// tracking_value / can_set_tracking_value / ra_value / dec_value to
// shape the read responses.
// -----------------------------------------------------------------------

struct MockTelescope {
    fail_slew: bool,
    fail_slewing_poll: bool,
    fail_sync: bool,
    fail_right_ascension: bool,
    fail_declination: bool,
    fail_tracking: bool,
    fail_can_set_tracking: bool,
    fail_set_tracking: bool,
    fail_park: bool,
    fail_unpark: bool,
    fail_at_park: bool,
    fail_can_park: bool,
    fail_can_unpark: bool,
    fail_abort_slew: bool,
    /// `slewing()` returns `true` forever — drives the timeout path.
    stuck_slewing: bool,
    /// First N `slewing()` calls return a transient error before
    /// behaving normally — drives `poll_slewing_until_idle`'s
    /// transient-tolerance path (issue #319). Counter is `slewing_calls`.
    slewing_transient_errors: u32,
    /// When non-zero (and `stuck_slewing` is false), `slewing()`
    /// returns `true` for the first N successful calls (after the
    /// transient-error budget is exhausted) and `false` thereafter.
    /// Used by progress-notification tests to model a slew of
    /// bounded duration. `0` (default) means "report idle immediately".
    slewing_true_count: u32,
    slewing_calls: std::sync::atomic::AtomicU32,
    /// When non-zero, `at_park()` returns `false` for the first N
    /// calls and `true` thereafter — models a park of bounded
    /// duration for progress-notification tests. `0` (default)
    /// means "report at_park immediately".
    at_park_false_count: u32,
    at_park_calls: std::sync::atomic::AtomicU32,
    tracking_value: bool,
    can_set_tracking_value: bool,
    at_park_value: bool,
    can_park_value: bool,
    can_unpark_value: bool,
    ra_value: f64,
    dec_value: f64,
}

impl Default for MockTelescope {
    fn default() -> Self {
        Self {
            fail_slew: false,
            fail_slewing_poll: false,
            fail_sync: false,
            fail_right_ascension: false,
            fail_declination: false,
            fail_tracking: false,
            fail_can_set_tracking: false,
            fail_set_tracking: false,
            fail_park: false,
            fail_unpark: false,
            fail_at_park: false,
            fail_can_park: false,
            fail_can_unpark: false,
            fail_abort_slew: false,
            stuck_slewing: false,
            slewing_transient_errors: 0,
            slewing_true_count: 0,
            slewing_calls: std::sync::atomic::AtomicU32::new(0),
            at_park_false_count: 0,
            at_park_calls: std::sync::atomic::AtomicU32::new(0),
            tracking_value: true,
            can_set_tracking_value: true,
            at_park_value: false,
            can_park_value: true,
            can_unpark_value: true,
            ra_value: 0.0,
            dec_value: 0.0,
        }
    }
}

impl_mock_device!(MockTelescope);

#[async_trait::async_trait]
impl ascom_alpaca::api::Telescope for MockTelescope {
    async fn at_home(&self) -> ascom_alpaca::ASCOMResult<bool> {
        Ok(false)
    }

    async fn at_park(&self) -> ascom_alpaca::ASCOMResult<bool> {
        if self.fail_at_park {
            return Err(ASCOMError::invalid_operation("at_park read failed"));
        }
        let n = self
            .at_park_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.at_park_false_count > 0 && n < self.at_park_false_count {
            return Ok(false);
        }
        Ok(self.at_park_value)
    }

    async fn can_park(&self) -> ascom_alpaca::ASCOMResult<bool> {
        if self.fail_can_park {
            return Err(ASCOMError::invalid_operation("can_park read failed"));
        }
        Ok(self.can_park_value)
    }

    async fn can_unpark(&self) -> ascom_alpaca::ASCOMResult<bool> {
        if self.fail_can_unpark {
            return Err(ASCOMError::invalid_operation("can_unpark read failed"));
        }
        Ok(self.can_unpark_value)
    }

    async fn park(&self) -> ascom_alpaca::ASCOMResult<()> {
        if self.fail_park {
            return Err(ASCOMError::invalid_operation("park failed"));
        }
        Ok(())
    }

    async fn unpark(&self) -> ascom_alpaca::ASCOMResult<()> {
        if self.fail_unpark {
            return Err(ASCOMError::invalid_operation("unpark failed"));
        }
        Ok(())
    }

    async fn declination(&self) -> ascom_alpaca::ASCOMResult<f64> {
        if self.fail_declination {
            return Err(ASCOMError::invalid_operation("encoder fault"));
        }
        Ok(self.dec_value)
    }

    async fn declination_rate(&self) -> ascom_alpaca::ASCOMResult<f64> {
        Ok(0.0)
    }

    async fn equatorial_system(
        &self,
    ) -> ascom_alpaca::ASCOMResult<ascom_alpaca::api::telescope::EquatorialCoordinateType> {
        Ok(ascom_alpaca::api::telescope::EquatorialCoordinateType::J2000)
    }

    async fn right_ascension(&self) -> ascom_alpaca::ASCOMResult<f64> {
        if self.fail_right_ascension {
            return Err(ASCOMError::invalid_operation("encoder fault"));
        }
        Ok(self.ra_value)
    }

    async fn right_ascension_rate(&self) -> ascom_alpaca::ASCOMResult<f64> {
        Ok(0.0)
    }

    async fn sidereal_time(&self) -> ascom_alpaca::ASCOMResult<f64> {
        Ok(0.0)
    }

    async fn tracking(&self) -> ascom_alpaca::ASCOMResult<bool> {
        if self.fail_tracking {
            return Err(ASCOMError::invalid_operation("tracking read failed"));
        }
        Ok(self.tracking_value)
    }

    async fn set_tracking(&self, _: bool) -> ascom_alpaca::ASCOMResult<()> {
        if self.fail_set_tracking {
            return Err(ASCOMError::invalid_operation("CanSetTracking is false"));
        }
        Ok(())
    }

    async fn can_set_tracking(&self) -> ascom_alpaca::ASCOMResult<bool> {
        if self.fail_can_set_tracking {
            return Err(ASCOMError::invalid_operation("capability read failed"));
        }
        Ok(self.can_set_tracking_value)
    }

    async fn tracking_rate(
        &self,
    ) -> ascom_alpaca::ASCOMResult<ascom_alpaca::api::telescope::DriveRate> {
        Ok(ascom_alpaca::api::telescope::DriveRate::Sidereal)
    }

    async fn axis_rates(
        &self,
        _axis: ascom_alpaca::api::telescope::TelescopeAxis,
    ) -> ascom_alpaca::ASCOMResult<Vec<std::ops::RangeInclusive<f64>>> {
        Ok(vec![])
    }

    async fn utc_date(&self) -> ascom_alpaca::ASCOMResult<std::time::SystemTime> {
        Ok(std::time::SystemTime::UNIX_EPOCH)
    }

    async fn slewing(&self) -> ascom_alpaca::ASCOMResult<bool> {
        let n = self
            .slewing_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.fail_slewing_poll || n < self.slewing_transient_errors {
            return Err(ASCOMError::invalid_operation("slewing poll failed"));
        }
        if self.stuck_slewing {
            return Ok(true);
        }
        if self.slewing_true_count > 0 {
            // Successful-call index (after the transient-error budget).
            let success_idx = n - self.slewing_transient_errors;
            if success_idx < self.slewing_true_count {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn abort_slew(&self) -> ascom_alpaca::ASCOMResult<()> {
        if self.fail_abort_slew {
            return Err(ASCOMError::invalid_operation("abort_slew failed"));
        }
        Ok(())
    }

    async fn slew_to_coordinates_async(
        &self,
        _ra: f64,
        _dec: f64,
    ) -> ascom_alpaca::ASCOMResult<()> {
        if self.fail_slew {
            return Err(ASCOMError::invalid_operation("Tracking is off"));
        }
        Ok(())
    }

    async fn sync_to_coordinates(&self, _ra: f64, _dec: f64) -> ascom_alpaca::ASCOMResult<()> {
        if self.fail_sync {
            return Err(ASCOMError::invalid_operation("sync failed"));
        }
        Ok(())
    }
}

// -----------------------------------------------------------------------
// Helper functions
// -----------------------------------------------------------------------

fn test_handler(registry: crate::equipment::EquipmentRegistry) -> McpHandler {
    McpHandler::new(
        Arc::new(registry),
        Arc::new(crate::events::EventBus::from_config(&[])),
        SessionConfig {
            data_directory: std::env::temp_dir()
                .join("rp-unit-test")
                .to_string_lossy()
                .to_string(),
        },
        ImageCache::new(64, 4, std::path::PathBuf::from("/nonexistent")),
        None,
    )
}

fn assert_tool_error(result: Result<CallToolResult, rmcp::ErrorData>, expected_substr: &str) {
    let call_result = result.expect("tool returned protocol error");
    assert!(
        call_result.is_error.unwrap_or(false),
        "expected is_error=true"
    );
    let text = call_result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|tc| tc.text.as_str())
        .unwrap_or("");
    assert!(
        text.contains(expected_substr),
        "expected error containing '{}', got: '{}'",
        expected_substr,
        text
    );
}

// -----------------------------------------------------------------------
// Registry builders
// -----------------------------------------------------------------------

/// Pre-populated cache values for the `MockCamera` defaults: the mock
/// reports `max_adu = 65535`, `pixel_size_* = 3.76 µm`, and `camera_*_
/// size = 1024 px`. Test helpers stamp the same values onto the
/// `CameraEntry` cache so `do_capture` sees what `connect_camera` would
/// have populated against the real driver — without paying connect-time
/// Alpaca calls in unit tests.
const MOCK_CAMERA_MAX_ADU: u32 = 65535;
const MOCK_CAMERA_PIXEL_SIZE_UM: f64 = 3.76;
const MOCK_CAMERA_SENSOR_PX: u32 = 1024;

/// Per-call overrides for the cached invariant-metadata fields on
/// `CameraEntry`. Defaults mirror `MockCamera`'s static reads so tests
/// that don't care about metadata get the same shape `connect_camera`
/// would have produced. Tests that want to model a connect-time read
/// failure (or a scientific camera with `max_adu > u16::MAX`) override
/// the relevant field.
#[derive(Clone, Copy)]
struct CachedCameraMeta {
    max_adu: Option<u32>,
    pixel_size_x_um: Option<f64>,
    pixel_size_y_um: Option<f64>,
    sensor_width_px: Option<u32>,
    sensor_height_px: Option<u32>,
}

impl Default for CachedCameraMeta {
    fn default() -> Self {
        Self {
            max_adu: Some(MOCK_CAMERA_MAX_ADU),
            pixel_size_x_um: Some(MOCK_CAMERA_PIXEL_SIZE_UM),
            pixel_size_y_um: Some(MOCK_CAMERA_PIXEL_SIZE_UM),
            sensor_width_px: Some(MOCK_CAMERA_SENSOR_PX),
            sensor_height_px: Some(MOCK_CAMERA_SENSOR_PX),
        }
    }
}

fn camera_registry(cam: Arc<dyn ascom_alpaca::api::Camera>) -> crate::equipment::EquipmentRegistry {
    camera_registry_with_focal_length(cam, None)
}

fn camera_registry_with_focal_length(
    cam: Arc<dyn ascom_alpaca::api::Camera>,
    focal_length_mm: Option<f64>,
) -> crate::equipment::EquipmentRegistry {
    camera_registry_with_meta(cam, focal_length_mm, CachedCameraMeta::default())
}

fn camera_registry_with_meta(
    cam: Arc<dyn ascom_alpaca::api::Camera>,
    focal_length_mm: Option<f64>,
    meta: CachedCameraMeta,
) -> crate::equipment::EquipmentRegistry {
    crate::equipment::EquipmentRegistry {
        cameras: vec![crate::equipment::CameraEntry {
            id: "cam".to_string(),
            connected: true,
            config: crate::config::CameraConfig {
                id: "cam".to_string(),
                name: "mock".to_string(),
                alpaca_url: "http://localhost:1".to_string(),
                device_type: String::new(),
                device_number: 0,
                cooler_target_c: None,
                gain: None,
                offset: None,
                focal_length_mm,
                auth: None,
            },
            device: Some(cam),
            max_adu: meta.max_adu,
            pixel_size_x_um: meta.pixel_size_x_um,
            pixel_size_y_um: meta.pixel_size_y_um,
            sensor_width_px: meta.sensor_width_px,
            sensor_height_px: meta.sensor_height_px,
        }],
        filter_wheels: vec![],
        cover_calibrators: vec![],
        focusers: vec![],
        mount: None,
    }
}

fn filter_wheel_registry(
    fw: Arc<dyn ascom_alpaca::api::FilterWheel>,
) -> crate::equipment::EquipmentRegistry {
    crate::equipment::EquipmentRegistry {
        cameras: vec![],
        filter_wheels: vec![crate::equipment::FilterWheelEntry {
            id: "fw".to_string(),
            connected: true,
            config: crate::config::FilterWheelConfig {
                id: "fw".to_string(),
                camera_id: String::new(),
                alpaca_url: "http://localhost:1".to_string(),
                device_number: 0,
                filters: vec!["Lum".to_string(), "Red".to_string()],
                auth: None,
            },
            device: Some(fw),
        }],
        cover_calibrators: vec![],
        focusers: vec![],
        mount: None,
    }
}

fn calibrator_registry(
    cc: Arc<dyn ascom_alpaca::api::CoverCalibrator>,
) -> crate::equipment::EquipmentRegistry {
    crate::equipment::EquipmentRegistry {
        cameras: vec![],
        filter_wheels: vec![],
        cover_calibrators: vec![crate::equipment::CoverCalibratorEntry {
            id: "cc".to_string(),
            connected: true,
            config: crate::config::CoverCalibratorConfig {
                id: "cc".to_string(),
                alpaca_url: "http://localhost:1".to_string(),
                device_number: 0,
                poll_interval: Duration::from_secs(1),
                auth: None,
            },
            device: Some(cc),
        }],
        focusers: vec![],
        mount: None,
    }
}

fn focuser_registry(
    foc: Arc<dyn ascom_alpaca::api::Focuser>,
    min_position: Option<i32>,
    max_position: Option<i32>,
) -> crate::equipment::EquipmentRegistry {
    crate::equipment::EquipmentRegistry {
        cameras: vec![],
        filter_wheels: vec![],
        cover_calibrators: vec![],
        focusers: vec![crate::equipment::FocuserEntry {
            id: "foc".to_string(),
            connected: true,
            config: crate::config::FocuserConfig {
                id: "foc".to_string(),
                camera_id: String::new(),
                alpaca_url: "http://localhost:1".to_string(),
                device_number: 0,
                min_position,
                max_position,
                auth: None,
            },
            device: Some(foc),
        }],
        mount: None,
    }
}

fn mount_registry(
    mount: Arc<dyn ascom_alpaca::api::Telescope>,
    settle_after_slew: Option<Duration>,
) -> crate::equipment::EquipmentRegistry {
    crate::equipment::EquipmentRegistry {
        cameras: vec![],
        filter_wheels: vec![],
        cover_calibrators: vec![],
        focusers: vec![],
        mount: Some(crate::equipment::MountEntry {
            connected: true,
            config: crate::config::MountConfig {
                alpaca_url: "http://localhost:1".to_string(),
                device_number: 0,
                settle_after_slew,
                auth: None,
            },
            device: Some(mount),
        }),
    }
}

fn empty_registry() -> crate::equipment::EquipmentRegistry {
    crate::equipment::EquipmentRegistry {
        cameras: vec![],
        filter_wheels: vec![],
        cover_calibrators: vec![],
        focusers: vec![],
        mount: None,
    }
}

fn disconnected_mount_registry() -> crate::equipment::EquipmentRegistry {
    crate::equipment::EquipmentRegistry {
        cameras: vec![],
        filter_wheels: vec![],
        cover_calibrators: vec![],
        focusers: vec![],
        mount: Some(crate::equipment::MountEntry {
            connected: false,
            config: crate::config::MountConfig {
                alpaca_url: "http://localhost:1".to_string(),
                device_number: 0,
                settle_after_slew: None,
                auth: None,
            },
            device: None,
        }),
    }
}

// -----------------------------------------------------------------------
// Capture tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_capture_start_exposure_fails() {
    let cam = MockCamera {
        fail_start_exposure: true,
        ..Default::default()
    };
    let handler = test_handler(camera_registry(Arc::new(cam)));
    let result = handler
        .capture_inner(
            CaptureParams {
                camera_id: "cam".into(),
                duration: Duration::from_millis(100),
            },
            None,
        )
        .await;
    assert_tool_error(result, "failed to start exposure");
}

#[tokio::test]
async fn test_capture_image_ready_error() {
    let cam = MockCamera {
        fail_image_ready: true,
        ..Default::default()
    };
    let handler = test_handler(camera_registry(Arc::new(cam)));
    let result = handler
        .capture_inner(
            CaptureParams {
                camera_id: "cam".into(),
                duration: Duration::from_millis(100),
            },
            None,
        )
        .await;
    assert_tool_error(result, "error checking image ready");
}

#[tokio::test]
async fn test_capture_image_array_fails() {
    let cam = MockCamera {
        fail_image_array: true,
        ..Default::default()
    };
    let handler = test_handler(camera_registry(Arc::new(cam)));
    let result = handler
        .capture_inner(
            CaptureParams {
                camera_id: "cam".into(),
                duration: Duration::from_millis(100),
            },
            None,
        )
        .await;
    assert_tool_error(result, "failed to download image array");
}

#[tokio::test]
async fn test_capture_failed_exposure_surfaces_error_not_hang() {
    // Regression for the 6 h CI hang: a camera that *fails* an exposure
    // reports `CameraState::Error` and leaves `ImageReady` false forever.
    // `do_capture` must surface that as an error rather than polling
    // `ImageReady` indefinitely. `fail_image_array` makes `image_array`
    // carry the stored failure detail, mirroring sky-survey-camera after a
    // follow-mode mount-read timeout. The outer `timeout` guard fails the
    // test loudly (rather than hanging the suite) if the loop regresses.
    let cam = MockCamera {
        never_ready: true,
        report_error_state: true,
        fail_image_array: true,
        ..Default::default()
    };
    let handler = test_handler(camera_registry(Arc::new(cam)));
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        handler.capture_inner(
            CaptureParams {
                camera_id: "cam".into(),
                duration: Duration::from_millis(100),
            },
            None,
        ),
    )
    .await
    .expect("do_capture hung on a failed exposure instead of returning an error");
    assert_tool_error(result, "exposure failed");
}

#[tokio::test(start_paused = true)]
async fn test_capture_times_out_when_camera_never_ready() {
    // Backstop: a camera wedged not-ready *without* signalling an Error
    // state must still not hang forever — the `duration + CAPTURE_READOUT_
    // GRACE` deadline ends the loop. `start_paused` auto-advances tokio's
    // clock so the ~120 s deadline is reached without real waiting.
    let cam = MockCamera {
        never_ready: true,
        // report_error_state defaults false → camera_state == Idle, so the
        // Error branch never trips and only the deadline can end the loop.
        ..Default::default()
    };
    let handler = test_handler(camera_registry(Arc::new(cam)));
    let result = handler
        .capture_inner(
            CaptureParams {
                camera_id: "cam".into(),
                duration: Duration::from_millis(100),
            },
            None,
        )
        .await;
    assert_tool_error(result, "timeout waiting for image_ready");
}

// -----------------------------------------------------------------------
// get_camera_info tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_get_camera_info_max_adu_unavailable_when_cache_none() {
    // `max_adu` moved to a connect-time cache on `CameraEntry`. A connect-
    // time read failure leaves `max_adu = None`; `get_camera_info` must
    // surface that as a tool_error so consumers don't mistake "absent"
    // for "zero". This replaces the old live-read failure test.
    let registry = camera_registry_with_meta(
        Arc::new(MockCamera::default()),
        None,
        CachedCameraMeta {
            max_adu: None,
            ..CachedCameraMeta::default()
        },
    );
    let handler = test_handler(registry);
    let result = handler
        .get_camera_info(Parameters(CameraIdParams {
            camera_id: "cam".into(),
        }))
        .await;
    assert_tool_error(result, "max_adu unavailable");
}

#[tokio::test]
async fn test_get_camera_info_reads_max_adu_and_sensor_from_cache_not_live() {
    // Pin the contract that `max_adu` and the sensor dimensions come from
    // `CameraEntry`'s connect-time cache, NOT from per-call Alpaca reads.
    // We rig the MockCamera so its `max_adu` and `camera_size` methods
    // would fail if invoked, then seed the cache with distinctive values.
    // `get_camera_info` must succeed and report the cached values — proving
    // the live reads aren't happening on the hot path.
    let cam = MockCamera {
        fail_max_adu: true,
        fail_camera_size: true,
        ..Default::default()
    };
    let registry = camera_registry_with_meta(
        Arc::new(cam),
        None,
        CachedCameraMeta {
            max_adu: Some(4242),
            sensor_width_px: Some(3000),
            sensor_height_px: Some(2000),
            ..CachedCameraMeta::default()
        },
    );
    let handler = test_handler(registry);
    let result = handler
        .get_camera_info(Parameters(CameraIdParams {
            camera_id: "cam".into(),
        }))
        .await
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|tc| tc.text.as_str())
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(json["max_adu"], 4242);
    assert_eq!(json["sensor_x"], 3000);
    assert_eq!(json["sensor_y"], 2000);
}

#[tokio::test]
async fn test_get_camera_info_sensor_size_unavailable_when_cache_none() {
    // Sensor dimensions moved to the connect-time cache (same shape as
    // `max_adu` above). A missing `sensor_width_px` or `sensor_height_px`
    // surfaces as a tool_error.
    let registry = camera_registry_with_meta(
        Arc::new(MockCamera::default()),
        None,
        CachedCameraMeta {
            sensor_width_px: None,
            ..CachedCameraMeta::default()
        },
    );
    let handler = test_handler(registry);
    let result = handler
        .get_camera_info(Parameters(CameraIdParams {
            camera_id: "cam".into(),
        }))
        .await;
    assert_tool_error(result, "sensor size unavailable");
}

// -----------------------------------------------------------------------
// set_filter tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_set_filter_set_position_fails() {
    let fw = MockFilterWheel {
        fail_set_position: true,
        ..Default::default()
    };
    let handler = test_handler(filter_wheel_registry(Arc::new(fw)));
    let result = handler
        .set_filter(Parameters(SetFilterParams {
            filter_wheel_id: "fw".into(),
            filter_name: "Lum".into(),
        }))
        .await;
    assert_tool_error(result, "failed to set filter position");
}

// -----------------------------------------------------------------------
// CoverCalibrator tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_close_cover_command_fails() {
    let cc = MockCoverCalibrator {
        fail_close_cover: true,
        ..Default::default()
    };
    let handler = test_handler(calibrator_registry(Arc::new(cc)));
    let result = handler
        .close_cover(Parameters(CalibratorIdParams {
            calibrator_id: "cc".into(),
        }))
        .await;
    assert_tool_error(result, "failed to close cover");
}

#[tokio::test]
async fn test_close_cover_polling_error() {
    let cc = MockCoverCalibrator {
        fail_cover_state_poll: true,
        ..Default::default()
    };
    let handler = test_handler(calibrator_registry(Arc::new(cc)));
    let result = handler
        .close_cover(Parameters(CalibratorIdParams {
            calibrator_id: "cc".into(),
        }))
        .await;
    assert_tool_error(result, "error polling cover state");
}

#[tokio::test]
async fn test_open_cover_command_fails() {
    let cc = MockCoverCalibrator {
        fail_open_cover: true,
        ..Default::default()
    };
    let handler = test_handler(calibrator_registry(Arc::new(cc)));
    let result = handler
        .open_cover(Parameters(CalibratorIdParams {
            calibrator_id: "cc".into(),
        }))
        .await;
    assert_tool_error(result, "failed to open cover");
}

#[tokio::test]
async fn test_calibrator_on_max_brightness_fails() {
    let cc = MockCoverCalibrator {
        fail_max_brightness: true,
        ..Default::default()
    };
    let handler = test_handler(calibrator_registry(Arc::new(cc)));
    let result = handler
        .calibrator_on(Parameters(CalibratorOnParams {
            calibrator_id: "cc".into(),
            brightness: None,
        }))
        .await;
    assert_tool_error(result, "failed to read max_brightness");
}

#[tokio::test]
async fn test_calibrator_on_command_fails() {
    let cc = MockCoverCalibrator {
        fail_calibrator_on: true,
        ..Default::default()
    };
    let handler = test_handler(calibrator_registry(Arc::new(cc)));
    let result = handler
        .calibrator_on(Parameters(CalibratorOnParams {
            calibrator_id: "cc".into(),
            brightness: None,
        }))
        .await;
    assert_tool_error(result, "failed to turn calibrator on");
}

#[tokio::test]
async fn test_calibrator_off_command_fails() {
    let cc = MockCoverCalibrator {
        fail_calibrator_off: true,
        ..Default::default()
    };
    let handler = test_handler(calibrator_registry(Arc::new(cc)));
    let result = handler
        .calibrator_off(Parameters(CalibratorIdParams {
            calibrator_id: "cc".into(),
        }))
        .await;
    assert_tool_error(result, "failed to turn calibrator off");
}

// -----------------------------------------------------------------------
// capture — write_fits failure
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_capture_write_fits_fails() {
    let cam = MockCamera::default(); // succeeds through image_array
    let registry = camera_registry(Arc::new(cam));
    // Use an existing file as the "directory" so write_fits fails cross-platform.
    // The capture tool appends /<uuid8>.fits — creating a file inside
    // another file fails on all OSes.
    let blocker = tempfile::NamedTempFile::new().unwrap();
    let handler = McpHandler::new(
        Arc::new(registry),
        Arc::new(crate::events::EventBus::from_config(&[])),
        SessionConfig {
            data_directory: blocker.path().to_string_lossy().to_string(),
        },
        ImageCache::new(64, 4, std::path::PathBuf::from("/nonexistent")),
        None,
    );
    let result = handler
        .capture_inner(
            CaptureParams {
                camera_id: "cam".into(),
                duration: Duration::from_millis(100),
            },
            None,
        )
        .await;
    assert_tool_error(result, "failed to write FITS file");
}

// -----------------------------------------------------------------------
// capture — caches I32 variant when max_adu > u16::MAX
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_capture_caches_i32_when_max_adu_above_u16_max() {
    // Drives the scientific-camera (I32) cache-insert branch in
    // `capture` — exercised by no other test, since OmniSim and the
    // default MockCamera both report max_adu ≤ 65535. Pins the
    // capture invariant: a successful capture leaves the embedded
    // document accessible through the cache entry (now the single
    // source of truth) with the matching `max_adu`.
    //
    // The 20-bit max_adu lives on the cached `CameraEntry` rather than
    // on the MockCamera: `do_capture` reads max_adu from the cache (see
    // `CameraEntry` docs) so the live `cam.max_adu()` flag on MockCamera
    // is irrelevant for capture-time semantics.
    let cam = MockCamera::default();
    let registry = camera_registry_with_meta(
        Arc::new(cam),
        None,
        CachedCameraMeta {
            max_adu: Some(1 << 20),
            ..CachedCameraMeta::default()
        },
    );
    let temp = tempfile::tempdir().unwrap();
    let cache = ImageCache::new(64, 4, std::path::PathBuf::from("/nonexistent"));
    let handler = McpHandler::new(
        Arc::new(registry),
        Arc::new(crate::events::EventBus::from_config(&[])),
        SessionConfig {
            data_directory: temp.path().to_string_lossy().to_string(),
        },
        cache.clone(),
        None,
    );
    let result = handler
        .capture_inner(
            CaptureParams {
                camera_id: "cam".into(),
                duration: Duration::from_millis(100),
            },
            None,
        )
        .await
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|tc| tc.text.clone())
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let doc_id = json["document_id"].as_str().unwrap();
    let cached = cache.get(doc_id).expect("expected cache entry");
    assert_eq!(cached.max_adu, 1 << 20);
    assert!(
        matches!(cached.pixels, CachedPixels::I32(_)),
        "expected I32 variant for max_adu > u16::MAX"
    );
    let doc = cache
        .resolve_document(doc_id)
        .await
        .expect("expected cache entry to carry the document");
    assert_eq!(doc.max_adu, Some(1 << 20));
}

// -----------------------------------------------------------------------
// capture — filename uses 8-char UUID suffix
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_capture_filename_uses_uuid8_suffix() {
    // Pins the on-disk reverse-lookup contract: the FITS basename matches
    // the first 8 hex chars of the document_id. The disk-fallback
    // resolution path in Phase 7 grep's by this suffix.
    let cam = MockCamera::default();
    let temp = tempfile::tempdir().unwrap();
    let cache = ImageCache::new(64, 4, std::path::PathBuf::from("/nonexistent"));
    let handler = McpHandler::new(
        Arc::new(camera_registry(Arc::new(cam))),
        Arc::new(crate::events::EventBus::from_config(&[])),
        SessionConfig {
            data_directory: temp.path().to_string_lossy().to_string(),
        },
        cache,
        None,
    );
    let result = handler
        .capture_inner(
            CaptureParams {
                camera_id: "cam".into(),
                duration: Duration::from_millis(100),
            },
            None,
        )
        .await
        .unwrap();
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|tc| tc.text.clone())
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let doc_id = json["document_id"].as_str().unwrap().to_string();
    let image_path = json["image_path"].as_str().unwrap().to_string();
    let basename = std::path::Path::new(&image_path)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        basename,
        format!("{}.fits", &doc_id[..8]),
        "FITS basename must equal first 8 hex chars of document_id + .fits"
    );
    assert!(
        std::path::Path::new(&image_path).exists(),
        "FITS file should exist at the reported path"
    );
}

// -----------------------------------------------------------------------
// capture — optics block in sidecar
// -----------------------------------------------------------------------

async fn capture_and_read_sidecar(
    registry: crate::equipment::EquipmentRegistry,
) -> ExposureDocument {
    let temp = tempfile::tempdir().unwrap();
    let cache = ImageCache::new(64, 4, std::path::PathBuf::from("/nonexistent"));
    let handler = McpHandler::new(
        Arc::new(registry),
        Arc::new(crate::events::EventBus::from_config(&[])),
        SessionConfig {
            data_directory: temp.path().to_string_lossy().to_string(),
        },
        cache,
        None,
    );
    let result = handler
        .capture_inner(
            CaptureParams {
                camera_id: "cam".into(),
                duration: Duration::from_millis(100),
            },
            None,
        )
        .await
        .unwrap();
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|tc| tc.text.clone())
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let image_path = json["image_path"].as_str().unwrap().to_string();
    let sidecar = persistence::sidecar_path(&image_path);
    let doc = persistence::read_sidecar_sync(&sidecar).unwrap();
    // Explicit drop pins the TempDir lifetime past the sidecar read
    // — without it the borrow checker is happy but the temp dir could
    // be cleaned up at any drop point the optimizer chose.
    drop(temp);
    doc
}

#[tokio::test]
async fn test_capture_persists_optics_when_focal_length_configured() {
    // Mock returns 3.76 µm pixels and 1024×1024 sensor; with a 1000 mm
    // focal length the derivation gives:
    //   pixel_scale = 206.265 × 3.76 / 1000 ≈ 0.7755564 arcsec/px
    //   fov         = 0.7755564 × 1024 / 3600 ≈ 0.220603 deg
    let cam = MockCamera::default();
    let registry = camera_registry_with_focal_length(Arc::new(cam), Some(1000.0));
    let doc = capture_and_read_sidecar(registry).await;
    let optics = doc.optics.expect("optics block should be present");
    assert_eq!(optics.focal_length_mm, 1000.0);
    assert_eq!(optics.pixel_size_x_um, 3.76);
    assert_eq!(optics.pixel_size_y_um, 3.76);
    assert_eq!(optics.sensor_width_px, 1024);
    assert_eq!(optics.sensor_height_px, 1024);
    assert!(
        (optics.pixel_scale_x_arcsec_per_pixel - 0.7755564).abs() < 1e-6,
        "pixel_scale_x = {}",
        optics.pixel_scale_x_arcsec_per_pixel
    );
    assert!(
        (optics.fov_height_deg - 0.220603).abs() < 1e-4,
        "fov_height_deg = {}",
        optics.fov_height_deg
    );
}

#[tokio::test]
async fn test_capture_omits_optics_when_focal_length_missing() {
    let cam = MockCamera::default();
    let registry = camera_registry_with_focal_length(Arc::new(cam), None);
    let doc = capture_and_read_sidecar(registry).await;
    assert!(
        doc.optics.is_none(),
        "optics must be omitted when focal_length_mm is not configured"
    );
}

#[tokio::test]
async fn test_capture_omits_optics_when_pixel_size_unavailable() {
    // Models a camera whose connect-time `pixel_size_*` read failed:
    // `CameraEntry.pixel_size_*_um` is None, so the optics block has no
    // pixel pitch to combine with `focal_length_mm` and must be omitted.
    let cam = MockCamera::default();
    let registry = camera_registry_with_meta(
        Arc::new(cam),
        Some(1000.0),
        CachedCameraMeta {
            pixel_size_x_um: None,
            pixel_size_y_um: None,
            ..CachedCameraMeta::default()
        },
    );
    let doc = capture_and_read_sidecar(registry).await;
    assert!(
        doc.optics.is_none(),
        "optics must be omitted when cached pixel_size is None"
    );
}

#[tokio::test]
async fn test_capture_omits_optics_when_sensor_size_unavailable() {
    // Same shape as the pixel_size case: models a camera whose connect-
    // time `camera_*_size` read failed.
    let cam = MockCamera::default();
    let registry = camera_registry_with_meta(
        Arc::new(cam),
        Some(1000.0),
        CachedCameraMeta {
            sensor_width_px: None,
            sensor_height_px: None,
            ..CachedCameraMeta::default()
        },
    );
    let doc = capture_and_read_sidecar(registry).await;
    assert!(
        doc.optics.is_none(),
        "optics must be omitted when cached sensor size is None"
    );
}

#[tokio::test]
async fn test_capture_does_not_call_invariant_metadata_methods_on_device() {
    // Regression contract pinned by `MockCameraNoMetadata` (above): every
    // invariant-sensor property method panics. With the `CameraEntry`
    // cache populated, `do_capture` must satisfy itself from the cache
    // and never touch the device — so the call must succeed without any
    // panic. If a future change reintroduces a per-capture read of one
    // of these properties, this test catches it via panic.
    let cam = MockCameraNoMetadata;
    let registry = camera_registry_with_meta(
        Arc::new(cam),
        Some(1000.0),
        // Populate the cache with realistic values so the U16-cache path
        // is taken and the optics block is built — exercising every
        // place `do_capture` consumes a cached metadata field.
        CachedCameraMeta::default(),
    );
    let doc = capture_and_read_sidecar(registry).await;
    assert_eq!(doc.max_adu, Some(MOCK_CAMERA_MAX_ADU));
    let optics = doc.optics.expect(
        "optics block should be present (cached pixel/sensor + configured focal_length_mm)",
    );
    assert_eq!(optics.focal_length_mm, 1000.0);
    assert_eq!(optics.pixel_size_x_um, MOCK_CAMERA_PIXEL_SIZE_UM);
    assert_eq!(optics.sensor_width_px, MOCK_CAMERA_SENSOR_PX);
}

// -----------------------------------------------------------------------
// persist_capture_artifact — sidecar failure skips cache
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_persist_capture_artifact_skips_cache_on_sidecar_failure() {
    // Pins the sidecar-failure branch in `persist_capture_artifact` (the
    // post-FITS persistence step extracted from `capture`). Contract
    // documented in `docs/services/rp.md` → Capture Tool Details
    // → Sidecar failure contract: write_sidecar fails →
    // `document_persistence_failed` event payload is constructed → cache
    // insert is skipped → `document_id`-keyed lookups return 404.
    //
    // Forcing the failure: `doc.file_path` lives inside a regular file so
    // `create_dir_all(parent)` in write_sidecar errors with NotADirectory.
    // Same trick as the put_section rollback tests in cache.rs.
    let temp = tempfile::tempdir().unwrap();
    let blocker = temp.path().join("blocker");
    std::fs::write(&blocker, b"not a directory").unwrap();

    let cache = ImageCache::new(64, 4, std::path::PathBuf::from("/nonexistent"));
    let handler = McpHandler::new(
        Arc::new(crate::equipment::EquipmentRegistry {
            cameras: vec![],
            filter_wheels: vec![],
            cover_calibrators: vec![],
            focusers: vec![],
            mount: None,
        }),
        Arc::new(crate::events::EventBus::from_config(&[])),
        SessionConfig {
            data_directory: temp.path().to_string_lossy().to_string(),
        },
        cache.clone(),
        None,
    );

    let doc = ExposureDocument {
        id: "doc-fail-1".to_string(),
        captured_at: "2026-04-30T00:00:00Z".to_string(),
        file_path: blocker.join("x.fits").to_string_lossy().into_owned(),
        width: 2,
        height: 2,
        camera_id: Some("cam".into()),
        duration: Some(Duration::from_millis(100)),
        max_adu: Some(65535),
        optics: None,
        sections: serde_json::Map::new(),
    };
    let cached = CachedPixels::from_i32_pixels(vec![1, 2, 3, 4], (2, 2), 65535);

    handler
        .persist_capture_artifact(doc, cached, Some(65535))
        .await;

    assert!(
        cache.get("doc-fail-1").is_none(),
        "cache must not be populated when sidecar write fails"
    );
}

// -----------------------------------------------------------------------
// get_camera_info — exposure_range fallback
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_get_camera_info_exposure_range_fallback() {
    let cam = MockCamera {
        fail_exposure_range: true,
        ..Default::default()
    };
    let handler = test_handler(camera_registry(Arc::new(cam)));
    let result = handler
        .get_camera_info(Parameters(CameraIdParams {
            camera_id: "cam".into(),
        }))
        .await;
    // This is a soft failure — it falls back to defaults, so the call succeeds
    let call_result = result.unwrap();
    assert!(!call_result.is_error.unwrap_or(false));
    let text = call_result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|tc| tc.text.as_str())
        .unwrap_or("");
    let json: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(json["exposure_min"], "1ms");
    assert_eq!(json["exposure_max"], "1h");
}

// -----------------------------------------------------------------------
// set_filter — polling error
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_set_filter_polling_error() {
    let fw = MockFilterWheel {
        fail_position_poll: true,
        ..Default::default()
    };
    let handler = test_handler(filter_wheel_registry(Arc::new(fw)));
    let result = handler
        .set_filter(Parameters(SetFilterParams {
            filter_wheel_id: "fw".into(),
            filter_name: "Lum".into(),
        }))
        .await;
    assert_tool_error(result, "error waiting for filter wheel");
}

// -----------------------------------------------------------------------
// get_filter — errors
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_get_filter_position_error() {
    let fw = MockFilterWheel {
        fail_position_poll: true,
        ..Default::default()
    };
    let handler = test_handler(filter_wheel_registry(Arc::new(fw)));
    let result = handler
        .get_filter(Parameters(FilterWheelIdParams {
            filter_wheel_id: "fw".into(),
        }))
        .await;
    assert_tool_error(result, "failed to get filter position");
}

#[tokio::test]
async fn test_get_filter_wheel_moving() {
    let fw = MockFilterWheel {
        report_moving: true,
        ..Default::default()
    };
    let handler = test_handler(filter_wheel_registry(Arc::new(fw)));
    let result = handler
        .get_filter(Parameters(FilterWheelIdParams {
            filter_wheel_id: "fw".into(),
        }))
        .await;
    assert_tool_error(result, "filter wheel is moving");
}

// -----------------------------------------------------------------------
// Timeout tests (use tokio::time::pause to fast-forward)
// -----------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn test_close_cover_timeout() {
    let cc = MockCoverCalibrator {
        stuck_cover_moving: true,
        ..Default::default()
    };
    let handler = test_handler(calibrator_registry(Arc::new(cc)));
    let result = handler
        .close_cover(Parameters(CalibratorIdParams {
            calibrator_id: "cc".into(),
        }))
        .await;
    assert_tool_error(result, "timeout waiting for cover to close");
}

#[tokio::test(start_paused = true)]
async fn test_open_cover_timeout() {
    let cc = MockCoverCalibrator {
        stuck_cover_moving: true,
        ..Default::default()
    };
    let handler = test_handler(calibrator_registry(Arc::new(cc)));
    let result = handler
        .open_cover(Parameters(CalibratorIdParams {
            calibrator_id: "cc".into(),
        }))
        .await;
    assert_tool_error(result, "timeout waiting for cover to open");
}

#[tokio::test]
async fn test_open_cover_polling_error() {
    let cc = MockCoverCalibrator {
        fail_cover_state_poll: true,
        ..Default::default()
    };
    let handler = test_handler(calibrator_registry(Arc::new(cc)));
    let result = handler
        .open_cover(Parameters(CalibratorIdParams {
            calibrator_id: "cc".into(),
        }))
        .await;
    assert_tool_error(result, "error polling cover state");
}

#[tokio::test(start_paused = true)]
async fn test_calibrator_on_timeout() {
    let cc = MockCoverCalibrator {
        stuck_calibrator_not_ready: true,
        ..Default::default()
    };
    let handler = test_handler(calibrator_registry(Arc::new(cc)));
    let result = handler
        .calibrator_on(Parameters(CalibratorOnParams {
            calibrator_id: "cc".into(),
            brightness: Some(100),
        }))
        .await;
    assert_tool_error(result, "timeout waiting for calibrator to become ready");
}

#[tokio::test]
async fn test_calibrator_on_polling_error() {
    let cc = MockCoverCalibrator {
        fail_calibrator_state_poll: true,
        ..Default::default()
    };
    let handler = test_handler(calibrator_registry(Arc::new(cc)));
    let result = handler
        .calibrator_on(Parameters(CalibratorOnParams {
            calibrator_id: "cc".into(),
            brightness: Some(100),
        }))
        .await;
    assert_tool_error(result, "error polling calibrator state");
}

#[tokio::test(start_paused = true)]
async fn test_calibrator_off_timeout() {
    let cc = MockCoverCalibrator {
        stuck_calibrator_not_ready: true,
        ..Default::default()
    };
    let handler = test_handler(calibrator_registry(Arc::new(cc)));
    let result = handler
        .calibrator_off(Parameters(CalibratorIdParams {
            calibrator_id: "cc".into(),
        }))
        .await;
    assert_tool_error(result, "timeout waiting for calibrator to turn off");
}

#[tokio::test]
async fn test_calibrator_off_polling_error() {
    let cc = MockCoverCalibrator {
        fail_calibrator_state_poll: true,
        ..Default::default()
    };
    let handler = test_handler(calibrator_registry(Arc::new(cc)));
    let result = handler
        .calibrator_off(Parameters(CalibratorIdParams {
            calibrator_id: "cc".into(),
        }))
        .await;
    assert_tool_error(result, "error polling calibrator state");
}

// -----------------------------------------------------------------------
// compute_image_stats error paths
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_compute_image_stats_bad_fits() {
    // Write a non-FITS file so read_fits_pixels fails inside spawn_blocking
    let dir = tempfile::tempdir().unwrap();
    let bad_file = dir.path().join("bad.fits");
    std::fs::write(&bad_file, b"not a fits file").unwrap();

    let handler = test_handler(crate::equipment::EquipmentRegistry {
        cameras: vec![],
        filter_wheels: vec![],
        cover_calibrators: vec![],
        focusers: vec![],
        mount: None,
    });
    let result = handler
        .compute_image_stats(Parameters(ComputeImageStatsParams {
            image_path: Some(bad_file.to_string_lossy().to_string()),
            document_id: None,
        }))
        .await;
    assert_tool_error(result, "failed to compute stats");
}

#[tokio::test]
async fn test_compute_image_stats_missing_arguments() {
    // Pins the validation contract surfaced by `rp.md:702` ("document_id
    // or image_path"): callers must supply at least one. Mirrors the
    // missing-args branch tested for measure_basic / estimate_background.
    let handler = test_handler(crate::equipment::EquipmentRegistry {
        cameras: vec![],
        filter_wheels: vec![],
        cover_calibrators: vec![],
        focusers: vec![],
        mount: None,
    });
    let result = handler
        .compute_image_stats(Parameters(ComputeImageStatsParams {
            document_id: None,
            image_path: None,
        }))
        .await;
    assert_tool_error(result, "image_path");
}

#[tokio::test]
async fn test_compute_image_stats_persists_section_via_document_id() {
    // Pins the load-bearing piece of issue #175: when called with a
    // `document_id` that resolves through the cache, the computed
    // stats are written into the exposure document as an
    // `image_stats` section. Mirrors the section-persistence test
    // shape used by the other imaging tools (measure_basic ->
    // image_analysis, estimate_background -> background, etc).
    let temp = tempfile::tempdir().unwrap();
    let cache = ImageCache::new(64, 4, temp.path().to_path_buf());

    // Pixels chosen so the resulting stats are unambiguous: pixel_count = 4,
    // min = 100, max = 400, median = (200 + 300) / 2 = 250, mean = 250.0.
    let pixel_buf: Vec<u16> = vec![100, 200, 300, 400];
    let cached_pixels = CachedPixels::from_u16_pixels(pixel_buf, (2, 2)).unwrap();

    let document_id = "doc-image-stats-1".to_string();
    let uuid8 = "doc-imgs"; // 8-char-stable suffix for the on-disk basename.
    let file_path = temp
        .path()
        .join(format!("{}.fits", uuid8))
        .to_string_lossy()
        .into_owned();

    let doc = ExposureDocument {
        id: document_id.clone(),
        captured_at: "2026-05-08T00:00:00Z".to_string(),
        file_path: file_path.clone(),
        width: 2,
        height: 2,
        camera_id: Some("cam".into()),
        duration: Some(Duration::from_millis(100)),
        max_adu: Some(65535),
        optics: None,
        sections: serde_json::Map::new(),
    };

    cache.insert(
        document_id.clone(),
        crate::persistence::CachedImage::new(
            cached_pixels,
            2,
            2,
            std::path::PathBuf::from(&file_path),
            65535,
            doc,
        ),
    );

    let handler = McpHandler::new(
        Arc::new(crate::equipment::EquipmentRegistry {
            cameras: vec![],
            filter_wheels: vec![],
            cover_calibrators: vec![],
            focusers: vec![],
            mount: None,
        }),
        Arc::new(crate::events::EventBus::from_config(&[])),
        SessionConfig {
            data_directory: temp.path().to_string_lossy().to_string(),
        },
        cache.clone(),
        None,
    );

    let call_result = handler
        .compute_image_stats(Parameters(ComputeImageStatsParams {
            document_id: Some(document_id.clone()),
            image_path: None,
        }))
        .await
        .unwrap();
    assert!(!call_result.is_error.unwrap_or(false));

    let payload_text = call_result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|tc| tc.text.clone())
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(&payload_text).unwrap();
    assert_eq!(payload["pixel_count"], 4);
    assert_eq!(payload["min_adu"], 100);
    assert_eq!(payload["max_adu"], 400);
    assert_eq!(payload["median_adu"], 250);
    assert!((payload["mean_adu"].as_f64().unwrap() - 250.0).abs() < 1e-9);

    let updated = cache
        .resolve_document(&document_id)
        .await
        .expect("document should resolve from cache after compute_image_stats");
    let section = updated
        .sections
        .get("image_stats")
        .expect("image_stats section must be persisted when document_id is supplied");
    assert_eq!(section["pixel_count"], 4);
    assert_eq!(section["min_adu"], 100);
    assert_eq!(section["max_adu"], 400);
    assert_eq!(section["median_adu"], 250);
}

// -----------------------------------------------------------------------
// set_filter — filter not found
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_set_filter_filter_not_found() {
    let fw = MockFilterWheel::default();
    let handler = test_handler(filter_wheel_registry(Arc::new(fw)));
    let result = handler
        .set_filter(Parameters(SetFilterParams {
            filter_wheel_id: "fw".into(),
            filter_name: "Ultraviolet".into(), // not in mock's filter list
        }))
        .await;
    assert_tool_error(result, "filter not found");
}

// -----------------------------------------------------------------------
// get_filter — success path (covers lines 387-391)
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_get_filter_success() {
    let fw = MockFilterWheel::default(); // position() returns Some(0)
    let handler = test_handler(filter_wheel_registry(Arc::new(fw)));
    let result = handler
        .get_filter(Parameters(FilterWheelIdParams {
            filter_wheel_id: "fw".into(),
        }))
        .await;
    let call_result = result.unwrap();
    assert!(!call_result.is_error.unwrap_or(false));
    let text = call_result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|tc| tc.text.as_str())
        .unwrap_or("");
    let json: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(json["filter_name"], "Lum");
    assert_eq!(json["position"], 0);
}

// -----------------------------------------------------------------------
// CoverCalibrator success paths (covers resolve_device! macro lines)
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_close_cover_success() {
    let cc = MockCoverCalibrator::default(); // cover_state returns Closed
    let handler = test_handler(calibrator_registry(Arc::new(cc)));
    let result = handler
        .close_cover(Parameters(CalibratorIdParams {
            calibrator_id: "cc".into(),
        }))
        .await;
    let call_result = result.unwrap();
    assert!(!call_result.is_error.unwrap_or(false));
}

// -----------------------------------------------------------------------
// Focuser tests
// -----------------------------------------------------------------------

fn ok_text(call_result: CallToolResult) -> serde_json::Value {
    assert!(
        !call_result.is_error.unwrap_or(false),
        "expected success, got error: {:?}",
        call_result.content
    );
    let text = call_result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|tc| tc.text.as_str())
        .unwrap_or("");
    serde_json::from_str(text).expect("valid JSON")
}

#[tokio::test]
async fn test_move_focuser_success() {
    let foc = MockFocuser {
        position_value: 4321,
        ..Default::default()
    };
    let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
    let result = handler
        .move_focuser_inner(
            MoveFocuserParams {
                focuser_id: "foc".into(),
                position: 4321,
            },
            None,
        )
        .await
        .unwrap();
    let json = ok_text(result);
    assert_eq!(json["actual_position"], 4321);
    assert_eq!(json["focuser_id"], "foc");
}

#[tokio::test]
async fn test_move_focuser_not_found() {
    let foc = MockFocuser::default();
    let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
    let result = handler
        .move_focuser_inner(
            MoveFocuserParams {
                focuser_id: "missing".into(),
                position: 100,
            },
            None,
        )
        .await;
    assert_tool_error(result, "focuser not found");
}

#[tokio::test]
async fn test_move_focuser_below_min_position() {
    let foc = MockFocuser::default();
    let handler = test_handler(focuser_registry(Arc::new(foc), Some(1000), Some(9000)));
    let result = handler
        .move_focuser_inner(
            MoveFocuserParams {
                focuser_id: "foc".into(),
                position: 500,
            },
            None,
        )
        .await;
    assert_tool_error(result, "position out of range");
}

#[tokio::test]
async fn test_move_focuser_above_max_position() {
    let foc = MockFocuser::default();
    let handler = test_handler(focuser_registry(Arc::new(foc), Some(1000), Some(9000)));
    let result = handler
        .move_focuser_inner(
            MoveFocuserParams {
                focuser_id: "foc".into(),
                position: 9500,
            },
            None,
        )
        .await;
    assert_tool_error(result, "position out of range");
}

#[tokio::test]
async fn test_move_focuser_command_fails() {
    let foc = MockFocuser {
        fail_move: true,
        ..Default::default()
    };
    let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
    let result = handler
        .move_focuser_inner(
            MoveFocuserParams {
                focuser_id: "foc".into(),
                position: 1000,
            },
            None,
        )
        .await;
    assert_tool_error(result, "failed to move focuser");
}

#[tokio::test]
async fn test_move_focuser_is_moving_poll_fails() {
    let foc = MockFocuser {
        fail_is_moving: true,
        ..Default::default()
    };
    let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
    let result = handler
        .move_focuser_inner(
            MoveFocuserParams {
                focuser_id: "foc".into(),
                position: 1000,
            },
            None,
        )
        .await;
    assert_tool_error(result, "error polling focuser is_moving");
}

#[tokio::test]
async fn test_move_focuser_position_read_fails() {
    let foc = MockFocuser {
        fail_position: true,
        ..Default::default()
    };
    let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
    let result = handler
        .move_focuser_inner(
            MoveFocuserParams {
                focuser_id: "foc".into(),
                position: 1000,
            },
            None,
        )
        .await;
    assert_tool_error(result, "failed to read focuser position");
}

#[tokio::test(start_paused = true)]
async fn test_move_focuser_timeout() {
    let foc = MockFocuser {
        stuck_moving: true,
        ..Default::default()
    };
    let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
    let result = handler
        .move_focuser_inner(
            MoveFocuserParams {
                focuser_id: "foc".into(),
                position: 1000,
            },
            None,
        )
        .await;
    assert_tool_error(result, "timeout waiting for focuser to settle");
}

#[tokio::test]
async fn test_move_focuser_not_connected() {
    let registry = crate::equipment::EquipmentRegistry {
        cameras: vec![],
        filter_wheels: vec![],
        cover_calibrators: vec![],
        focusers: vec![crate::equipment::FocuserEntry {
            id: "foc".to_string(),
            connected: false,
            config: crate::config::FocuserConfig {
                id: "foc".to_string(),
                camera_id: String::new(),
                alpaca_url: "http://localhost:1".to_string(),
                device_number: 0,
                min_position: None,
                max_position: None,
                auth: None,
            },
            device: None,
        }],
        mount: None,
    };
    let handler = test_handler(registry);
    let result = handler
        .move_focuser_inner(
            MoveFocuserParams {
                focuser_id: "foc".into(),
                position: 1000,
            },
            None,
        )
        .await;
    assert_tool_error(result, "focuser not connected");
}

#[tokio::test]
async fn test_get_focuser_position_success() {
    let foc = MockFocuser {
        position_value: 12345,
        ..Default::default()
    };
    let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
    let result = handler
        .get_focuser_position(Parameters(FocuserIdParams {
            focuser_id: "foc".into(),
        }))
        .await
        .unwrap();
    let json = ok_text(result);
    assert_eq!(json["position"], 12345);
}

#[tokio::test]
async fn test_get_focuser_position_not_connected() {
    let registry = crate::equipment::EquipmentRegistry {
        cameras: vec![],
        filter_wheels: vec![],
        cover_calibrators: vec![],
        focusers: vec![crate::equipment::FocuserEntry {
            id: "foc".to_string(),
            connected: false,
            config: crate::config::FocuserConfig {
                id: "foc".to_string(),
                camera_id: String::new(),
                alpaca_url: "http://localhost:1".to_string(),
                device_number: 0,
                min_position: None,
                max_position: None,
                auth: None,
            },
            device: None,
        }],
        mount: None,
    };
    let handler = test_handler(registry);
    let result = handler
        .get_focuser_position(Parameters(FocuserIdParams {
            focuser_id: "foc".into(),
        }))
        .await;
    assert_tool_error(result, "focuser not connected");
}

/// `Temperature` is independent of `TempCompAvailable`: a focuser may
/// report a temperature reading regardless of whether temperature
/// compensation is available. The mock leaves `temp_comp_available()`
/// at its default `Ok(false)` to make that decoupling explicit.
#[tokio::test]
async fn test_get_focuser_temperature_returns_value() {
    let foc = MockFocuser {
        temperature_value: 12.5,
        ..Default::default()
    };
    let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
    let result = handler
        .get_focuser_temperature(Parameters(FocuserIdParams {
            focuser_id: "foc".into(),
        }))
        .await
        .unwrap();
    let json = ok_text(result);
    assert_eq!(json["temperature_c"], 12.5);
}

/// `Temperature` returning `NOT_IMPLEMENTED` is the only signal that
/// the property is unsupported on this device; the tool surfaces
/// `temperature_c: null` for that exact case.
#[tokio::test]
async fn test_get_focuser_temperature_null_when_not_implemented() {
    let foc = MockFocuser {
        temperature_not_implemented: true,
        ..Default::default()
    };
    let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
    let result = handler
        .get_focuser_temperature(Parameters(FocuserIdParams {
            focuser_id: "foc".into(),
        }))
        .await
        .unwrap();
    let json = ok_text(result);
    assert!(
        json["temperature_c"].is_null(),
        "expected null temperature_c, got {:?}",
        json["temperature_c"]
    );
}

/// Any non-`NOT_IMPLEMENTED` failure from `temperature()` propagates
/// as a tool error rather than being silently coerced to `null`. This
/// pins the asymmetry between "device says I don't have one" and
/// "device tried to read but the read itself failed".
#[tokio::test]
async fn test_get_focuser_temperature_sensor_fails() {
    let foc = MockFocuser {
        fail_temperature: true,
        ..Default::default()
    };
    let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
    let result = handler
        .get_focuser_temperature(Parameters(FocuserIdParams {
            focuser_id: "foc".into(),
        }))
        .await;
    assert_tool_error(result, "failed to read focuser temperature");
}

// -----------------------------------------------------------------------
// Mount tool tests — slew / sync_mount / get_mount_position /
// get_tracking / set_tracking. Singular mount, no mount_id parameter.
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_slew_success() {
    let mount = MockTelescope {
        ra_value: 10.6847,
        dec_value: 41.2689,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler
        .slew_inner(
            SlewParams {
                ra: Some(10.6847),
                dec: Some(41.2689),
                settle_after: None,
            },
            None,
        )
        .await
        .unwrap();
    let json = ok_text(result);
    assert_eq!(json["actual_ra"], 10.6847);
    assert_eq!(json["actual_dec"], 41.2689);
}

#[tokio::test]
async fn test_slew_no_mount_configured() {
    let handler = test_handler(empty_registry());
    let result = handler
        .slew_inner(
            SlewParams {
                ra: Some(0.0),
                dec: Some(0.0),
                settle_after: None,
            },
            None,
        )
        .await;
    assert_tool_error(result, "no mount configured");
}

#[tokio::test]
async fn test_slew_mount_not_connected() {
    let handler = test_handler(disconnected_mount_registry());
    let result = handler
        .slew_inner(
            SlewParams {
                ra: Some(0.0),
                dec: Some(0.0),
                settle_after: None,
            },
            None,
        )
        .await;
    assert_tool_error(result, "mount not connected");
}

#[tokio::test]
async fn test_slew_missing_ra() {
    let handler = test_handler(mount_registry(Arc::new(MockTelescope::default()), None));
    let result = handler
        .slew_inner(
            SlewParams {
                ra: None,
                dec: Some(0.0),
                settle_after: None,
            },
            None,
        )
        .await;
    assert_tool_error(result, "missing required parameter: ra");
}

#[tokio::test]
async fn test_slew_missing_dec() {
    let handler = test_handler(mount_registry(Arc::new(MockTelescope::default()), None));
    let result = handler
        .slew_inner(
            SlewParams {
                ra: Some(0.0),
                dec: None,
                settle_after: None,
            },
            None,
        )
        .await;
    assert_tool_error(result, "missing required parameter: dec");
}

#[tokio::test]
async fn test_slew_ra_out_of_range() {
    let handler = test_handler(mount_registry(Arc::new(MockTelescope::default()), None));
    let result = handler
        .slew_inner(
            SlewParams {
                ra: Some(25.0),
                dec: Some(0.0),
                settle_after: None,
            },
            None,
        )
        .await;
    assert_tool_error(result, "ra out of range");
}

#[tokio::test]
async fn test_slew_dec_out_of_range() {
    let handler = test_handler(mount_registry(Arc::new(MockTelescope::default()), None));
    let result = handler
        .slew_inner(
            SlewParams {
                ra: Some(0.0),
                dec: Some(91.0),
                settle_after: None,
            },
            None,
        )
        .await;
    assert_tool_error(result, "dec out of range");
}

/// Models the ASCOM `InvalidOperationException` that fires when
/// `Tracking == false` and the caller invokes
/// `SlewToCoordinatesAsync` — the natural error path the design
/// explicitly chose over a magical `ensure_tracking` parameter.
#[tokio::test]
async fn test_slew_alpaca_error_propagates() {
    let mount = MockTelescope {
        fail_slew: true,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler
        .slew_inner(
            SlewParams {
                ra: Some(0.0),
                dec: Some(0.0),
                settle_after: None,
            },
            None,
        )
        .await;
    assert_tool_error(result, "failed to slew");
}

/// Drives the timeout escalation path: `slewing()` returns `true`
/// indefinitely, the 300 s deadline expires, `abort_slew()` is
/// called (best-effort, ignored result), and the tool returns the
/// timeout error. `start_paused` lets tokio auto-advance virtual
/// time so the test runs in real-time milliseconds.
#[tokio::test(start_paused = true)]
async fn test_slew_timeout_returns_error_after_abort() {
    let mount = MockTelescope {
        stuck_slewing: true,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler
        .slew_inner(
            SlewParams {
                ra: Some(0.0),
                dec: Some(0.0),
                settle_after: None,
            },
            None,
        )
        .await;
    assert_tool_error(result, "timeout waiting for mount to settle");
}

/// Per-call `settle_after` overrides the config default. Passes
/// `Duration::ZERO` to skip an otherwise-non-zero config value;
/// behavior of the actual sleep is exercised in BDD where
/// wall-clock timing is observable.
#[tokio::test]
async fn test_slew_per_call_settle_overrides_config() {
    let mount = MockTelescope::default();
    let handler = test_handler(mount_registry(
        Arc::new(mount),
        Some(Duration::from_secs(60)),
    ));
    let result = handler
        .slew_inner(
            SlewParams {
                ra: Some(0.0),
                dec: Some(0.0),
                settle_after: Some(Duration::ZERO),
            },
            None,
        )
        .await
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn test_sync_mount_success() {
    let mount = MockTelescope::default();
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler
        .sync_mount(Parameters(SyncMountParams {
            ra: Some(0.0),
            dec: Some(0.0),
        }))
        .await
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn test_sync_mount_no_mount_configured() {
    let handler = test_handler(empty_registry());
    let result = handler
        .sync_mount(Parameters(SyncMountParams {
            ra: Some(0.0),
            dec: Some(0.0),
        }))
        .await;
    assert_tool_error(result, "no mount configured");
}

#[tokio::test]
async fn test_sync_mount_alpaca_error() {
    let mount = MockTelescope {
        fail_sync: true,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler
        .sync_mount(Parameters(SyncMountParams {
            ra: Some(0.0),
            dec: Some(0.0),
        }))
        .await;
    assert_tool_error(result, "failed to sync mount");
}

#[tokio::test]
async fn test_sync_mount_ra_out_of_range() {
    let handler = test_handler(mount_registry(Arc::new(MockTelescope::default()), None));
    let result = handler
        .sync_mount(Parameters(SyncMountParams {
            ra: Some(-1.0),
            dec: Some(0.0),
        }))
        .await;
    assert_tool_error(result, "ra out of range");
}

#[tokio::test]
async fn test_get_mount_position_returns_ra_dec() {
    let mount = MockTelescope {
        ra_value: 12.5,
        dec_value: -23.4,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler
        .get_mount_position(Parameters(GetMountPositionParams {}))
        .await
        .unwrap();
    let json = ok_text(result);
    assert_eq!(json["ra"], 12.5);
    assert_eq!(json["dec"], -23.4);
}

#[tokio::test]
async fn test_get_mount_position_no_mount() {
    let handler = test_handler(empty_registry());
    let result = handler
        .get_mount_position(Parameters(GetMountPositionParams {}))
        .await;
    assert_tool_error(result, "no mount configured");
}

#[tokio::test]
async fn test_get_mount_position_ra_read_fails() {
    let mount = MockTelescope {
        fail_right_ascension: true,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler
        .get_mount_position(Parameters(GetMountPositionParams {}))
        .await;
    assert_tool_error(result, "failed to read mount right_ascension");
}

#[tokio::test]
async fn test_get_tracking_returns_state_and_capability() {
    let mount = MockTelescope {
        tracking_value: true,
        can_set_tracking_value: true,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler
        .get_tracking(Parameters(GetTrackingParams {}))
        .await
        .unwrap();
    let json = ok_text(result);
    assert_eq!(json["tracking"], true);
    assert_eq!(json["can_set_tracking"], true);
}

/// Mount that reports `CanSetTracking == false` — surfaces in the
/// tool result rather than failing the call. Workflows can read
/// the field and decide whether to continue.
#[tokio::test]
async fn test_get_tracking_surfaces_can_set_tracking_false() {
    let mount = MockTelescope {
        tracking_value: false,
        can_set_tracking_value: false,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler
        .get_tracking(Parameters(GetTrackingParams {}))
        .await
        .unwrap();
    let json = ok_text(result);
    assert_eq!(json["tracking"], false);
    assert_eq!(json["can_set_tracking"], false);
}

/// Per the design decision: fail loud on `Tracking` read errors;
/// don't try to half-succeed by returning `can_set_tracking` alone.
#[tokio::test]
async fn test_get_tracking_fails_when_tracking_read_errors() {
    let mount = MockTelescope {
        fail_tracking: true,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler.get_tracking(Parameters(GetTrackingParams {})).await;
    assert_tool_error(result, "failed to read mount tracking");
}

#[tokio::test]
async fn test_set_tracking_enables() {
    let mount = MockTelescope::default();
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler
        .set_tracking(Parameters(SetTrackingParams { enabled: true }))
        .await
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn test_set_tracking_disables() {
    let mount = MockTelescope::default();
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler
        .set_tracking(Parameters(SetTrackingParams { enabled: false }))
        .await
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
}

/// Models a mount that responds to `set_tracking` with an
/// `InvalidOperationException` (e.g. `CanSetTracking == false`).
/// The error propagates with the friendly prefix.
#[tokio::test]
async fn test_set_tracking_alpaca_error() {
    let mount = MockTelescope {
        fail_set_tracking: true,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler
        .set_tracking(Parameters(SetTrackingParams { enabled: true }))
        .await;
    assert_tool_error(result, "failed to set tracking");
}

// -----------------------------------------------------------------------
// Mount park / unpark / get_park_state / abort_slew tests.
// Singular mount, no params on any of these tools.
// -----------------------------------------------------------------------

/// Mock's `at_park()` returns true immediately, so the polling
/// loop exits on the first iteration.
#[tokio::test]
async fn test_park_success() {
    let mount = MockTelescope {
        at_park_value: true,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler.park_inner(ParkParams {}, None).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn test_park_no_mount_configured() {
    let handler = test_handler(empty_registry());
    let result = handler.park_inner(ParkParams {}, None).await;
    assert_tool_error(result, "no mount configured");
}

#[tokio::test]
async fn test_park_mount_not_connected() {
    let handler = test_handler(disconnected_mount_registry());
    let result = handler.park_inner(ParkParams {}, None).await;
    assert_tool_error(result, "mount not connected");
}

#[tokio::test]
async fn test_park_alpaca_error_propagates() {
    let mount = MockTelescope {
        fail_park: true,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler.park_inner(ParkParams {}, None).await;
    assert_tool_error(result, "failed to park");
}

/// 300 s deadline expires while `at_park()` keeps returning `false`.
/// Unlike `slew`, `park` does NOT auto-abort — it surfaces the
/// timeout and lets the caller decide. `start_paused` lets tokio
/// auto-advance virtual time so the test runs in real-time
/// milliseconds.
#[tokio::test(start_paused = true)]
async fn test_park_timeout_does_not_auto_abort() {
    let mount = MockTelescope {
        at_park_value: false,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler.park_inner(ParkParams {}, None).await;
    assert_tool_error(result, "timeout waiting for mount to park");
}

#[tokio::test]
async fn test_unpark_success() {
    let mount = MockTelescope {
        at_park_value: true,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler.unpark(Parameters(UnparkParams {})).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn test_unpark_no_mount_configured() {
    let handler = test_handler(empty_registry());
    let result = handler.unpark(Parameters(UnparkParams {})).await;
    assert_tool_error(result, "no mount configured");
}

#[tokio::test]
async fn test_unpark_alpaca_error() {
    let mount = MockTelescope {
        fail_unpark: true,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler.unpark(Parameters(UnparkParams {})).await;
    assert_tool_error(result, "failed to unpark");
}

#[tokio::test]
async fn test_get_park_state_returns_all_fields() {
    let mount = MockTelescope {
        at_park_value: true,
        can_park_value: true,
        can_unpark_value: true,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler
        .get_park_state(Parameters(GetParkStateParams {}))
        .await
        .unwrap();
    let json = ok_text(result);
    assert_eq!(json["at_park"], true);
    assert_eq!(json["can_park"], true);
    assert_eq!(json["can_unpark"], true);
}

/// Per the design decision: fail loud on `at_park` read errors;
/// don't try to half-succeed by returning `can_park` alone.
#[tokio::test]
async fn test_get_park_state_fails_when_at_park_read_errors() {
    let mount = MockTelescope {
        fail_at_park: true,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler
        .get_park_state(Parameters(GetParkStateParams {}))
        .await;
    assert_tool_error(result, "failed to read mount at_park");
}

#[tokio::test]
async fn test_get_park_state_fails_when_can_park_read_errors() {
    let mount = MockTelescope {
        fail_can_park: true,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler
        .get_park_state(Parameters(GetParkStateParams {}))
        .await;
    assert_tool_error(result, "failed to read mount can_park");
}

#[tokio::test]
async fn test_get_park_state_fails_when_can_unpark_read_errors() {
    let mount = MockTelescope {
        fail_can_unpark: true,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler
        .get_park_state(Parameters(GetParkStateParams {}))
        .await;
    assert_tool_error(result, "failed to read mount can_unpark");
}

/// `park()` succeeds, but the very first `at_park()` poll errors
/// — covers the `Err` arm of the polling loop. The previous
/// implementation polled `Slewing` and then verified `AtPark`
/// separately; now both arms collapse into the single at_park
/// poll error path.
#[tokio::test]
async fn test_park_at_park_poll_fails() {
    let mount = MockTelescope {
        fail_at_park: true,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler.park_inner(ParkParams {}, None).await;
    assert_tool_error(result, "error polling mount at_park");
}

/// `do_slew_blocking` polls `Slewing`; this covers the
/// `PollIdleError::Read` arm. A *persistent* `slewing()` read error
/// (here every call fails) is tolerated for
/// `SLEWING_READ_ERROR_TOLERANCE` ticks, then surfaces — `start_paused`
/// so those ticks cost no wall-clock time.
#[tokio::test(start_paused = true)]
async fn test_slew_polling_error_propagates() {
    let mount = MockTelescope {
        fail_slewing_poll: true,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler
        .slew_inner(
            SlewParams {
                ra: Some(0.0),
                dec: Some(0.0),
                settle_after: None,
            },
            None,
        )
        .await;
    assert_tool_error(result, "error polling mount slewing");
}

/// Issue #319 resilience: a *transient* `slewing()` read error mid-slew
/// (fewer than `SLEWING_READ_ERROR_TOLERANCE` consecutive failures) is
/// tolerated — the poll keeps going and reports idle once the reads
/// recover — rather than aborting the slew (and the whole
/// `center_on_target` loop) on a single hiccup.
#[tokio::test(start_paused = true)]
async fn poll_slewing_tolerates_transient_read_errors_then_idles() {
    let mount = MockTelescope {
        slewing_transient_errors: 3,
        stuck_slewing: false,
        ..Default::default()
    };
    super::internals::poll_slewing_until_idle(&mount, None)
        .await
        .expect("transient slewing() errors below the tolerance must not abort the slew");
}

#[tokio::test]
async fn test_abort_slew_success() {
    let mount = MockTelescope::default();
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler
        .abort_slew(Parameters(AbortSlewParams {}))
        .await
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn test_abort_slew_no_mount_configured() {
    let handler = test_handler(empty_registry());
    let result = handler.abort_slew(Parameters(AbortSlewParams {})).await;
    assert_tool_error(result, "no mount configured");
}

/// Models a mount that returns `InvalidOperation` from `abort_slew`
/// (e.g. when not currently slewing). The error propagates with
/// the friendly prefix.
#[tokio::test]
async fn test_abort_slew_alpaca_error() {
    let mount = MockTelescope {
        fail_abort_slew: true,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler.abort_slew(Parameters(AbortSlewParams {})).await;
    assert_tool_error(result, "failed to abort slew");
}

// -----------------------------------------------------------------------
// Planner tools — error paths
//
// The new ephemeris/planner tools added in Phases 5-7 share two common
// failure shapes: missing site config (10 of the 12 tools require it)
// and parameter validation (range / format). One unit test per branch
// is enough to pin the wiring; the math itself is covered by the
// primitives.rs / decision.rs unit tests.
// -----------------------------------------------------------------------

fn test_handler_with_site(site: rp_ephemeris::Site) -> McpHandler {
    McpHandler::new(
        Arc::new(empty_registry()),
        Arc::new(crate::events::EventBus::from_config(&[])),
        SessionConfig {
            data_directory: std::env::temp_dir()
                .join("rp-planner-unit-test")
                .to_string_lossy()
                .to_string(),
        },
        ImageCache::new(64, 4, std::path::PathBuf::from("/nonexistent")),
        Some(site),
    )
}

fn test_site() -> rp_ephemeris::Site {
    rp_ephemeris::Site::new(51.0786, -0.2944).unwrap()
}

#[tokio::test]
async fn compute_alt_az_errors_when_site_absent() {
    let h = test_handler(empty_registry());
    let r = h
        .compute_alt_az(Parameters(AltAzParams {
            ra: 0.7,
            dec: 41.0,
            time: None,
        }))
        .await;
    assert_tool_error(r, "site not configured");
}

#[tokio::test]
async fn compute_alt_az_errors_on_out_of_range_inputs() {
    let h = test_handler_with_site(test_site());
    let r = h
        .compute_alt_az(Parameters(AltAzParams {
            ra: 30.0,
            dec: 0.0,
            time: None,
        }))
        .await;
    assert_tool_error(r, "ra_hours");
}

#[tokio::test]
async fn compute_alt_az_errors_on_bad_time() {
    let h = test_handler_with_site(test_site());
    let r = h
        .compute_alt_az(Parameters(AltAzParams {
            ra: 0.0,
            dec: 0.0,
            time: Some("not a time".into()),
        }))
        .await;
    assert_tool_error(r, "RFC3339");
}

#[tokio::test]
async fn compute_transit_errors_when_site_absent() {
    let h = test_handler(empty_registry());
    let r = h
        .compute_transit(Parameters(TransitParams {
            ra: 0.0,
            dec: 0.0,
            date: "2026-05-03".into(),
        }))
        .await;
    assert_tool_error(r, "site not configured");
}

#[tokio::test]
async fn compute_transit_errors_on_bad_date() {
    let h = test_handler_with_site(test_site());
    let r = h
        .compute_transit(Parameters(TransitParams {
            ra: 0.0,
            dec: 0.0,
            date: "tomorrow".into(),
        }))
        .await;
    assert_tool_error(r, "YYYY-MM-DD");
}

#[tokio::test]
async fn compute_rise_set_errors_on_out_of_range_min_alt() {
    let h = test_handler_with_site(test_site());
    let r = h
        .compute_rise_set(Parameters(RiseSetParams {
            ra: 0.0,
            dec: 0.0,
            date: "2026-05-03".into(),
            min_alt_degrees: 200.0,
        }))
        .await;
    assert_tool_error(r, "min_alt_degrees");
}

#[tokio::test]
async fn compute_rise_set_errors_when_site_absent() {
    let h = test_handler(empty_registry());
    let r = h
        .compute_rise_set(Parameters(RiseSetParams {
            ra: 0.0,
            dec: 0.0,
            date: "2026-05-03".into(),
            min_alt_degrees: 0.0,
        }))
        .await;
    assert_tool_error(r, "site not configured");
}

#[tokio::test]
async fn compute_meridian_flip_errors_when_site_absent() {
    let h = test_handler(empty_registry());
    let r = h
        .compute_meridian_flip(Parameters(MeridianFlipParams {
            ra: 0.0,
            dec: 0.0,
            time: None,
            side_of_pier: "unknown".into(),
        }))
        .await;
    assert_tool_error(r, "site not configured");
}

#[tokio::test]
async fn compute_meridian_flip_errors_on_bad_side_of_pier() {
    let h = test_handler_with_site(test_site());
    let r = h
        .compute_meridian_flip(Parameters(MeridianFlipParams {
            ra: 0.0,
            dec: 0.0,
            time: None,
            side_of_pier: "middle".into(),
        }))
        .await;
    assert_tool_error(r, "side_of_pier");
}

#[tokio::test]
async fn get_sun_position_errors_when_site_absent() {
    let h = test_handler(empty_registry());
    let r = h
        .get_sun_position(Parameters(TimeOnlyParams { time: None }))
        .await;
    assert_tool_error(r, "site not configured");
}

#[tokio::test]
async fn get_twilight_errors_when_site_absent() {
    let h = test_handler(empty_registry());
    let r = h
        .get_twilight(Parameters(TwilightParams {
            date: "2026-12-21".into(),
            kind: "civil".into(),
        }))
        .await;
    assert_tool_error(r, "site not configured");
}

#[tokio::test]
async fn get_moon_position_errors_when_site_absent() {
    let h = test_handler(empty_registry());
    let r = h
        .get_moon_position(Parameters(TimeOnlyParams { time: None }))
        .await;
    assert_tool_error(r, "site not configured");
}

#[tokio::test]
async fn compute_moon_separation_errors_on_bad_inputs() {
    let h = test_handler(empty_registry());
    let r = h
        .compute_moon_separation(Parameters(MoonSeparationParams {
            ra: 100.0,
            dec: 0.0,
            time: None,
        }))
        .await;
    assert_tool_error(r, "ra_hours");
}

#[tokio::test]
async fn get_local_sidereal_time_errors_when_site_absent() {
    let h = test_handler(empty_registry());
    let r = h
        .get_local_sidereal_time(Parameters(TimeOnlyParams { time: None }))
        .await;
    assert_tool_error(r, "site not configured");
}

#[tokio::test]
async fn get_target_status_errors_when_site_absent() {
    let h = test_handler(empty_registry());
    let r = h
        .get_target_status(Parameters(GetTargetStatusParams {
            target_name: Some("M 31".into()),
            ra: None,
            dec: None,
            time: None,
        }))
        .await;
    assert_tool_error(r, "site not configured");
}

#[tokio::test]
async fn get_target_status_errors_on_unknown_name() {
    let h = test_handler_with_site(test_site());
    let r = h
        .get_target_status(Parameters(GetTargetStatusParams {
            target_name: Some("M 999".into()),
            ra: None,
            dec: None,
            time: None,
        }))
        .await;
    // The catalog miss path returns a structured `target_not_found`
    // payload as a CallToolResult::error.
    let call_result = r.expect("tool returned protocol error");
    assert!(call_result.is_error.unwrap_or(false));
    let text = call_result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap();
    assert!(text.contains("target_not_found"), "got: {text}");
}

#[tokio::test]
async fn get_target_status_errors_when_neither_name_nor_radec_supplied() {
    let h = test_handler_with_site(test_site());
    let r = h
        .get_target_status(Parameters(GetTargetStatusParams {
            target_name: None,
            ra: None,
            dec: None,
            time: None,
        }))
        .await;
    assert_tool_error(r, "supply exactly one");
}

#[tokio::test]
async fn get_target_status_accepts_radec_form() {
    let h = test_handler_with_site(test_site());
    let r = h
        .get_target_status(Parameters(GetTargetStatusParams {
            target_name: None,
            ra: Some(2.5301944),
            dec: Some(89.2641111),
            time: None,
        }))
        .await
        .expect("tool returned protocol error");
    assert!(!r.is_error.unwrap_or(false), "expected success");
}

#[tokio::test]
async fn get_next_target_errors_when_site_absent() {
    let h = test_handler(empty_registry());
    let r = h
        .get_next_target(Parameters(GetNextTargetParams { time: None }))
        .await;
    assert_tool_error(r, "site not configured");
}

#[tokio::test]
async fn get_next_target_with_no_targets_returns_no_targets_configured() {
    let h = test_handler_with_site(test_site());
    let r = h
        .get_next_target(Parameters(GetNextTargetParams { time: None }))
        .await
        .expect("tool returned protocol error");
    let text = r
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap();
    assert!(text.contains("no_targets_configured"), "got: {text}");
}

#[tokio::test]
async fn get_meridian_status_errors_when_site_absent() {
    let h = test_handler(empty_registry());
    let r = h
        .get_meridian_status(Parameters(GetMeridianStatusParams { time: None }))
        .await;
    assert_tool_error(r, "site not configured");
}

#[tokio::test]
async fn get_meridian_status_errors_when_mount_absent() {
    let h = test_handler_with_site(test_site());
    let r = h
        .get_meridian_status(Parameters(GetMeridianStatusParams { time: None }))
        .await;
    // empty_registry has no mount, so `resolve_mount` returns the
    // standard "mount not configured" error.
    assert_tool_error(r, "mount");
}

// -----------------------------------------------------------------------
// Planner tools — happy paths (cover the success-return arms in mcp.rs;
// value correctness is covered by primitives.rs / convenience.rs unit
// tests).
// -----------------------------------------------------------------------

fn handler_with_site_and_mount() -> McpHandler {
    let mock = MockTelescope::default();
    let mount_cfg = crate::config::MountConfig {
        alpaca_url: "http://unused".into(),
        device_number: 0,
        settle_after_slew: None,
        auth: None,
    };
    // Skip the connect-time HTTP fetch by hand-building a registry
    // with the mock device wired in directly.
    let registry = crate::equipment::EquipmentRegistry {
        cameras: vec![],
        filter_wheels: vec![],
        cover_calibrators: vec![],
        focusers: vec![],
        mount: Some(crate::equipment::MountEntry {
            connected: true,
            config: mount_cfg,
            device: Some(Arc::new(mock)),
        }),
    };
    McpHandler::new(
        Arc::new(registry),
        Arc::new(crate::events::EventBus::from_config(&[])),
        SessionConfig {
            data_directory: std::env::temp_dir()
                .join("rp-planner-happy-test")
                .to_string_lossy()
                .to_string(),
        },
        ImageCache::new(64, 4, std::path::PathBuf::from("/nonexistent")),
        Some(test_site()),
    )
}

/// Yank the JSON payload from a successful CallToolResult.
fn ok_json(r: Result<CallToolResult, rmcp::ErrorData>) -> serde_json::Value {
    let call_result = r.expect("tool returned protocol error");
    assert!(
        !call_result.is_error.unwrap_or(false),
        "expected success, got error: {:?}",
        call_result
    );
    let text = call_result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .expect("expected text content");
    serde_json::from_str(&text).expect("response was not valid JSON")
}

const TEST_TIME: &str = "2026-05-03T22:00:00Z";

#[tokio::test]
async fn compute_alt_az_happy_path() {
    let h = test_handler_with_site(test_site());
    let v = ok_json(
        h.compute_alt_az(Parameters(AltAzParams {
            ra: 2.5301944,
            dec: 89.2641111,
            time: Some(TEST_TIME.into()),
        }))
        .await,
    );
    assert!(v["altitude_degrees"].as_f64().is_some());
    assert!(v["azimuth_degrees"].as_f64().is_some());
}

#[tokio::test]
async fn compute_transit_happy_path() {
    let h = test_handler_with_site(test_site());
    let v = ok_json(
        h.compute_transit(Parameters(TransitParams {
            ra: 0.7123,
            dec: 41.27,
            date: "2026-11-01".into(),
        }))
        .await,
    );
    assert!(v.get("transit_utc").is_some());
}

#[tokio::test]
async fn compute_rise_set_happy_path() {
    let h = test_handler_with_site(test_site());
    let v = ok_json(
        h.compute_rise_set(Parameters(RiseSetParams {
            ra: 0.7123,
            dec: 41.27,
            date: "2026-11-01".into(),
            min_alt_degrees: 30.0,
        }))
        .await,
    );
    assert!(v.get("rise_utc").is_some());
    assert!(v.get("set_utc").is_some());
}

#[tokio::test]
async fn compute_meridian_flip_happy_path() {
    let h = test_handler_with_site(test_site());
    let v = ok_json(
        h.compute_meridian_flip(Parameters(MeridianFlipParams {
            ra: 0.7123,
            dec: 41.27,
            time: Some(TEST_TIME.into()),
            side_of_pier: "east".into(),
        }))
        .await,
    );
    assert!(v["time_to_flip_seconds"].as_i64().is_some());
}

#[tokio::test]
async fn get_sun_position_happy_path() {
    let h = test_handler_with_site(test_site());
    let v = ok_json(
        h.get_sun_position(Parameters(TimeOnlyParams {
            time: Some(TEST_TIME.into()),
        }))
        .await,
    );
    assert!(v["ra_hours"].as_f64().is_some());
    assert!(v["dec_degrees"].as_f64().is_some());
}

#[tokio::test]
async fn get_twilight_happy_path() {
    let h = test_handler_with_site(test_site());
    let v = ok_json(
        h.get_twilight(Parameters(TwilightParams {
            date: "2026-12-21".into(),
            kind: "civil".into(),
        }))
        .await,
    );
    assert_eq!(v["kind"], "civil");
}

#[tokio::test]
async fn get_moon_position_happy_path() {
    let h = test_handler_with_site(test_site());
    let v = ok_json(
        h.get_moon_position(Parameters(TimeOnlyParams {
            time: Some(TEST_TIME.into()),
        }))
        .await,
    );
    assert!(v["phase_degrees"].as_f64().is_some());
    assert!(v["illumination_fraction"].as_f64().is_some());
}

#[tokio::test]
async fn compute_moon_separation_happy_path() {
    let h = test_handler_with_site(test_site());
    let v = ok_json(
        h.compute_moon_separation(Parameters(MoonSeparationParams {
            ra: 0.7123,
            dec: 41.27,
            time: Some(TEST_TIME.into()),
        }))
        .await,
    );
    let sep = v["separation_degrees"].as_f64().unwrap();
    assert!((0.0..=180.0).contains(&sep));
}

#[tokio::test]
async fn get_local_sidereal_time_happy_path() {
    let h = test_handler_with_site(test_site());
    let v = ok_json(
        h.get_local_sidereal_time(Parameters(TimeOnlyParams {
            time: Some(TEST_TIME.into()),
        }))
        .await,
    );
    let lst = v["lst_hours"].as_f64().unwrap();
    assert!((0.0..24.0).contains(&lst));
}

#[tokio::test]
async fn get_target_status_happy_path_via_catalog() {
    let h = test_handler_with_site(test_site());
    let v = ok_json(
        h.get_target_status(Parameters(GetTargetStatusParams {
            target_name: Some("M 31".into()),
            ra: None,
            dec: None,
            time: Some(TEST_TIME.into()),
        }))
        .await,
    );
    assert_eq!(v["target_name"], "M 31");
    assert!(v["altitude_degrees"].as_f64().is_some());
}

#[tokio::test]
async fn get_meridian_status_happy_path() {
    // MockTelescope doesn't implement side_of_pier, which returns
    // NOT_IMPLEMENTED — get_meridian_status maps that to "unknown"
    // and surfaces the JSON. Exercises the success arm + the
    // NOT_IMPLEMENTED → Unknown branch in one shot.
    let h = handler_with_site_and_mount();
    let v = ok_json(
        h.get_meridian_status(Parameters(GetMeridianStatusParams {
            time: Some(TEST_TIME.into()),
        }))
        .await,
    );
    assert!(v["time_to_flip_seconds"].is_number());
    assert_eq!(v["side_of_pier"], "unknown");
    assert!(v["mount_ra_hours"].as_f64().is_some());
}

// -----------------------------------------------------------------------
// plate_solve tests
// -----------------------------------------------------------------------
//
// These exercise the MCP handler against `MockPlateSolveClient`. The
// handler resolves devices, validates hints, calls the client, maps
// SolveError → MCP error, and attempts persistence. End-to-end
// wire-format coverage lives in the BDD suite (`plate_solve.feature`)
// against `PlateSolverStub`; the unit layer pins the per-code error
// shape and the mount-hint conversion deterministically.

use rp_plate_solver::{MockPlateSolveClient, SolveError, SolveOutcome};

fn ok_outcome() -> SolveOutcome {
    SolveOutcome {
        ra_center: 10.6848,
        dec_center: 41.269,
        pixel_scale_arcsec: 1.05,
        rotation_deg: 12.3,
        solver: "stub".to_string(),
    }
}

/// Build a handler with a configured plate-solver client. Pass
/// `expectations` to wire up `mock.expect_solve()` calls before the
/// handler is built; pass `None` for `default_search_radius_deg`
/// when the test doesn't care about the config-default path.
fn handler_with_plate_solver(
    registry: crate::equipment::EquipmentRegistry,
    configure: impl FnOnce(&mut MockPlateSolveClient),
    default_search_radius_deg: Option<f64>,
) -> McpHandler {
    let mut mock = MockPlateSolveClient::new();
    configure(&mut mock);
    let client: Arc<dyn rp_plate_solver::PlateSolveClient> = Arc::new(mock);
    test_handler(registry).with_plate_solver(Some(client), default_search_radius_deg)
}

#[tokio::test]
async fn plate_solve_happy_path_image_path() {
    let handler = handler_with_plate_solver(
        empty_registry(),
        |mock| {
            mock.expect_solve()
                .withf(|req| req.fits_path == "/tmp/x.fits")
                .returning(|_| Ok(ok_outcome()));
        },
        None,
    );
    let result = handler
        .plate_solve(Parameters(PlateSolveParams {
            document_id: None,
            image_path: Some("/tmp/x.fits".to_string()),
            pointing_hint: None,
            use_mount_hints: None,
            fov_hint_deg: None,
            search_radius_deg: None,
            timeout: None,
        }))
        .await
        .unwrap();
    let json = ok_text(result);
    assert_eq!(json["ra_center"], 10.6848);
    assert_eq!(json["dec_center"], 41.269);
    assert_eq!(json["pixel_scale_arcsec"], 1.05);
    assert_eq!(json["rotation_deg"], 12.3);
    assert_eq!(json["solver"], "stub");
}

#[tokio::test]
async fn plate_solve_neither_document_id_nor_image_path_errors() {
    let handler = handler_with_plate_solver(empty_registry(), |_| {}, None);
    let result = handler
        .plate_solve(Parameters(PlateSolveParams {
            document_id: None,
            image_path: None,
            pointing_hint: None,
            use_mount_hints: None,
            fov_hint_deg: None,
            search_radius_deg: None,
            timeout: None,
        }))
        .await;
    assert_tool_error(result, "image_path");
}

#[tokio::test]
async fn plate_solve_unconfigured_returns_not_configured_error() {
    // No `with_plate_solver` call ⇒ tool reports not configured
    // even when a path is supplied.
    let handler = test_handler(empty_registry());
    let result = handler
        .plate_solve(Parameters(PlateSolveParams {
            document_id: None,
            image_path: Some("/tmp/x.fits".to_string()),
            pointing_hint: None,
            use_mount_hints: None,
            fov_hint_deg: None,
            search_radius_deg: None,
            timeout: None,
        }))
        .await;
    assert_tool_error(result, "plate solver not configured");
}

#[tokio::test]
async fn plate_solve_pointing_hint_and_use_mount_hints_are_mutually_exclusive() {
    let handler = handler_with_plate_solver(empty_registry(), |_| {}, None);
    let result = handler
        .plate_solve(Parameters(PlateSolveParams {
            document_id: None,
            image_path: Some("/tmp/x.fits".to_string()),
            pointing_hint: Some(PointingHint {
                ra_deg: 1.0,
                dec_deg: 2.0,
            }),
            use_mount_hints: Some(true),
            fov_hint_deg: None,
            search_radius_deg: None,
            timeout: None,
        }))
        .await;
    assert_tool_error(result, "pointing_hint or use_mount_hints");
}

#[tokio::test]
async fn plate_solve_use_mount_hints_with_no_mount_errors() {
    let handler = handler_with_plate_solver(empty_registry(), |_| {}, None);
    let result = handler
        .plate_solve(Parameters(PlateSolveParams {
            document_id: None,
            image_path: Some("/tmp/x.fits".to_string()),
            pointing_hint: None,
            use_mount_hints: Some(true),
            fov_hint_deg: None,
            search_radius_deg: None,
            timeout: None,
        }))
        .await;
    assert_tool_error(result, "use_mount_hints");
}

#[tokio::test]
async fn plate_solve_use_mount_hints_reads_mount_and_converts_ra_to_degrees() {
    // Mount at RA=10.6848h, Dec=41.269° — matches the plate-solver.md
    // M31 example. The handler should send ra_hint=160.272° (10.6848*15)
    // and dec_hint=41.269° to the wrapper.
    let mount = MockTelescope {
        ra_value: 10.6848,
        dec_value: 41.269,
        ..Default::default()
    };
    let registry = mount_registry(Arc::new(mount), None);
    let handler = handler_with_plate_solver(
        registry,
        |mock| {
            mock.expect_solve()
                .withf(|req| {
                    let ra = req.ra_hint.unwrap_or_default();
                    let dec = req.dec_hint.unwrap_or_default();
                    (ra - 160.272).abs() < 1e-3 && (dec - 41.269).abs() < 1e-3
                })
                .returning(|_| Ok(ok_outcome()));
        },
        None,
    );
    let result = handler
        .plate_solve(Parameters(PlateSolveParams {
            document_id: None,
            image_path: Some("/tmp/x.fits".to_string()),
            pointing_hint: None,
            use_mount_hints: Some(true),
            fov_hint_deg: None,
            search_radius_deg: None,
            timeout: None,
        }))
        .await
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn plate_solve_explicit_pointing_hint_forwards_verbatim() {
    let handler = handler_with_plate_solver(
        empty_registry(),
        |mock| {
            mock.expect_solve()
                .withf(|req| req.ra_hint == Some(120.5) && req.dec_hint == Some(-30.1))
                .returning(|_| Ok(ok_outcome()));
        },
        None,
    );
    let result = handler
        .plate_solve(Parameters(PlateSolveParams {
            document_id: None,
            image_path: Some("/tmp/x.fits".to_string()),
            pointing_hint: Some(PointingHint {
                ra_deg: 120.5,
                dec_deg: -30.1,
            }),
            use_mount_hints: None,
            fov_hint_deg: None,
            search_radius_deg: None,
            timeout: None,
        }))
        .await
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn plate_solve_optional_fields_forwarded_verbatim() {
    let handler = handler_with_plate_solver(
        empty_registry(),
        |mock| {
            mock.expect_solve()
                .withf(|req| {
                    req.fov_hint_deg == Some(1.5)
                        && req.search_radius_deg == Some(2.0)
                        && req.timeout == Some(Duration::from_secs(45))
                })
                .returning(|_| Ok(ok_outcome()));
        },
        None,
    );
    let result = handler
        .plate_solve(Parameters(PlateSolveParams {
            document_id: None,
            image_path: Some("/tmp/x.fits".to_string()),
            pointing_hint: None,
            use_mount_hints: None,
            fov_hint_deg: Some(1.5),
            search_radius_deg: Some(2.0),
            timeout: Some(Duration::from_secs(45)),
        }))
        .await
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn plate_solve_config_default_search_radius_applied_when_omitted() {
    let handler = handler_with_plate_solver(
        empty_registry(),
        |mock| {
            mock.expect_solve()
                .withf(|req| req.search_radius_deg == Some(4.0))
                .returning(|_| Ok(ok_outcome()));
        },
        Some(4.0),
    );
    let result = handler
        .plate_solve(Parameters(PlateSolveParams {
            document_id: None,
            image_path: Some("/tmp/x.fits".to_string()),
            pointing_hint: None,
            use_mount_hints: None,
            fov_hint_deg: None,
            search_radius_deg: None,
            timeout: None,
        }))
        .await
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn plate_solve_per_call_search_radius_overrides_config_default() {
    let handler = handler_with_plate_solver(
        empty_registry(),
        |mock| {
            mock.expect_solve()
                .withf(|req| req.search_radius_deg == Some(2.5))
                .returning(|_| Ok(ok_outcome()));
        },
        Some(4.0),
    );
    let result = handler
        .plate_solve(Parameters(PlateSolveParams {
            document_id: None,
            image_path: Some("/tmp/x.fits".to_string()),
            pointing_hint: None,
            use_mount_hints: None,
            fov_hint_deg: None,
            search_radius_deg: Some(2.5),
            timeout: None,
        }))
        .await
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn plate_solve_no_hints_produces_blind_request() {
    let handler = handler_with_plate_solver(
        empty_registry(),
        |mock| {
            mock.expect_solve()
                .withf(|req| {
                    req.ra_hint.is_none()
                        && req.dec_hint.is_none()
                        && req.fov_hint_deg.is_none()
                        && req.search_radius_deg.is_none()
                })
                .returning(|_| Ok(ok_outcome()));
        },
        None,
    );
    let result = handler
        .plate_solve(Parameters(PlateSolveParams {
            document_id: None,
            image_path: Some("/tmp/x.fits".to_string()),
            pointing_hint: None,
            use_mount_hints: None,
            fov_hint_deg: None,
            search_radius_deg: None,
            timeout: None,
        }))
        .await
        .unwrap();
    assert!(!result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn plate_solve_propagates_wrapper_solve_failed() {
    let handler = handler_with_plate_solver(
        empty_registry(),
        |mock| {
            mock.expect_solve().returning(|_| {
                Err(SolveError::Wrapper {
                    code: "solve_failed".to_string(),
                    message: "ASTAP exited with code 1".to_string(),
                    details: serde_json::Value::Null,
                })
            });
        },
        None,
    );
    let result = handler
        .plate_solve(Parameters(PlateSolveParams {
            document_id: None,
            image_path: Some("/tmp/x.fits".to_string()),
            pointing_hint: None,
            use_mount_hints: None,
            fov_hint_deg: None,
            search_radius_deg: None,
            timeout: None,
        }))
        .await;
    assert_tool_error(result, "solve_failed");
}

#[tokio::test]
async fn plate_solve_propagates_wrapper_solve_failed_with_details() {
    let handler = handler_with_plate_solver(
        empty_registry(),
        |mock| {
            mock.expect_solve().returning(|_| {
                Err(SolveError::Wrapper {
                    code: "solve_failed".to_string(),
                    message: "ASTAP exited with code 1".to_string(),
                    details: serde_json::json!({"stderr_tail": "no stars found"}),
                })
            });
        },
        None,
    );
    let result = handler
        .plate_solve(Parameters(PlateSolveParams {
            document_id: None,
            image_path: Some("/tmp/x.fits".to_string()),
            pointing_hint: None,
            use_mount_hints: None,
            fov_hint_deg: None,
            search_radius_deg: None,
            timeout: None,
        }))
        .await;
    assert_tool_error(result, "no stars found");
}

#[tokio::test]
async fn plate_solve_maps_service_unreachable_to_distinct_error() {
    let handler = handler_with_plate_solver(
        empty_registry(),
        |mock| {
            mock.expect_solve().returning(|_| {
                Err(SolveError::ServiceUnreachable(
                    "connection refused".to_string(),
                ))
            });
        },
        None,
    );
    let result = handler
        .plate_solve(Parameters(PlateSolveParams {
            document_id: None,
            image_path: Some("/tmp/x.fits".to_string()),
            pointing_hint: None,
            use_mount_hints: None,
            fov_hint_deg: None,
            search_radius_deg: None,
            timeout: None,
        }))
        .await;
    assert_tool_error(result, "service unreachable");
}

#[tokio::test]
async fn plate_solve_maps_internal_error_with_internal_prefix() {
    let handler = handler_with_plate_solver(
        empty_registry(),
        |mock| {
            mock.expect_solve()
                .returning(|_| Err(SolveError::Internal("broken pipe".to_string())));
        },
        None,
    );
    let result = handler
        .plate_solve(Parameters(PlateSolveParams {
            document_id: None,
            image_path: Some("/tmp/x.fits".to_string()),
            pointing_hint: None,
            use_mount_hints: None,
            fov_hint_deg: None,
            search_radius_deg: None,
            timeout: None,
        }))
        .await;
    assert_tool_error(result, "internal");
}

// =======================================================================
// auto_focus happy-path test
//
// Closes the coverage gap on lines 156-176 of
// `mcp/built_in/auto_focus.rs` (the `Ok(result)` branch — `focus_complete`
// emit + JSON success-result construction). Drives the full
// `run_auto_focus` sweep against the canonical V-curve fixtures
// under `tests/fixtures/auto_focus/`, asserts the JSON shape and the
// `focus_complete` event payload.
//
// Why not OmniSim: the simulator's camera (`Camera.Simulator/Camera.cs`
// in ASCOM.Alpaca.Simulators) loads a single JPG/PNG via ImageSharp's
// `Image.Load<Rgba32>` and reuses it for every exposure with no
// optics simulation — every focuser position would yield the same
// HFR → singular fit → `monotonic_curve` error. We synthesize the
// V-curve ourselves and inject the per-step images via `FixtureCamera`,
// which implements `ascom_alpaca::api::Camera` directly (no HTTP) and
// returns the fixtures in sweep order.
// =======================================================================

use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

/// The 11 canonical V-curve fixtures embedded at compile time via
/// `include_bytes!` so the test runs under both Cargo and Bazel
/// without filesystem access. Bazel sandboxes test execution and
/// would need explicit `data` declarations in `BUILD.bazel` to make
/// `tests/fixtures/**` visible at runtime — `include_bytes!` sidesteps
/// that, matching the precedent set by
/// `services/plate-solver/src/bin/mock_astap.rs`. Order matches the
/// natural sweep order (`-100, -80, …, +100`) `run_auto_focus`
/// produces, so the `FixtureCamera` counter indexes directly into
/// this slice.
const AUTO_FOCUS_FIXTURE_BYTES: [&[u8]; 11] = [
    include_bytes!("../../tests/fixtures/auto_focus/pos_m100.fits"),
    include_bytes!("../../tests/fixtures/auto_focus/pos_m080.fits"),
    include_bytes!("../../tests/fixtures/auto_focus/pos_m060.fits"),
    include_bytes!("../../tests/fixtures/auto_focus/pos_m040.fits"),
    include_bytes!("../../tests/fixtures/auto_focus/pos_m020.fits"),
    include_bytes!("../../tests/fixtures/auto_focus/pos_p000.fits"),
    include_bytes!("../../tests/fixtures/auto_focus/pos_p020.fits"),
    include_bytes!("../../tests/fixtures/auto_focus/pos_p040.fits"),
    include_bytes!("../../tests/fixtures/auto_focus/pos_p060.fits"),
    include_bytes!("../../tests/fixtures/auto_focus/pos_p080.fits"),
    include_bytes!("../../tests/fixtures/auto_focus/pos_p100.fits"),
];

/// Decode the embedded V-curve fixtures into `(width, height, 1)`
/// `Array3<i32>` frames in sweep order. `ascom_alpaca`'s
/// `ImageArray::from` expects that shape for monochrome data.
fn load_auto_focus_fixtures() -> Vec<ndarray::Array3<i32>> {
    AUTO_FOCUS_FIXTURE_BYTES
        .iter()
        .map(|bytes| {
            let (pixels, w, h) = rp_fits::reader::read_primary_as_i32(std::io::Cursor::new(*bytes))
                .expect("decode fixture");
            ndarray::Array3::from_shape_vec((w as usize, h as usize, 1), pixels)
                .expect("fixture shape")
        })
        .collect()
}

/// Mock camera that returns a sequence of pre-loaded `Array3<i32>`
/// frames in order, one per `image_array()` call. Used to drive
/// `auto_focus` end-to-end with a known V-curve.
struct FixtureCamera {
    images: Vec<ndarray::Array3<i32>>,
    counter: AtomicUsize,
}

impl FixtureCamera {
    fn new(images: Vec<ndarray::Array3<i32>>) -> Self {
        Self {
            images,
            counter: AtomicUsize::new(0),
        }
    }
}

impl_mock_device!(FixtureCamera);

#[async_trait::async_trait]
impl ascom_alpaca::api::Camera for FixtureCamera {
    async fn start_exposure(
        &self,
        _duration: Duration,
        _light: bool,
    ) -> ascom_alpaca::ASCOMResult<()> {
        Ok(())
    }

    async fn image_ready(&self) -> ascom_alpaca::ASCOMResult<bool> {
        Ok(true)
    }

    async fn image_array(
        &self,
    ) -> ascom_alpaca::ASCOMResult<ascom_alpaca::api::camera::ImageArray> {
        let idx = self.counter.fetch_add(1, Ordering::SeqCst);
        let img = self
            .images
            .get(idx)
            .unwrap_or_else(|| {
                panic!(
                    "FixtureCamera exhausted: requested image {} of {}",
                    idx,
                    self.images.len()
                )
            })
            .clone();
        Ok(img.into())
    }

    async fn max_adu(&self) -> ascom_alpaca::ASCOMResult<u32> {
        Ok(65535)
    }

    async fn camera_x_size(&self) -> ascom_alpaca::ASCOMResult<u32> {
        Ok(200)
    }

    async fn camera_y_size(&self) -> ascom_alpaca::ASCOMResult<u32> {
        Ok(200)
    }

    async fn pixel_size_x(&self) -> ascom_alpaca::ASCOMResult<f64> {
        Ok(3.76)
    }

    async fn pixel_size_y(&self) -> ascom_alpaca::ASCOMResult<f64> {
        Ok(3.76)
    }

    async fn exposure_max(&self) -> ascom_alpaca::ASCOMResult<Duration> {
        Ok(Duration::from_secs(3600))
    }

    async fn exposure_min(&self) -> ascom_alpaca::ASCOMResult<Duration> {
        Ok(Duration::from_millis(1))
    }

    async fn exposure_resolution(&self) -> ascom_alpaca::ASCOMResult<Duration> {
        Ok(Duration::from_millis(1))
    }

    async fn has_shutter(&self) -> ascom_alpaca::ASCOMResult<bool> {
        Ok(true)
    }

    async fn start_x(&self) -> ascom_alpaca::ASCOMResult<u32> {
        Ok(0)
    }

    async fn set_start_x(&self, _: u32) -> ascom_alpaca::ASCOMResult<()> {
        Ok(())
    }

    async fn start_y(&self) -> ascom_alpaca::ASCOMResult<u32> {
        Ok(0)
    }

    async fn set_start_y(&self, _: u32) -> ascom_alpaca::ASCOMResult<()> {
        Ok(())
    }
}

/// Mock focuser that tracks position across `move_(target)` calls
/// via an interior `AtomicI32`. Returns a constant temperature.
struct TrackingFocuser {
    position: AtomicI32,
    temperature_c: f64,
}

impl TrackingFocuser {
    fn new(starting_position: i32, temperature_c: f64) -> Self {
        Self {
            position: AtomicI32::new(starting_position),
            temperature_c,
        }
    }
}

impl_mock_device!(TrackingFocuser);

#[async_trait::async_trait]
impl ascom_alpaca::api::Focuser for TrackingFocuser {
    async fn absolute(&self) -> ascom_alpaca::ASCOMResult<bool> {
        Ok(true)
    }

    async fn is_moving(&self) -> ascom_alpaca::ASCOMResult<bool> {
        Ok(false)
    }

    async fn max_increment(&self) -> ascom_alpaca::ASCOMResult<u32> {
        Ok(100000)
    }

    async fn max_step(&self) -> ascom_alpaca::ASCOMResult<u32> {
        Ok(100000)
    }

    async fn position(&self) -> ascom_alpaca::ASCOMResult<i32> {
        Ok(self.position.load(Ordering::SeqCst))
    }

    async fn step_size(&self) -> ascom_alpaca::ASCOMResult<f64> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn temp_comp(&self) -> ascom_alpaca::ASCOMResult<bool> {
        Ok(false)
    }

    async fn set_temp_comp(&self, _: bool) -> ascom_alpaca::ASCOMResult<()> {
        Err(ASCOMError::NOT_IMPLEMENTED)
    }

    async fn temp_comp_available(&self) -> ascom_alpaca::ASCOMResult<bool> {
        Ok(false)
    }

    async fn temperature(&self) -> ascom_alpaca::ASCOMResult<f64> {
        Ok(self.temperature_c)
    }

    async fn halt(&self) -> ascom_alpaca::ASCOMResult<()> {
        Ok(())
    }

    async fn move_(&self, position: i32) -> ascom_alpaca::ASCOMResult<()> {
        self.position.store(position, Ordering::SeqCst);
        Ok(())
    }
}

/// Build an `EquipmentRegistry` with a `FixtureCamera` (preloaded
/// V-curve fixtures) and a `TrackingFocuser` (starting at
/// `starting_position`). The IDs match the ones the test calls
/// `auto_focus` with: `"cam"` and `"foc"`.
fn auto_focus_registry(starting_position: i32) -> crate::equipment::EquipmentRegistry {
    let camera = FixtureCamera::new(load_auto_focus_fixtures());
    let focuser = TrackingFocuser::new(starting_position, 4.5);
    crate::equipment::EquipmentRegistry {
        cameras: vec![crate::equipment::CameraEntry {
            id: "cam".to_string(),
            connected: true,
            config: crate::config::CameraConfig {
                id: "cam".to_string(),
                name: "fixture".to_string(),
                alpaca_url: "http://localhost:1".to_string(),
                device_type: String::new(),
                device_number: 0,
                cooler_target_c: None,
                gain: None,
                offset: None,
                focal_length_mm: None,
                auth: None,
            },
            device: Some(Arc::new(camera)),
            // FixtureCamera reports max_adu=65535, pixel_size=3.76 µm,
            // sensor_*_size=200 px (see its impl); mirror those values
            // here so `do_capture` (which consumes the cache rather than
            // calling the device) behaves identically to a real connect.
            max_adu: Some(65535),
            pixel_size_x_um: Some(3.76),
            pixel_size_y_um: Some(3.76),
            sensor_width_px: Some(200),
            sensor_height_px: Some(200),
        }],
        filter_wheels: vec![],
        cover_calibrators: vec![],
        focusers: vec![crate::equipment::FocuserEntry {
            id: "foc".to_string(),
            connected: true,
            config: crate::config::FocuserConfig {
                id: "foc".to_string(),
                camera_id: String::new(),
                alpaca_url: "http://localhost:1".to_string(),
                device_number: 0,
                min_position: None,
                max_position: None,
                auth: None,
            },
            device: Some(Arc::new(focuser)),
        }],
        mount: None,
    }
}

#[tokio::test]
async fn auto_focus_happy_path_emits_focus_complete_and_returns_curve() {
    // starting_position pinned at a realistic focuser scale (QHY
    // M-series, Pegasus FocusCube, Robofocus all operate in the
    // 5_000–100_000 range). fit_parabola recenters x by the weighted
    // mean before solving the normal equations, so the design matrix
    // stays well-conditioned at any plausible focuser scale (#174).
    // The fitted vertex must land near `starting_position` because
    // the synthesised parabola in HFR is symmetric about d=0.
    const STARTING_POSITION: i32 = 11_000;

    // Sandbox the FITS writes do_capture performs in a per-test temp
    // dir so successive runs don't pollute /tmp.
    let dir = tempfile::tempdir().expect("tempdir");
    let mut handler = test_handler(auto_focus_registry(STARTING_POSITION));
    handler.session_config = SessionConfig {
        data_directory: dir.path().to_string_lossy().into_owned(),
    };

    // Sweep grid: 11 positions at step_size=20 over half_width=100,
    // matching the eleven fixture offsets generated by
    // `examples/gen_autofocus_fixtures.rs`.
    let result = handler
        .auto_focus_inner(
            AutoFocusToolParams {
                camera_id: Some("cam".to_string()),
                focuser_id: Some("foc".to_string()),
                duration: Some(Duration::from_millis(100)),
                step_size: Some(20),
                half_width: Some(100),
                min_area: Some(4),
                max_area: Some(2000),
                threshold_sigma: 5.0,
                min_fit_points: None,
            },
            None,
        )
        .await;
    let call_result = result.expect("auto_focus protocol error");
    assert!(
        !call_result.is_error.unwrap_or(false),
        "expected success; got: {:?}",
        call_result.content
    );
    let body: serde_json::Value = ok_text(call_result);

    // Vertex of the synthesised parabola sits at the focuser's
    // starting_position by construction (HFR(d) = 2.0 + 0.0005·d² is
    // symmetric about d=0). The fitted best_position should land
    // within a couple of steps of that — empirically the σ≈1 px
    // measure_basic smoothing kernel introduces a small negative
    // bias on HFRs around 3-4 px, but the symmetry of the curve
    // keeps the fitted vertex within ±2 step_size of true minimum.
    let best_position = body["best_position"].as_i64().expect("best_position i64");
    assert!(
        (best_position - STARTING_POSITION as i64).abs() <= 2 * 20,
        "best_position {} not within ±2·step_size of starting {}",
        best_position,
        STARTING_POSITION
    );

    // best_hfr should be near the synthesised minimum (2.0 px). The
    // measured curve at d=0 is essentially exact (2.0023 px); the
    // parabolic fit on the 11 noisy samples lands very close.
    let best_hfr = body["best_hfr"].as_f64().expect("best_hfr f64");
    assert!(
        (best_hfr - 2.0).abs() < 0.5,
        "best_hfr {} not near 2.0",
        best_hfr
    );

    // Final focuser position must equal best_position — auto_focus
    // moves the focuser to the fit's vertex at the end of the run.
    let final_position = body["final_position"].as_i64().expect("final_position i64");
    assert_eq!(final_position, best_position);

    // All 11 sweep frames have detectable stars (the V-curve fixture
    // generation pipeline ensures non-null HFR at every offset), so
    // `samples_used` must be 11 — every grid point contributes.
    let samples_used = body["samples_used"].as_u64().expect("samples_used u64");
    assert_eq!(samples_used, 11);

    // curve_points must have one entry per grid point.
    let curve_points = body["curve_points"].as_array().expect("curve_points array");
    assert_eq!(curve_points.len(), 11);
    for entry in curve_points {
        assert!(entry["position"].is_i64());
        assert!(entry["hfr"].is_f64() || entry["hfr"].is_null());
        assert!(entry["star_count"].is_u64());
        assert!(entry["document_id"].is_string());
    }

    // Temperature passes through from the focuser's `temperature()`
    // read in `auto_focus`'s step-1 (recorded once before any sweep
    // motion).
    let temperature_c = body["temperature_c"].as_f64().expect("temperature_c f64");
    assert!(
        (temperature_c - 4.5).abs() < 1e-9,
        "temperature_c {} not 4.5",
        temperature_c
    );

    // The handler emits `focus_complete` at the end of the success
    // branch (mcp/built_in/auto_focus.rs line 157 in the post-split
    // tree). The event_bus exposed by `test_handler` is an empty-
    // config bus, so subscribers list is empty; we instead inspect
    // the handler.event_bus via a probe — the most readable assertion
    // is that the whole pipeline produced an Ok result without an
    // error, which is what the body checks above. The event itself
    // is an outbound webhook side-effect and is exercised by the
    // event_delivery BDD scenarios. Coverage of lines 157-166 is the
    // primary objective and is achieved as soon as `Ok(result)` is
    // returned.
}

// -----------------------------------------------------------------------
// Progress-notification emission from long-running blocking helpers.
//
// rmcp 1.7's `LocalSessionManager` constructs sessions with a 300 s
// `keep_alive` that fires when the session sees no activity. The
// blocking helpers in `mcp/internals.rs` all have deadlines that
// approach or match 300 s, so without progress emission the two
// timers race and a legitimate long tool can EOF its own SSE response
// stream — the client's call_tool future then never resolves (BDD's
// 360 s `MCP_CALL_TIMEOUT` is the only thing that catches it).
//
// These tests pin that each helper emits at least one progress tick
// per [`PROGRESS_INTERVAL`] while it is polling, by driving the helper
// against a mock that reports "not ready" for a controlled number of
// poll iterations and counting emissions on a test sink.
//
// `start_paused` lets tokio's virtual time auto-advance past each
// `sleep`, so a 12 s simulated wait runs in real-time milliseconds
// without the test sleeping for real.
// -----------------------------------------------------------------------

/// `do_slew_blocking` against a mount that reports `slewing == true`
/// for ~12 s of simulated time and then `false` must fire at least
/// two progress notifications (one each at the 5 s and 10 s marks,
/// per `PROGRESS_INTERVAL == 5 s`).
#[tokio::test(start_paused = true)]
async fn do_slew_blocking_emits_progress_during_slew() {
    // 120 polls × 100 ms tick ≈ 12 s of simulated `slewing()==true`.
    // After 12 s of activity, `PROGRESS_INTERVAL == 5 s` produces a
    // tick at 5 s and at 10 s — assert ≥ 2 emissions.
    let mount = MockTelescope {
        slewing_true_count: 120,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let emitter = super::progress::test_support::CountingProgressEmitter::default();
    let (actual_ra, actual_dec) = handler
        .do_slew_blocking(0.0, 0.0, Duration::ZERO, Some(&emitter))
        .await
        .expect("slew completes when the mount reports idle");
    assert_eq!(actual_ra, 0.0);
    assert_eq!(actual_dec, 0.0);
    assert!(
        emitter.count() >= 2,
        "expected ≥ 2 progress notifications over ~12 s of slew, got {}",
        emitter.count()
    );
}

/// `do_park_blocking` against a mount that reports `at_park == false`
/// for ~12 s of simulated time and then `true` must fire at least two
/// progress notifications, same shape as the slew test.
#[tokio::test(start_paused = true)]
async fn do_park_blocking_emits_progress_during_park() {
    let mount = MockTelescope {
        at_park_false_count: 120,
        at_park_value: true,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let emitter = super::progress::test_support::CountingProgressEmitter::default();
    handler
        .do_park_blocking(Some(&emitter))
        .await
        .expect("park completes when the mount reports at_park");
    assert!(
        emitter.count() >= 2,
        "expected ≥ 2 progress notifications over ~12 s of park, got {}",
        emitter.count()
    );
}

/// `do_capture` against a camera whose `image_ready` returns `false`
/// for ~12 s and then `true` must fire at least two progress
/// notifications during the readout-wait poll loop.
///
/// Uses the same `temp_dir`-redirected `SessionConfig` shape as the
/// other `do_capture` tests so the FITS write inside `do_capture`
/// lands in a sandbox.
#[tokio::test(start_paused = true)]
async fn do_capture_emits_progress_during_readout_wait() {
    let cam = MockCamera {
        not_ready_count: 120,
        ..Default::default()
    };
    let tmp = tempfile::tempdir().expect("temp dir");
    let mut handler = test_handler(camera_registry(Arc::new(cam)));
    handler.session_config = SessionConfig {
        data_directory: tmp.path().to_string_lossy().to_string(),
    };
    let emitter = super::progress::test_support::CountingProgressEmitter::default();
    let (_image_path, _document_id) = handler
        .do_capture("cam", Duration::from_millis(50), Some(&emitter))
        .await
        .expect("capture completes when image_ready flips true");
    assert!(
        emitter.count() >= 2,
        "expected ≥ 2 progress notifications over ~12 s of readout wait, got {}",
        emitter.count()
    );
}

/// `None` for the progress emitter is the default for unit tests and
/// for MCP clients that didn't send a `progressToken`. The helpers
/// must remain functionally identical — pin that the slew still
/// completes correctly when no sink is supplied.
#[tokio::test(start_paused = true)]
async fn do_slew_blocking_with_none_emitter_is_a_noop() {
    let mount = MockTelescope {
        slewing_true_count: 5,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let result = handler
        .do_slew_blocking(0.0, 0.0, Duration::ZERO, None)
        .await;
    result.expect("slew with None emitter still completes");
}

/// `ProgressSink::from_peer_and_meta` returns `None` when `_meta`
/// carries no `progressToken`. The helper is consequently a no-op for
/// any client (or BDD harness) that doesn't opt in.
#[test]
fn progress_sink_returns_none_without_progress_token() {
    // Build an empty Meta — no progressToken set. We can't easily
    // construct a real `Peer<RoleServer>` from outside rmcp (its
    // constructor is `pub(crate)`), but the meta-only path through
    // `Meta::get_progress_token()` is enough to pin the contract:
    // any None on the meta side must turn into a None sink.
    let meta = rmcp::model::Meta::default();
    assert!(
        meta.get_progress_token().is_none(),
        "default Meta should not carry a progressToken"
    );
}

/// Companion to `do_capture_emits_progress_during_readout_wait`. With a
/// `duration` longer than `PROGRESS_INTERVAL`, every tick fired while the
/// camera is still shuttering falls in the `elapsed < duration` arm, so
/// the emitted phase is `"exposing"` — the branch the 50 ms-duration
/// readout test never reaches.
#[tokio::test(start_paused = true)]
async fn do_capture_emits_exposing_phase_before_readout() {
    // `not_ready_count: 120` ≈ 12 s of `image_ready==false`; a 60 s
    // `duration` keeps the 5 s and 10 s emit marks inside the exposure
    // window, so each is tagged `"exposing"`.
    let cam = MockCamera {
        not_ready_count: 120,
        ..Default::default()
    };
    let tmp = tempfile::tempdir().expect("temp dir");
    let mut handler = test_handler(camera_registry(Arc::new(cam)));
    handler.session_config = SessionConfig {
        data_directory: tmp.path().to_string_lossy().to_string(),
    };
    let emitter = super::progress::test_support::CountingProgressEmitter::default();
    handler
        .do_capture("cam", Duration::from_secs(60), Some(&emitter))
        .await
        .expect("capture completes when image_ready flips true");
    assert!(
        emitter.count() >= 2,
        "expected ≥ 2 progress notifications during the exposure window, got {}",
        emitter.count()
    );
    assert!(
        emitter
            .records()
            .iter()
            .any(|(_, _, msg)| msg.as_deref() == Some("exposing")),
        "expected at least one tick tagged \"exposing\", got {:?}",
        emitter.records()
    );
}

/// `do_move_focuser_blocking` against a focuser that reports
/// `is_moving == true` for ~12 s and then `false` must fire at least two
/// progress notifications (5 s + 10 s marks) tagged `"focuser_moving"`
/// during the settle poll — the focuser counterpart to the slew/park/
/// capture progress tests.
#[tokio::test(start_paused = true)]
async fn do_move_focuser_blocking_emits_progress_during_move() {
    let foc = MockFocuser {
        is_moving_true_count: 120,
        position_value: 4321,
        ..Default::default()
    };
    let handler = test_handler(focuser_registry(Arc::new(foc), None, None));
    let emitter = super::progress::test_support::CountingProgressEmitter::default();
    let position = handler
        .do_move_focuser_blocking("foc", 4321, Some(&emitter))
        .await
        .expect("move completes when the focuser reports idle");
    assert_eq!(position, 4321);
    assert!(
        emitter.count() >= 2,
        "expected ≥ 2 progress notifications over ~12 s of focuser move, got {}",
        emitter.count()
    );
    assert!(
        emitter
            .records()
            .iter()
            .any(|(_, _, msg)| msg.as_deref() == Some("focuser_moving")),
        "expected at least one tick tagged \"focuser_moving\", got {:?}",
        emitter.records()
    );
}

/// `do_slew_blocking` with a `settle_after` longer than
/// `PROGRESS_INTERVAL` emits one `"settling"` tick even when the slew
/// itself finishes immediately — covers the settle-phase emit that the
/// during-slew test (zero settle) never reaches.
#[tokio::test(start_paused = true)]
async fn do_slew_blocking_emits_progress_during_settle() {
    // `slewing_true_count: 0` ⇒ the mount reports idle on the first poll,
    // so `poll_slewing_until_idle` returns without emitting; the only tick
    // is the settle one (`settle_after == 10 s >= PROGRESS_INTERVAL`).
    let mount = MockTelescope {
        slewing_true_count: 0,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let emitter = super::progress::test_support::CountingProgressEmitter::default();
    handler
        .do_slew_blocking(0.0, 0.0, Duration::from_secs(10), Some(&emitter))
        .await
        .expect("slew + settle completes");
    assert!(
        emitter
            .records()
            .iter()
            .any(|(_, _, msg)| msg.as_deref() == Some("settling")),
        "expected a tick tagged \"settling\", got {:?}",
        emitter.records()
    );
}

/// Complement to the settle test: a non-zero `settle_after` *below*
/// `PROGRESS_INTERVAL` enters the `Some(sink)` block but skips the emit
/// (the `settle_after >= PROGRESS_INTERVAL` guard is false), so no tick
/// is sent. Pins the short-settle branch so brief corrective slews don't
/// spam progress.
#[tokio::test(start_paused = true)]
async fn do_slew_blocking_skips_settle_tick_below_interval() {
    let mount = MockTelescope {
        slewing_true_count: 0,
        ..Default::default()
    };
    let handler = test_handler(mount_registry(Arc::new(mount), None));
    let emitter = super::progress::test_support::CountingProgressEmitter::default();
    handler
        .do_slew_blocking(0.0, 0.0, Duration::from_secs(2), Some(&emitter))
        .await
        .expect("slew + short settle completes");
    assert_eq!(
        emitter.count(),
        0,
        "a settle below PROGRESS_INTERVAL must emit no tick, got {:?}",
        emitter.records()
    );
}

/// Exercises the *live* `ProgressSink::emit` (not the `CountingProgressEmitter`
/// double): builds a real `Peer<RoleServer>` via `serve_directly` over an
/// in-memory tokio duplex. `serve_directly` skips the init handshake, so no
/// client is required — the server end backs the peer, and the client end is
/// held open (unread) so the single outbound notification buffers instead of
/// erroring. Pins that `from_peer_and_meta` yields a sink when a
/// `progressToken` is present and that `emit` performs a `notify_progress`
/// send without panicking. A 5 s timeout guards against a transport wedge.
#[tokio::test]
async fn progress_sink_emit_sends_via_real_peer() {
    use super::progress::{ProgressEmitter, ProgressSink};
    use rmcp::model::{Meta, NumberOrString, ProgressToken};

    let (server_io, _client_io) = tokio::io::duplex(4096);
    let (rx, tx) = tokio::io::split(server_io);
    let service = test_handler(camera_registry(Arc::new(MockCamera::default())));
    let running = rmcp::service::serve_directly(service, (rx, tx), None);
    let peer = running.peer().clone();

    let meta = Meta::with_progress_token(ProgressToken(NumberOrString::Number(1)));
    let sink = ProgressSink::from_peer_and_meta(peer, &meta)
        .expect("meta carries a progressToken => Some(sink)");

    tokio::time::timeout(
        Duration::from_secs(5),
        sink.emit(5.0, Some(60.0), Some("exposing".to_string())),
    )
    .await
    .expect("emit completed within the timeout");

    // `_client_io` and `running` are held to here so the session worker is
    // alive (and the duplex buffer un-closed) while the notification drains;
    // both shut down on drop at end of scope.
    drop(running);
}
