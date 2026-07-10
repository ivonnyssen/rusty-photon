//! EAF focuser enumeration and device handle.
//!
//! [`Sdk::focusers`] lists every connected EAF's [`FocuserInfo`].
//! [`Sdk::open_focuser`] opens one and returns a [`Focuser`] RAII handle that
//! closes the device on drop. Unlike the EFW filter wheel, `EAFGetPosition` has
//! no moving sentinel — it always returns the live step count — so moving state
//! is read via the dedicated `EAFIsMoving` call instead. With the `simulation`
//! feature a single fabricated `EAF-Simulated` focuser is presented and the SDK
//! is never called.
//!
//! Note: like EFW, EAF status codes (`EAF_ERROR_CODE`) are a signed `c_int` (the
//! header's `EAF_ERROR_END = -1` makes the enum signed), so the return value is
//! fed to [`crate::eaf_check`] directly, with no `as i32` cast.

#[cfg(not(feature = "simulation"))]
use crate::ffi_util::{c_string_field, hex8};
#[cfg(not(feature = "simulation"))]
use crate::{eaf_check, sys};
#[cfg(not(feature = "simulation"))]
use std::os::raw::c_int;

use crate::{EafError, Error, Result, Sdk};

/// Safe view of `EAF_INFO`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocuserInfo {
    /// `ID` — the handle used by all per-focuser SDK calls.
    pub id: i32,
    /// Model name, e.g. `"EAF"`.
    pub name: String,
    /// Fixed maximum position (`EAF_INFO::MaxStep`).
    pub max_step: u32,
}

/// An open EAF focuser. Closes the device on drop.
///
/// As with [`crate::Camera`] and [`crate::FilterWheel`], the SDK is not safe for
/// concurrent calls on a single handle, so `Focuser` is `Send` but **not**
/// `Sync` — share it across threads behind a `Mutex`.
#[derive(Debug)]
pub struct Focuser {
    info: FocuserInfo,
    #[cfg(feature = "simulation")]
    state: std::sync::Mutex<SimFocuserState>,
    /// Makes `Focuser` `!Sync` (see the type docs) while leaving it `Send`.
    _not_sync: std::marker::PhantomData<std::cell::Cell<()>>,
}

impl Sdk {
    /// Enumerate every connected EAF's [`FocuserInfo`].
    ///
    /// # Errors
    /// Returns [`Error::Eaf`] if the SDK fails to read a focuser's id or
    /// property.
    pub fn focusers(&self) -> Result<Vec<FocuserInfo>> {
        #[cfg(feature = "simulation")]
        let infos = (0..crate::SIM_FOCUSER_COUNT)
            .map(|_| sim_focuser_info())
            .collect();
        #[cfg(not(feature = "simulation"))]
        let infos = {
            let n = self.focuser_count()?;
            (0..n)
                .map(|index| {
                    let idx =
                        i32::try_from(index).map_err(|_| Error::Eaf(EafError::InvalidIndex))?;
                    let id = read_focuser_id(idx)?;
                    read_focuser_property(id)
                })
                .collect::<Result<Vec<_>>>()?
        };
        Ok(infos)
    }

    /// Open the EAF at enumeration `index`.
    ///
    /// On the real path this calls `EAFGetID` + `EAFOpen` + `EAFGetProperty` (so
    /// the returned info carries the real `MaxStep`); the [`Focuser`] closes the
    /// device on drop.
    ///
    /// # Errors
    /// Returns [`Error::Eaf`] if the index is out of range or the SDK fails to
    /// open the focuser.
    pub fn open_focuser(&self, index: usize) -> Result<Focuser> {
        #[cfg(feature = "simulation")]
        let focuser = {
            if index >= crate::SIM_FOCUSER_COUNT {
                return Err(Error::Eaf(EafError::InvalidIndex));
            }
            Focuser {
                info: sim_focuser_info(),
                state: std::sync::Mutex::new(SimFocuserState::default()),
                _not_sync: std::marker::PhantomData,
            }
        };
        #[cfg(not(feature = "simulation"))]
        let focuser = {
            let idx = i32::try_from(index).map_err(|_| Error::Eaf(EafError::InvalidIndex))?;
            let id = read_focuser_id(idx)?;
            // SAFETY: `id` is a valid focuser id from enumeration; open it.
            eaf_check(unsafe { sys::EAFOpen(id) })?;
            // Read the property after opening so MaxStep is populated. On
            // failure, close the focuser so the open handle is not leaked.
            let info = match read_focuser_property(id) {
                Ok(info) => info,
                Err(e) => {
                    // SAFETY: the focuser was just opened; close it again.
                    unsafe {
                        let _ = sys::EAFClose(id);
                    }
                    return Err(e);
                }
            };
            Focuser {
                info,
                _not_sync: std::marker::PhantomData,
            }
        };
        Ok(focuser)
    }
}

