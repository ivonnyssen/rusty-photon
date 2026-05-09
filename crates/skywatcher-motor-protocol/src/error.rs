//! Protocol-layer errors.

/// Errors that can arise when encoding or decoding a Sky-Watcher motor-protocol
/// frame, or when the controller itself reports a numbered error reply.
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
pub enum ProtocolError {
    /// The frame did not start with `:`, did not end with `\r`, had trailing
    /// bytes after `\r`, or was otherwise structurally malformed.
    #[error("frame error: {0}")]
    FrameError(String),

    /// A 24-bit (or 16-bit) hex payload contained non-hex bytes or was the
    /// wrong length.
    #[error("hex decode error: {0}")]
    HexError(String),

    /// The payload of an `=` reply did not match the shape expected for the
    /// command that elicited it (e.g. wrong number of bytes, unknown status
    /// flag).
    #[error("payload error: {0}")]
    PayloadError(String),

    /// The controller replied with `!XX\r` where `XX` is the two-hex-digit
    /// mount-side error byte. Carries the decoded [`MountErrorCode`] so
    /// callers can map it to an ASCOM error.
    #[error("mount error: {0:?}")]
    MountError(MountErrorCode),
}

/// Numbered mount-side errors as listed in the [Sky-Watcher motor-controller
/// command set] §"Error responses".
///
/// [Sky-Watcher motor-controller command set]: https://inter-static.skywatcher.com/downloads/skywatcher_motor_controller_command_set.pdf
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum MountErrorCode {
    /// `0` — unknown command character.
    UnknownCommand,
    /// `1` — frame length did not match the command's payload spec.
    CommandLengthError,
    /// `2` — motion command issued while the motor was still running.
    MotorNotStopped,
    /// `3` — non-hex character in payload.
    InvalidCharacter,
    /// `4` — operation attempted before `:F` initialisation.
    NotInitialized,
    /// `5` — driver is in low-power sleep mode.
    DriverSleeping,
    /// `7` — PEC training is in progress.
    PecTrainingRunning,
    /// `8` — PEC playback requested but no training data exists.
    NoValidPecData,
    /// Any error byte the spec does not name. Carries the raw byte value.
    Unknown(u8),
}

impl MountErrorCode {
    /// Decode a mount-side error byte (`0x00`..=`0xFF`) into a
    /// [`MountErrorCode`].
    ///
    /// `code` is the parsed numeric value of the two ASCII hex digits in the
    /// `!XX\r` wire frame, not the ASCII bytes themselves; callers that have
    /// raw hex bytes should run them through [`crate::codec::decode_u8`]
    /// first. The named variants (`UnknownCommand`..=`NoValidPecData`)
    /// correspond to the single-nibble codes the spec lists; everything else
    /// maps to [`MountErrorCode::Unknown`].
    pub fn from_byte(code: u8) -> Self {
        match code {
            0 => Self::UnknownCommand,
            1 => Self::CommandLengthError,
            2 => Self::MotorNotStopped,
            3 => Self::InvalidCharacter,
            4 => Self::NotInitialized,
            5 => Self::DriverSleeping,
            7 => Self::PecTrainingRunning,
            8 => Self::NoValidPecData,
            other => Self::Unknown(other),
        }
    }
}

/// Result alias used throughout this crate.
pub type Result<T> = std::result::Result<T, ProtocolError>;
