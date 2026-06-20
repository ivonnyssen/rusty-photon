//! `QhyFilterWheelDevice` — the ASCOM `Device` + `FilterWheel` implementation
//! over the [`FilterWheelHandle`](crate::backend::FilterWheelHandle) seam.
//!
//! Registered (one per discovered CFW) only when `filterwheel.enabled`. `Names`
//! are the configured `filter_names` or generated `Filter0..N`; `Position`
//! returns `None` while the commanded target differs from the actual slot (ASCOM
//! "moving" sentinel); `FocusOffsets` is zero per filter in v0.

use std::sync::Arc;

use ascom_alpaca::api::{Device, FilterWheel};
use ascom_alpaca::{ASCOMError, ASCOMResult};
use parking_lot::Mutex;
use tracing::debug;

use crate::backend::FilterWheelHandle;

#[derive(Debug)]
struct FilterWheelState {
    number_of_filters: Mutex<Option<u32>>,
    target_position: Mutex<Option<u32>>,
}

/// One ASCOM FilterWheel device per discovered CFW.
#[derive(Clone, derive_more::Debug)]
pub struct QhyFilterWheelDevice {
    #[debug(skip)]
    handle: Arc<dyn FilterWheelHandle>,
    unique_id: String,
    name: String,
    /// Human filter names from config (overrides generated `Filter0..N`).
    filter_names: Option<Vec<String>>,
    state: Arc<FilterWheelState>,
}

impl QhyFilterWheelDevice {
    /// Build a CFW device. The ASCOM `UniqueID` is `CFW-<sdk-id>` (prefixed so it
    /// never collides with the camera's UniqueID, which shares the SDK id on
    /// single-handle models). `filter_names` / `name` come from the per-serial
    /// config override.
    pub fn new(
        handle: Arc<dyn FilterWheelHandle>,
        filter_names: Option<Vec<String>>,
        name: Option<String>,
    ) -> Self {
        let id = handle.id();
        let unique_id = format!("CFW-{id}");
        let name = name.unwrap_or_else(|| format!("QHYCCD Filter Wheel {id}"));
        Self {
            handle,
            unique_id,
            name,
            filter_names,
            state: Arc::new(FilterWheelState {
                number_of_filters: Mutex::new(None),
                target_position: Mutex::new(None),
            }),
        }
    }

    fn ensure_connected(&self) -> ASCOMResult<()> {
        match self.handle.is_open() {
            Ok(true) => Ok(()),
            _ => Err(ASCOMError::NOT_CONNECTED),
        }
    }

    fn filter_count(&self) -> ASCOMResult<u32> {
        (*self.state.number_of_filters.lock()).ok_or(ASCOMError::NOT_CONNECTED)
    }

    fn connect(&self) -> ASCOMResult<()> {
        // `handle.open()` is refcounted across the shared physical connection
        // (`backend::SharedCameraConnection`): a QHY CFW is driven through the
        // camera's USB handle, so the Camera and FilterWheel devices on the same
        // SDK id share ONE `OpenQHYCCD`. Opening the wheel just bumps that
        // refcount (physically opening only if it is the first connect).
        self.handle.open().map_err(|_| ASCOMError::NOT_CONNECTED)?;
        // If any step of the post-open handshake fails, close the handle (drop our
        // refcount) before propagating so a failed connect leaves Connected ==
        // false rather than an opened-but-unusable wheel (mirrors the camera).
        if let Err(e) = self.open_handshake() {
            if let Err(close_err) = self.handle.close() {
                debug!(error = %close_err, "close after a failed filter-wheel connect handshake also failed");
            }
            return Err(e);
        }
        Ok(())
    }

    fn open_handshake(&self) -> ASCOMResult<()> {
        let count = self
            .handle
            .get_number_of_filters()
            .map_err(|_| ASCOMError::NOT_CONNECTED)?;
        *self.state.number_of_filters.lock() = Some(count);
        // Initial target = the current physical slot.
        let position = self
            .handle
            .get_position()
            .map_err(|_| ASCOMError::NOT_CONNECTED)?;
        *self.state.target_position.lock() = Some(position);
        debug!(filter_wheel = %self.unique_id, slots = count, "filter wheel connected");
        Ok(())
    }

