//! Error types for the star-adventurer-gti driver.

use ascom_alpaca::{ASCOMError, ASCOMErrorCode};

/// Errors that can arise inside the driver.
#[derive(Debug, thiserror::Error)]
pub enum StarAdvError {
    #[error("not connected to mount")]
    NotConnected,

    #[error("connection failed: {0}")]
    ConnectionFailed(String),

    #[error("transport error: {0}")]
    Transport(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("timeout: {0}")]
    Timeout(String),

    #[error("protocol error: {0}")]
    Protocol(#[from] skywatcher_motor_protocol::ProtocolError),

    #[error("invalid value: {0}")]
    InvalidValue(String),

    #[error("invalid operation: {0}")]
    InvalidOperation(String),

    #[error("parked")]
    Parked,

    #[error("config error: {0}")]
    Config(String),

    /// ERFA-side time-conversion failure. In practice this is
    /// `eraCal2jd` (reached transitively from `Dtf2d`) returning
    /// `-1` for a year outside its calendar floor (`IYMIN = -4799`)
    /// — a host clock set absurdly far in the past. ERFA's
    /// leap-second-table boundary (years before 1960 or beyond
    /// `IYV + 5`) is *not* this error; `Utctai` reports it as
    /// status `1` ("dubious") which the erfars binding maps to
    /// `Ok((value, 1))`, so the LST still computes normally for
    /// any realistic future-shifted clock. The chrono-validated
    /// `Dtf2d` calendar-component checks are unreachable in
    /// practice but are folded into the same variant so the call
    /// site stays a total function.
    #[error("timekeeping error: {0}")]
    Timekeeping(String),

    /// The connect handshake's first wire query (`:e1`) returned a reply
    /// that doesn't look like a Sky-Watcher motor-board-version response.
    /// The most likely cause is that the configured transport target
    /// (serial port or UDP host) points at a different device — a power
    /// box, focuser, or unrelated USB-CDC peripheral sharing the host's
    /// USB bus. See [issue #254][issue].
    ///
    /// [issue]: https://github.com/ivonnyssen/rusty-photon/issues/254
    #[error(
        "handshake to {port} returned malformed data ({reason}); this device may not be a \
         Sky-Watcher motor controller. Common cause: wrong serial port (e.g. pointing at a \
         power box, focuser, or other device sharing the host's USB bus). Verify that {port} \
         is the GTi serial endpoint."
    )]
    WrongDevice { port: String, reason: String },
}

/// Map a driver error to the closest ASCOM error code.
///
/// This is the single point of truth for the [`StarAdvError`] →
/// [`ASCOMError`] conversion; call sites use `.into()` /
/// `ASCOMError::from(err)` to invoke it. Inline-matched here so there's no
/// detour through an intermediate named helper.
impl From<StarAdvError> for ASCOMError {
    fn from(err: StarAdvError) -> Self {
        let msg = err.to_string();
        match err {
            StarAdvError::NotConnected => ASCOMError::new(ASCOMErrorCode::NOT_CONNECTED, msg),
            StarAdvError::InvalidValue(_) => ASCOMError::new(ASCOMErrorCode::INVALID_VALUE, msg),
            StarAdvError::Parked => ASCOMError::new(ASCOMErrorCode::INVALID_WHILE_PARKED, msg),
            _ => ASCOMError::invalid_operation(msg),
        }
    }
}

/// Driver result alias.
pub type Result<T> = std::result::Result<T, StarAdvError>;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn not_connected_maps_to_ascom_not_connected() {
        let err: ASCOMError = StarAdvError::NotConnected.into();
        assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
    }

    #[test]
    fn invalid_value_maps_to_ascom_invalid_value() {
        let err: ASCOMError = StarAdvError::InvalidValue("ra out of range".to_string()).into();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_VALUE);
    }

    #[test]
    fn parked_maps_to_ascom_invalid_while_parked() {
        let err: ASCOMError = StarAdvError::Parked.into();
        assert_eq!(err.code, ASCOMErrorCode::INVALID_WHILE_PARKED);
    }

    #[test]
    fn other_variants_map_to_invalid_operation() {
        for err in [
            StarAdvError::ConnectionFailed("boom".into()),
            StarAdvError::Transport("eof".into()),
            StarAdvError::Timeout("read".into()),
            StarAdvError::InvalidOperation("double connect".into()),
            StarAdvError::Config("missing".into()),
            StarAdvError::Timekeeping("ERFA Utctai returned -1".into()),
            StarAdvError::WrongDevice {
                port: "/dev/ttyUSB1".into(),
                reason: "test reason".into(),
            },
        ] {
            let mapped: ASCOMError = err.into();
            assert_eq!(mapped.code, ASCOMErrorCode::INVALID_OPERATION);
        }
    }

    #[test]
    fn wrong_device_display_quotes_port_and_suggests_cause() {
        // The operator-facing message must name the port (twice — once
        // in the diagnostic head, once in the verify-the-port trailer)
        // and call out the wrong-device hypothesis. The handshake hook
        // in `manager.rs` constructs this variant only after the `:e1`
        // reply fails framing or whitelist, so the message correctly
        // implies "we tried to identify the device and it wasn't a
        // Sky-Watcher".
        let err = StarAdvError::WrongDevice {
            port: "/dev/ttyUSB1".into(),
            reason: "unknown mount-type byte 0xFF".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("/dev/ttyUSB1"), "msg: {msg}");
        assert!(msg.contains("unknown mount-type byte 0xFF"), "msg: {msg}");
        assert!(msg.contains("Sky-Watcher"), "msg: {msg}");
        assert!(msg.contains("wrong serial port"), "msg: {msg}");
    }

    #[test]
    fn protocol_error_converts_through_from() {
        let pe = skywatcher_motor_protocol::ProtocolError::FrameError("missing CR".to_string());
        let driver: StarAdvError = pe.into();
        assert!(matches!(driver, StarAdvError::Protocol(_)));
    }

    #[test]
    fn io_error_converts_through_from() {
        let io = std::io::Error::new(std::io::ErrorKind::TimedOut, "read timed out");
        let driver: StarAdvError = io.into();
        assert!(matches!(driver, StarAdvError::Io(_)));
    }
}