impl Focuser {
    /// The focuser's cached [`FocuserInfo`].
    #[must_use]
    pub fn info(&self) -> &FocuserInfo {
        &self.info
    }

    /// The focuser's `ID`.
    #[must_use]
    pub fn id(&self) -> i32 {
        self.info.id
    }

    /// Fixed maximum position (`EAF_INFO::MaxStep`).
    #[must_use]
    pub fn max_step(&self) -> u32 {
        self.info.max_step
    }

    /// Current step position. Unlike [`crate::FilterWheel::position`], the EAF
    /// has no moving sentinel — this always returns the live/target step count,
    /// whether or not the focuser is currently moving (see [`Self::is_moving`]).
    ///
    /// # Errors
    /// Returns [`Error::Eaf`] if the SDK call fails.
    pub fn position(&self) -> Result<i32> {
        #[cfg(feature = "simulation")]
        let pos = self.sim_position();
        #[cfg(not(feature = "simulation"))]
        let pos = {
            let mut p: c_int = 0;
            // SAFETY: open focuser id; the SDK writes the current step count.
            eaf_check(unsafe { sys::EAFGetPosition(self.info.id, &mut p) })?;
            p
        };
        Ok(pos)
    }

    /// Whether the focuser is currently moving (`EAFIsMoving`).
    ///
    /// The SDK's `pbHandControl` out-parameter (whether the physical hand-paddle
    /// is driving the move) is read but discarded: no ASCOM `Focuser` property
    /// maps to it (see `docs/services/zwo-focuser.md` "MVP scope").
    ///
    /// # Errors
    /// Returns [`Error::Eaf`] if the SDK call fails.
    pub fn is_moving(&self) -> Result<bool> {
        #[cfg(feature = "simulation")]
        let moving = self.sim_is_moving();
        #[cfg(not(feature = "simulation"))]
        let moving = {
            let mut moving = false;
            let mut hand_control = false;
            // SAFETY: open focuser id; the SDK writes both out-parameters.
            eaf_check(unsafe { sys::EAFIsMoving(self.info.id, &mut moving, &mut hand_control) })?;
            moving
        };
        Ok(moving)
    }

    /// Move to absolute `position` (0-based step count, `0..=max_step`).
    ///
    /// The SDK itself does not document a bounds check for `EAFMove`; range
    /// validation against `max_step` is the ASCOM device's responsibility (see
    /// `docs/services/zwo-focuser.md` "ASCOM Focuser Mapping"). The `simulation`
    /// backend enforces the same bound for test fidelity.
    ///
    /// # Errors
    /// Returns [`Error::Eaf`] if the focuser is already moving or the SDK call
    /// fails.
    pub fn move_to(&self, position: i32) -> Result<()> {
        #[cfg(feature = "simulation")]
        self.sim_move_to(position)?;
        #[cfg(not(feature = "simulation"))]
        {
            // SAFETY: open focuser id; the SDK validates/starts the move.
            eaf_check(unsafe { sys::EAFMove(self.info.id, position) })?;
        }
        Ok(())
    }

    /// Stop an in-progress move (`EAFStop`). A no-op on an idle focuser.
    ///
    /// # Errors
    /// Returns [`Error::Eaf`] if the SDK call fails.
    pub fn stop(&self) -> Result<()> {
        #[cfg(feature = "simulation")]
        self.sim_stop();
        #[cfg(not(feature = "simulation"))]
        // SAFETY: open focuser id; stops any in-progress move.
        eaf_check(unsafe { sys::EAFStop(self.info.id) })?;
        Ok(())
    }