    fn disconnect(&self) -> ASCOMResult<()> {
        // Refcounted close (`backend::SharedCameraConnection`): the underlying
        // camera is physically closed only when the LAST device sharing this SDK
        // id disconnects. Disconnecting the wheel therefore no longer tears down a
        // concurrently-connected camera — the real-hardware failure mode flagged
        // in review and confirmed before this fix. See docs/services/qhy-camera.md.
        self.handle.close().map_err(|_| ASCOMError::NOT_CONNECTED)
    }
}

#[async_trait::async_trait]
impl Device for QhyFilterWheelDevice {
    fn static_name(&self) -> &str {
        &self.name
    }

    fn unique_id(&self) -> &str {
        &self.unique_id
    }

    async fn connected(&self) -> ASCOMResult<bool> {
        self.handle.is_open().map_err(|_| ASCOMError::NOT_CONNECTED)
    }

    async fn set_connected(&self, connected: bool) -> ASCOMResult<()> {
        let current = self
            .handle
            .is_open()
            .map_err(|_| ASCOMError::NOT_CONNECTED)?;
        if current == connected {
            return Ok(());
        }
        if connected {
            self.connect()
        } else {
            self.disconnect()
        }
    }

    async fn description(&self) -> ASCOMResult<String> {
        Ok("QHYCCD filter wheel".to_string())
    }

    async fn driver_info(&self) -> ASCOMResult<String> {
        Ok("rusty-photon qhy-camera".to_string())
    }

    async fn driver_version(&self) -> ASCOMResult<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }
}

#[async_trait::async_trait]
impl FilterWheel for QhyFilterWheelDevice {
    async fn names(&self) -> ASCOMResult<Vec<String>> {
        self.ensure_connected()?;
        let count = self.filter_count()? as usize;
        // ASCOM requires the `Names` array to have exactly one entry per slot
        // (matching `FocusOffsets` and the `Position` range). The hardware slot
        // count is unknown until connect, so configured `filter_names` cannot be
        // validated at config-load time — normalise here: take the first `count`
        // configured names and pad any remainder with generated `Filter{i}`.
        Ok((0..count)
            .map(|i| {
                self.filter_names
                    .as_ref()
                    .and_then(|names| names.get(i).cloned())
                    .unwrap_or_else(|| format!("Filter{i}"))
            })
            .collect())
    }

    async fn focus_offsets(&self) -> ASCOMResult<Vec<i32>> {
        self.ensure_connected()?;
        let count = self.filter_count()?;
        Ok(vec![0; count as usize])
    }

    async fn position(&self) -> ASCOMResult<Option<usize>> {
        self.ensure_connected()?;
        let target = (*self.state.target_position.lock()).ok_or(ASCOMError::NOT_CONNECTED)?;
        let actual = self
            .handle
            .get_position()
            .map_err(|_| ASCOMError::INVALID_OPERATION)?;
        // `None` is the ASCOM "moving" sentinel: target not yet reached.
        if actual == target {
            Ok(Some(actual as usize))
        } else {
            Ok(None)
        }
    }

