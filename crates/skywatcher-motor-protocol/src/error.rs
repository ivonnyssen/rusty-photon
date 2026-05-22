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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn from_byte_maps_named_codes() {
        assert_eq!(MountErrorCode::from_byte(0), MountErrorCode::UnknownCommand);
        assert_eq!(
            MountErrorCode::from_byte(1),
            MountErrorCode::CommandLengthError
        );
        assert_eq!(
            MountErrorCode::from_byte(2),
            MountErrorCode::MotorNotStopped
        );
        assert_eq!(
            MountErrorCode::from_byte(3),
            MountErrorCode::InvalidCharacter
        );
        assert_eq!(MountErrorCode::from_byte(4), MountErrorCode::NotInitialized);
        assert_eq!(MountErrorCode::from_byte(5), MountErrorCode::DriverSleeping);
        assert_eq!(
            MountErrorCode::from_byte(7),
            MountErrorCode::PecTrainingRunning
        );
        assert_eq!(MountErrorCode::from_byte(8), MountErrorCode::NoValidPecData);
    }

    #[test]
    fn from_byte_reserves_unknown_for_unspecified_values() {
        // The spec leaves `6` undefined; the codec must surface it as Unknown
        // rather than silently mapping to a named variant.
        assert_eq!(MountErrorCode::from_byte(6), MountErrorCode::Unknown(6));
    }

    #[test]
    fn from_byte_passes_high_bytes_through_unknown() {
        // Wire form is `!XX\r` — full byte. Anything > 0x0F still round-trips
        // cleanly into Unknown(byte).
        assert_eq!(
            MountErrorCode::from_byte(0x9A),
            MountErrorCode::Unknown(0x9A)
        );
        assert_eq!(
            MountErrorCode::from_byte(0xFF),
            MountErrorCode::Unknown(0xFF)
        );
    }

    #[test]
    fn protocol_error_display_includes_payload() {
        let err = ProtocolError::FrameError("missing CR".to_string());
        assert_eq!(format!("{err}"), "frame error: missing CR");

        let err = ProtocolError::HexError("bad nibble".to_string());
        assert_eq!(format!("{err}"), "hex decode error: bad nibble");

        let err = ProtocolError::PayloadError("short".to_string());
        assert_eq!(format!("{err}"), "payload error: short");

        let err = ProtocolError::MountError(MountErrorCode::NotInitialized);
        assert_eq!(format!("{err}"), "mount error: NotInitialized");
    }
}