    /// The temperature sensor reading in degrees Celsius (`EAFGetTemp`).
    ///
    /// # Errors
    /// Returns [`Error::Eaf`] if the SDK call fails (e.g. the value is
    /// currently unusable — see the header's `EAFGetTemp` docs).
    pub fn temperature(&self) -> Result<f32> {
        #[cfg(feature = "simulation")]
        let temp = self.sim_temperature();
        #[cfg(not(feature = "simulation"))]
        let temp = {
            let mut t: f32 = 0.0;
            // SAFETY: open focuser id; the SDK writes the temperature reading.
            eaf_check(unsafe { sys::EAFGetTemp(self.info.id, &mut t) })?;
            t
        };
        Ok(temp)
    }

    /// Whether the focuser moves along the reverse direction.
    ///
    /// # Errors
    /// Returns [`Error::Eaf`] if the SDK call fails.
    pub fn reverse(&self) -> Result<bool> {
        #[cfg(feature = "simulation")]
        let reverse = self.sim_reverse();
        #[cfg(not(feature = "simulation"))]
        let reverse = {
            let mut r = false;
            // SAFETY: open focuser id; the SDK writes the direction flag.
            eaf_check(unsafe { sys::EAFGetReverse(self.info.id, &mut r) })?;
            r
        };
        Ok(reverse)
    }

    /// Set whether the focuser moves along the reverse direction.
    ///
    /// # Errors
    /// Returns [`Error::Eaf`] if the SDK call fails.
    pub fn set_reverse(&self, reverse: bool) -> Result<()> {
        #[cfg(feature = "simulation")]
        self.sim_set_reverse(reverse);
        #[cfg(not(feature = "simulation"))]
        // SAFETY: open focuser id; sets the moving direction.
        eaf_check(unsafe { sys::EAFSetReverse(self.info.id, reverse) })?;
        Ok(())
    }

    /// Set the current position without moving (`EAFResetPostion`, the header's
    /// own spelling). Kept for parity with `EFW`'s `calibrate`; not called by
    /// the ASCOM device in v0.
    ///
    /// # Errors
    /// Returns [`Error::Eaf`] if the SDK call fails.
    pub fn reset_position(&self, position: i32) -> Result<()> {
        #[cfg(feature = "simulation")]
        self.sim_reset_position(position);
        #[cfg(not(feature = "simulation"))]
        // SAFETY: open focuser id; sets the position counter without moving.
        eaf_check(unsafe { sys::EAFResetPostion(self.info.id, position) })?;
        Ok(())
    }

    /// The focuser's serial number as a 16-character hex string.
    ///
    /// # Errors
    /// Returns [`Error::Eaf`] if the firmware does not report a serial number
    /// (`EAF_ERROR_NOT_SUPPORTED` on older firmware).
    pub fn serial(&self) -> Result<String> {
        #[cfg(feature = "simulation")]
        let serial = SIM_EAF_SERIAL.to_owned();
        #[cfg(not(feature = "simulation"))]
        let serial = {
            // SAFETY: `EAF_SN` is a POD `[u8; 8]`; the SDK fills it on success.
            let mut sn: sys::EAF_SN = unsafe { std::mem::zeroed() };
            eaf_check(unsafe { sys::EAFGetSerialNumber(self.info.id, &mut sn) })?;
            hex8(&sn.id)
        };
        Ok(serial)
    }

    /// The focuser firmware version as `(major, minor, build)`.
    ///
    /// # Errors
    /// Returns [`Error::Eaf`] if the SDK call fails.
    pub fn firmware_version(&self) -> Result<(u8, u8, u8)> {
        #[cfg(feature = "simulation")]
        let version = (1, 0, 0);
        #[cfg(not(feature = "simulation"))]
        let version = {
            let mut major: u8 = 0;
            let mut minor: u8 = 0;
            let mut build: u8 = 0;
            // SAFETY: open focuser id; the SDK writes the three version bytes.
            eaf_check(unsafe {
                sys::EAFGetFirmwareVersion(self.info.id, &mut major, &mut minor, &mut build)
            })?;
            (major, minor, build)
        };
        Ok(version)
    }
}

