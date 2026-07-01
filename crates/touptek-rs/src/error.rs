//! Error types for the safe `touptek-rs` API.
//!
//! The ToupTek SDK reports results as Windows-style `HRESULT` codes: a value
//! `>= 0` is **success** (`S_OK` = 0, and `S_FALSE` = 1 is *also* success — "no
//! operation needed"), and a negative value is a failure code. We map the
//! documented `E_*` failure codes to typed errors **by numeric value** (fixed by
//! the vendored header) rather than by the generated `bindgen` constant names, so
//! the mapping is stable across bindgen versions.

use thiserror::Error;

/// Result alias for the safe API.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors returned by the safe `touptek-rs` API.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum Error {
    /// A ToupTek SDK call returned a failure `HRESULT`.
    #[error("ToupTek SDK error: {0}")]
    Sdk(#[from] SdkError),
    /// No device at the requested index/id, or the handle could not be opened
    /// (`Toupcam_Open*` returned null).
    #[error("ToupTek device not found or could not be opened")]
    DeviceNotFound,
}

/// ToupTek `HRESULT` failure codes (the negative `E_*` values), mapped from the
/// raw `int`.
///
/// Success codes (`>= 0`, i.e. `S_OK` / `S_FALSE`) are handled by [`hr_check`]
/// and are **not** represented here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum SdkError {
    /// `E_UNEXPECTED` (`0x8000ffff`) — conditions not met (e.g. setting an option
    /// that cannot change while the camera is running).
    #[error("unexpected failure (conditions not met)")]
    Unexpected,
    /// `E_NOTIMPL` (`0x80004001`) — not supported on this model.
    #[error("not supported / not implemented on this model")]
    NotImplemented,
    /// `E_NOINTERFACE` (`0x80004002`).
    #[error("no such interface")]
    NoInterface,
    /// `E_ACCESSDENIED` (`0x80070005`) — often a missing udev rule / privileges.
    #[error("permission denied (check udev rules / privileges)")]
    AccessDenied,
    /// `E_OUTOFMEMORY` (`0x8007000e`).
    #[error("out of memory")]
    OutOfMemory,
    /// `E_INVALIDARG` (`0x80070057`) — one or more arguments are not valid.
    #[error("invalid argument")]
    InvalidArg,
    /// `E_POINTER` (`0x80004003`) — a required pointer was null.
    #[error("null pointer")]
    Pointer,
    /// `E_FAIL` (`0x80004005`) — generic failure.
    #[error("generic failure")]
    Fail,
    /// `E_WRONG_THREAD` (`0x8001010e`) — called from the wrong thread.
    #[error("called from the wrong thread")]
    WrongThread,
    /// `E_GEN_FAILURE` (`0x8007001f`) — device not functioning (hardware/USB).
    #[error("device not functioning (hardware/USB error)")]
    DeviceFailure,
    /// `E_BUSY` (`0x800700aa`) — the camera is already in use.
    #[error("device busy / already in use")]
    Busy,
    /// `E_PENDING` (`0x8000000a`) — no data is available yet.
    #[error("no data available yet")]
    Pending,
    /// `E_TIMEOUT` (`0x8001011f`).
    #[error("operation timed out")]
    Timeout,
    /// `E_UNREACH` (`0x80072743`) — network unreachable (Wi-Fi/GigE models).
    #[error("network unreachable (check camera IP/firewall)")]
    NetworkUnreachable,
    /// `E_CANCELLED` (`0x800704c7`) — cancelled by the user.
    #[error("operation cancelled")]
    Cancelled,
    /// A failure `HRESULT` outside the set documented by the vendored header.
    #[error("unknown HRESULT {0:#010x}")]
    Unknown(i32),
}

impl SdkError {
    /// Map a raw negative `HRESULT` to a typed error.
    ///
    /// Non-negative codes are success and should be routed through [`hr_check`]
    /// rather than here; a non-negative input maps to [`SdkError::Unknown`].
    #[must_use]
    pub fn from_hresult(hr: i32) -> Self {
        // Compare on the unsigned bit pattern: the `E_*` codes have the high bit
        // set, so they are negative as `i32`.
        match hr as u32 {
            0x8000_ffff => Self::Unexpected,
            0x8000_4001 => Self::NotImplemented,
            0x8000_4002 => Self::NoInterface,
            0x8007_0005 => Self::AccessDenied,
            0x8007_000e => Self::OutOfMemory,
            0x8007_0057 => Self::InvalidArg,
            0x8000_4003 => Self::Pointer,
            0x8000_4005 => Self::Fail,
            0x8001_010e => Self::WrongThread,
            0x8007_001f => Self::DeviceFailure,
            0x8007_00aa => Self::Busy,
            0x8000_000a => Self::Pending,
            0x8001_011f => Self::Timeout,
            0x8007_2743 => Self::NetworkUnreachable,
            0x8007_04c7 => Self::Cancelled,
            _ => Self::Unknown(hr),
        }
    }
}

/// Convert a raw ToupTek `HRESULT` into `Result<()>`.
///
/// Success is `hr >= 0` — both `S_OK` (0) and the `S_FALSE` (1) "no-op" success.
///
/// # Errors
/// Returns [`Error::Sdk`] for any negative `HRESULT`.
pub fn hr_check(hr: i32) -> Result<()> {
    if hr >= 0 {
        Ok(())
    } else {
        Err(Error::Sdk(SdkError::from_hresult(hr)))
    }
}