    async fn set_position(&self, position: usize) -> ASCOMResult<()> {
        self.ensure_connected()?;
        let count = self.filter_count()?;
        let target = position as u32;
        if target >= count {
            return Err(ASCOMError::invalid_value(format!(
                "filter position {position} out of range (0..{count})"
            )));
        }
        if *self.state.target_position.lock() == Some(target) {
            return Ok(());
        }
        self.handle
            .set_position(target)
            .map_err(|_| ASCOMError::INVALID_OPERATION)?;
        *self.state.target_position.lock() = Some(target);
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::backend::mock::MockFilterWheelHandle;
    use ascom_alpaca::ASCOMErrorCode;
    use std::sync::atomic::Ordering;

    fn connected(filter_names: Option<Vec<String>>) -> QhyFilterWheelDevice {
        let handle = Arc::new(MockFilterWheelHandle::new("SIM-QHY178M", 7));
        let device = QhyFilterWheelDevice::new(handle, filter_names, None);
        device.connect().unwrap();
        device
    }

    #[tokio::test]
    async fn failed_handshake_closes_the_handle() {
        // open() succeeds but the post-open handshake fails: a failed connect
        // must leave the wheel cleanly disconnected, not opened-but-unusable.
        let handle = Arc::new(MockFilterWheelHandle::new("SIM-QHY178M", 7));
        handle.fail_handshake.store(true, Ordering::SeqCst);
        let device = QhyFilterWheelDevice::new(handle.clone(), None, None);

        let err = device.connect().unwrap_err();
        assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
        assert!(
            !handle.is_open().unwrap(),
            "handle must be closed after a failed connect handshake"
        );
    }

    #[tokio::test]
    async fn set_connected_toggles_and_is_idempotent() {
        // Drives `set_connected` (both branches) + `disconnect()` end to end —
        // the connect/disconnect lifecycle the other tests skip by calling
        // `connect()` directly.
        let handle = Arc::new(MockFilterWheelHandle::new("SIM-QHY178M", 7));
        let device = QhyFilterWheelDevice::new(handle, None, None);
        assert!(!device.connected().await.unwrap());

        // connect via set_connected (connect branch + handshake)
        device.set_connected(true).await.unwrap();
        assert!(device.connected().await.unwrap());
        assert_eq!(device.names().await.unwrap().len(), 7);

        // already connected → no-op (the current == connected early return)
        device.set_connected(true).await.unwrap();
        assert!(device.connected().await.unwrap());

        // disconnect via set_connected (disconnect branch)
        device.set_connected(false).await.unwrap();
        assert!(!device.connected().await.unwrap());
        // operations after disconnect report NOT_CONNECTED (ensure_connected)
        assert_eq!(
            device.names().await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );
        assert_eq!(
            device.position().await.unwrap_err().code,
            ASCOMErrorCode::NOT_CONNECTED
        );

        // already disconnected → no-op
        device.set_connected(false).await.unwrap();
        assert!(!device.connected().await.unwrap());
    }

    #[tokio::test]
    async fn generated_names_when_no_config() {
        let device = connected(None);
        let names = device.names().await.unwrap();
        assert_eq!(names.len(), 7);
        assert_eq!(names[0], "Filter0");
        assert_eq!(names[6], "Filter6");
    }

    #[tokio::test]
    async fn custom_names_from_config() {
        let custom = vec!["L", "R", "G", "B", "Ha", "OIII", "SII"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        let device = connected(Some(custom.clone()));
        assert_eq!(device.names().await.unwrap(), custom);
    }

    #[tokio::test]
    async fn too_few_config_names_are_padded_to_slot_count() {
        let device = connected(Some(vec!["L".into(), "R".into(), "G".into()]));
        let names = device.names().await.unwrap();
        assert_eq!(names.len(), 7, "Names must have one entry per slot");
        assert_eq!(names[0], "L");
        assert_eq!(names[2], "G");
        assert_eq!(names[3], "Filter3");
        assert_eq!(names[6], "Filter6");
    }

    #[tokio::test]
    async fn too_many_config_names_are_truncated_to_slot_count() {
        let nine = (0..9).map(|i| format!("F{i}")).collect::<Vec<_>>();
        let device = connected(Some(nine));
        let names = device.names().await.unwrap();
        assert_eq!(names.len(), 7, "Names must have one entry per slot");
        assert_eq!(names[0], "F0");
        assert_eq!(names[6], "F6");
    }

    #[tokio::test]
    async fn moving_to_a_valid_slot_updates_position() {
        let device = connected(None);
        device.set_position(3).await.unwrap();
        assert_eq!(device.position().await.unwrap(), Some(3));
    }

    #[tokio::test]
    async fn out_of_range_slot_is_rejected() {
        let device = connected(None);
        assert_eq!(
            device.set_position(7).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_VALUE
        );
        assert_eq!(
            device.set_position(99).await.unwrap_err().code,
            ASCOMErrorCode::INVALID_VALUE
        );
    }

    #[tokio::test]
    async fn focus_offsets_are_zero_per_filter() {
        let device = connected(None);
        assert_eq!(device.focus_offsets().await.unwrap(), vec![0; 7]);
    }

    #[tokio::test]
    async fn unique_id_is_prefixed() {
        let device = connected(None);
        assert_eq!(device.unique_id(), "CFW-SIM-QHY178M");
    }
}