#[cfg(not(feature = "simulation"))]
impl Drop for Focuser {
    fn drop(&mut self) {
        // SAFETY: closing an open focuser by id; `EAFClose` is safe to call once
        // on an open handle.
        unsafe {
            let _ = sys::EAFClose(self.info.id);
        }
    }
}

// ---- real FFI helpers --------------------------------------------------------

#[cfg(not(feature = "simulation"))]
fn read_focuser_id(index: i32) -> Result<i32> {
    let mut id: c_int = 0;
    // SAFETY: the SDK writes the focuser id for a valid index.
    eaf_check(unsafe { sys::EAFGetID(index, &mut id) })?;
    Ok(id)
}

#[cfg(not(feature = "simulation"))]
fn read_focuser_property(id: i32) -> Result<FocuserInfo> {
    // SAFETY: `EAF_INFO` is POD; the SDK fills it for a valid id.
    let mut raw: sys::EAF_INFO = unsafe { std::mem::zeroed() };
    eaf_check(unsafe { sys::EAFGetProperty(id, &mut raw) })?;
    Ok(FocuserInfo {
        id: raw.ID,
        name: c_string_field(&raw.Name),
        max_step: u32::try_from(raw.MaxStep).unwrap_or(0),
    })
}

// ---- simulation backend ------------------------------------------------------

#[cfg(feature = "simulation")]
const SIM_EAF_SERIAL: &str = "2a3b4c5d6e7f8091";

/// The fabricated simulated focuser: a fixed `MaxStep` of 7000 — illustrative
/// only, not any particular real EAF unit's actual value.
#[cfg(feature = "simulation")]
fn sim_focuser_info() -> FocuserInfo {
    FocuserInfo {
        id: 0,
        name: "EAF-Simulated".to_owned(),
        max_step: 7000,
    }
}

/// Mutable state for the simulated focuser, behind a `Mutex` so the `&self`
/// device methods can update it.
#[cfg(feature = "simulation")]
#[derive(Debug)]
struct SimFocuserState {
    position: i32,
    moving: bool,
    reverse: bool,
    temperature: f32,
}

#[cfg(feature = "simulation")]
impl Default for SimFocuserState {
    fn default() -> Self {
        Self {
            position: 0,
            moving: false,
            reverse: false,
            temperature: 20.0,
        }
    }
}

#[cfg(feature = "simulation")]
impl Focuser {
    fn sim_position(&self) -> i32 {
        // Unlike EFW's sentinel-carrying position, EAFGetPosition always
        // returns the live/target value — the simulated move jumps straight to
        // the target, so this is simply the cached position (see
        // `docs/services/zwo-focuser.md`'s note on where "settle after one
        // poll" belongs for the EAF).
        self.state.lock().unwrap().position
    }

    fn sim_is_moving(&self) -> bool {
        let mut st = self.state.lock().unwrap();
        if st.moving {
            // A simulated move settles one poll after it is requested.
            st.moving = false;
            true
        } else {
            false
        }
    }

    fn sim_move_to(&self, position: i32) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        // A focuser already in motion rejects a new move, as the hardware does.
        if st.moving {
            return Err(Error::Eaf(EafError::Moving));
        }
        if position < 0 || u32::try_from(position).unwrap_or(u32::MAX) > self.info.max_step {
            return Err(Error::Eaf(EafError::InvalidValue));
        }
        st.position = position;
        st.moving = true;
        Ok(())
    }

    fn sim_stop(&self) {
        self.state.lock().unwrap().moving = false;
    }

    fn sim_temperature(&self) -> f32 {
        self.state.lock().unwrap().temperature
    }

    fn sim_reverse(&self) -> bool {
        self.state.lock().unwrap().reverse
    }

    fn sim_set_reverse(&self, reverse: bool) {
        self.state.lock().unwrap().reverse = reverse;
    }

    fn sim_reset_position(&self, position: i32) {
        let mut st = self.state.lock().unwrap();
        st.position = position;
        st.moving = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focuser_is_send() {
        // Same threading contract as `Camera`/`FilterWheel`: `Send` but not `Sync`.
        fn assert_send<T: Send>() {}
        assert_send::<Focuser>();
    }

    #[test]
    fn focusers_enumerates() {
        let sdk = Sdk::new().unwrap();
        let focusers = sdk.focusers().unwrap();
        #[cfg(feature = "simulation")]
        {
            assert_eq!(focusers.len(), crate::SIM_FOCUSER_COUNT);
            assert_eq!(focusers[0].name, "EAF-Simulated");
            assert_eq!(focusers[0].max_step, 7000);
        }
        // Without the feature this calls the real SDK; with no hardware the
        // list is empty, but the call must still succeed.
        #[cfg(not(feature = "simulation"))]
        {
            let _ = focusers;
        }
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn open_exposes_info_serial_and_firmware() {
        let sdk = Sdk::new().unwrap();
        let focuser = sdk.open_focuser(0).unwrap();
        assert_eq!(focuser.id(), 0);
        assert_eq!(focuser.max_step(), 7000);
        assert_eq!(focuser.info().name, "EAF-Simulated");
        let serial = focuser.serial().unwrap();
        assert_eq!(serial.len(), 16);
        assert!(serial.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(focuser.firmware_version().unwrap(), (1, 0, 0));
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn open_out_of_range_is_rejected() {
        let sdk = Sdk::new().unwrap();
        assert_eq!(
            sdk.open_focuser(9).unwrap_err(),
            Error::Eaf(EafError::InvalidIndex)
        );
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn move_reports_moving_then_settles() {
        let sdk = Sdk::new().unwrap();
        let focuser = sdk.open_focuser(0).unwrap();
        assert_eq!(focuser.position().unwrap(), 0);
        focuser.move_to(3000).unwrap();
        // The simulated focuser reports moving once, then settles; position is
        // already live (no sentinel), so it reflects the target immediately.
        assert_eq!(focuser.position().unwrap(), 3000);
        assert!(focuser.is_moving().unwrap());
        assert!(!focuser.is_moving().unwrap());
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn move_out_of_range_is_rejected() {
        let sdk = Sdk::new().unwrap();
        let focuser = sdk.open_focuser(0).unwrap();
        assert_eq!(
            focuser.move_to(-1).unwrap_err(),
            Error::Eaf(EafError::InvalidValue)
        );
        assert_eq!(
            focuser.move_to(7001).unwrap_err(),
            Error::Eaf(EafError::InvalidValue)
        );
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn move_while_moving_is_rejected() {
        let sdk = Sdk::new().unwrap();
        let focuser = sdk.open_focuser(0).unwrap();
        focuser.move_to(100).unwrap();
        assert_eq!(
            focuser.move_to(200).unwrap_err(),
            Error::Eaf(EafError::Moving)
        );
        // After the move settles (one is_moving poll), a new move is accepted.
        assert!(focuser.is_moving().unwrap());
        focuser.move_to(200).unwrap();
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn stop_clears_moving() {
        let sdk = Sdk::new().unwrap();
        let focuser = sdk.open_focuser(0).unwrap();
        focuser.move_to(500).unwrap();
        focuser.stop().unwrap();
        assert!(!focuser.is_moving().unwrap());
        // A halted focuser accepts a new move immediately.
        focuser.move_to(600).unwrap();
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn reverse_round_trips() {
        let sdk = Sdk::new().unwrap();
        let focuser = sdk.open_focuser(0).unwrap();
        assert!(!focuser.reverse().unwrap());
        focuser.set_reverse(true).unwrap();
        assert!(focuser.reverse().unwrap());
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn temperature_returns_a_value() {
        let sdk = Sdk::new().unwrap();
        let focuser = sdk.open_focuser(0).unwrap();
        assert_eq!(focuser.temperature().unwrap(), 20.0);
    }

    #[cfg(feature = "simulation")]
    #[test]
    fn reset_position_sets_without_moving() {
        let sdk = Sdk::new().unwrap();
        let focuser = sdk.open_focuser(0).unwrap();
        focuser.reset_position(42).unwrap();
        assert_eq!(focuser.position().unwrap(), 42);
        assert!(!focuser.is_moving().unwrap());
    }
}
