//! Inbound responses.
//!
//! Two shapes:
//!
//! * `=<payload?>\r` — success. Payload is empty for setters and 0..=6 ASCII
//!   hex bytes for inquiries.
//! * `!<XX>\r` — error. `XX` is one ASCII hex byte (the mount-side error
//!   code); decoded into [`crate::error::MountErrorCode`].

use crate::codec::{decode_position, decode_u24, decode_u8, validate_response_frame};
use crate::command::{Axis, Command};
use crate::error::{MountErrorCode, ProtocolError, Result};

/// Decoded status bits returned by the `:f<axis>` inquiry.
///
/// The wire payload is three nibbles, each a bitfield. `Initialised` is the
/// bit you most often want to check after a `:F` handshake; `Running` plus
/// `Goto` together are how you tell a slew has finished (`Running == false`
/// while still in `Goto` mode means the goto reached its target and stopped).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct AxisStatus {
    /// Motor is currently producing step pulses.
    pub running: bool,
    /// Currently in goto mode (vs tracking mode).
    pub goto: bool,
    /// Currently moving in the "forward" direction.
    pub forward: bool,
    /// Motor is in high-speed (goto/slew) regime.
    pub fast: bool,
    /// `:F<axis>` has been issued at least once since power-on.
    pub initialized: bool,
}

impl AxisStatus {
    /// Decode the three-nibble `:f<axis>` payload.
    ///
    /// Wire form is three ASCII hex digits. The digits encode:
    ///
    /// | Digit | Bit | Meaning |
    /// |-------|-----|---------|
    /// | 1st   | 0x1 | Tracking-mode update (1) vs Goto-mode update (0) |
    /// | 1st   | 0x2 | Forward (1) vs Reverse (0) |
    /// | 1st   | 0x4 | Fast (1) vs Slow (0) |
    /// | 2nd   | 0x1 | Running (1) vs Stopped (0) |
    /// | 3rd   | 0x1 | Initialised (1) vs Not-initialised (0) |
    ///
    /// Layout matches EQMOD's `STATUS_DECODE` and the `indi-eqmod`
    /// reference; consult those when validating against real hardware.
    pub fn decode(payload: &[u8]) -> Result<Self> {
        if payload.len() != 3 {
            return Err(ProtocolError::PayloadError(format!(
                "axis status payload must be 3 hex digits, got {}",
                payload.len()
            )));
        }
        let n0 = decode_nibble(payload[0])?;
        let n1 = decode_nibble(payload[1])?;
        let n2 = decode_nibble(payload[2])?;
        Ok(Self {
            goto: (n0 & 0x1) == 0,
            forward: (n0 & 0x2) != 0,
            fast: (n0 & 0x4) != 0,
            running: (n1 & 0x1) != 0,
            initialized: (n2 & 0x1) != 0,
        })
    }
}

fn decode_nibble(b: u8) -> Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        other => Err(ProtocolError::PayloadError(format!(
            "expected ASCII hex digit, got {other:#04x}"
        ))),
    }
}

/// Every response shape the driver consumes.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Response {
    /// `=\r` — empty success acknowledgement.
    Ack,
    /// Single-byte payload, unpacked from two ASCII hex digits.
    U8(u8),
    /// 24-bit unsigned payload (e.g. CPR, TMR_Freq, high-speed ratio).
    U24(u32),
    /// 24-bit signed payload with the `0x800000` bias removed (encoder
    /// position).
    Position(i32),
    /// Decoded `:f<axis>` status payload.
    Status(AxisStatus),
}

impl Response {
    /// Decode a single response frame, including the `=` or `!` prefix and
    /// the trailing `\r`, in the context of the [`Command`] that elicited it.
    ///
    /// The command is needed because the same wire shape (e.g. a 6-hex-byte
    /// `=` reply) decodes differently depending on what was asked for —
    /// `:j1` returns a [`Response::Position`] (signed, debiased) whereas
    /// `:a1` returns a [`Response::U24`] (unsigned).
    pub fn decode(frame: &[u8], in_reply_to: &Command) -> Result<Self> {
        validate_response_frame(frame)?;
        // Strip the leading prefix and the trailing `\r`.
        let prefix = frame[0];
        let payload = &frame[1..frame.len() - 1];

        if prefix == b'!' {
            // Error reply: payload is exactly two hex digits forming a byte.
            let bytes = [payload[0], payload[1]];
            let code = decode_u8(bytes)?;
            return Err(ProtocolError::MountError(MountErrorCode::from_byte(code)));
        }

        // Success reply.
        match in_reply_to {
            Command::Initialize(_)
            | Command::SetMotionMode { .. }
            | Command::SetGotoTarget { .. }
            | Command::SetStepPeriod { .. }
            | Command::SetPosition { .. }
            | Command::StartMotion(_)
            | Command::StopMotion(_)
            | Command::InstantStop(_) => {
                if !payload.is_empty() {
                    return Err(ProtocolError::PayloadError(format!(
                        "expected empty `=\\r` ack, got {} payload bytes",
                        payload.len()
                    )));
                }
                Ok(Self::Ack)
            }
            Command::InquirePosition(_) => Ok(Self::Position(decode_position(
                expect_u24_payload(payload)?,
            )?)),
            Command::InquireCpr(_)
            | Command::InquireTmrFreq
            | Command::InquireHighSpeedRatio(_)
            | Command::InquireMotorBoardVersion(_) => {
                Ok(Self::U24(decode_u24(expect_u24_payload(payload)?)?))
            }
            Command::InquireStatus(_) => Ok(Self::Status(AxisStatus::decode(payload)?)),
        }
    }

    /// Helper: return the [`Axis`] the reply pertains to. Always derived from
    /// the originating command — the reply itself does not echo the axis.
    pub fn axis_of(in_reply_to: &Command) -> Option<Axis> {
        match in_reply_to {
            Command::Initialize(a)
            | Command::InquireCpr(a)
            | Command::InquireHighSpeedRatio(a)
            | Command::InquireMotorBoardVersion(a)
            | Command::InquirePosition(a)
            | Command::InquireStatus(a)
            | Command::SetMotionMode { axis: a, .. }
            | Command::SetGotoTarget { axis: a, .. }
            | Command::SetStepPeriod { axis: a, .. }
            | Command::SetPosition { axis: a, .. }
            | Command::StartMotion(a)
            | Command::StopMotion(a)
            | Command::InstantStop(a) => Some(*a),
            Command::InquireTmrFreq => Some(Axis::Ra),
        }
    }
}

fn expect_u24_payload(payload: &[u8]) -> Result<&[u8; 6]> {
    payload.try_into().map_err(|_| {
        ProtocolError::PayloadError(format!(
            "expected 6-hex-byte u24 payload, got {} bytes",
            payload.len()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::MotionMode;

    fn mode() -> MotionMode {
        MotionMode {
            goto: true,
            fast: false,
            forward: true,
        }
    }

    #[test]
    fn axis_of_returns_command_axis_for_axis_carrying_commands() {
        assert_eq!(
            Response::axis_of(&Command::Initialize(Axis::Ra)),
            Some(Axis::Ra)
        );
        assert_eq!(
            Response::axis_of(&Command::Initialize(Axis::Dec)),
            Some(Axis::Dec)
        );
        assert_eq!(
            Response::axis_of(&Command::InquireCpr(Axis::Dec)),
            Some(Axis::Dec)
        );
        assert_eq!(
            Response::axis_of(&Command::InquirePosition(Axis::Ra)),
            Some(Axis::Ra)
        );
        assert_eq!(
            Response::axis_of(&Command::InquireStatus(Axis::Both)),
            Some(Axis::Both)
        );
        assert_eq!(
            Response::axis_of(&Command::SetMotionMode {
                axis: Axis::Ra,
                mode: mode()
            }),
            Some(Axis::Ra)
        );
        assert_eq!(
            Response::axis_of(&Command::SetGotoTarget {
                axis: Axis::Dec,
                ticks: 42
            }),
            Some(Axis::Dec)
        );
        assert_eq!(
            Response::axis_of(&Command::SetStepPeriod {
                axis: Axis::Ra,
                period: 1000
            }),
            Some(Axis::Ra)
        );
        assert_eq!(
            Response::axis_of(&Command::SetPosition {
                axis: Axis::Dec,
                ticks: -7
            }),
            Some(Axis::Dec)
        );
        assert_eq!(
            Response::axis_of(&Command::StartMotion(Axis::Ra)),
            Some(Axis::Ra)
        );
        assert_eq!(
            Response::axis_of(&Command::StopMotion(Axis::Dec)),
            Some(Axis::Dec)
        );
        assert_eq!(
            Response::axis_of(&Command::InstantStop(Axis::Ra)),
            Some(Axis::Ra)
        );
    }

    #[test]
    fn axis_of_inquire_tmr_freq_is_always_ra() {
        // `:b1` is the only axis-1-only inquiry; the codec must report Ra,
        // not Both, so callers can route the reply through the same per-axis
        // dispatch path as the rest.
        assert_eq!(Response::axis_of(&Command::InquireTmrFreq), Some(Axis::Ra));
    }

    #[test]
    fn decode_ack_for_setter_commands() {
        let r = Response::decode(b"=\r", &Command::Initialize(Axis::Ra)).unwrap();
        assert_eq!(r, Response::Ack);
        let r = Response::decode(b"=\r", &Command::StartMotion(Axis::Ra)).unwrap();
        assert_eq!(r, Response::Ack);
    }

    #[test]
    fn decode_position_inquiry_returns_signed_ticks() {
        // 0x800000 bias → encoder count 0
        let r = Response::decode(b"=000080\r", &Command::InquirePosition(Axis::Ra)).unwrap();
        assert_eq!(r, Response::Position(0));

        // 0x7FFFFF (FFFF7F low-byte first) → encoder count -1
        let r = Response::decode(b"=FFFF7F\r", &Command::InquirePosition(Axis::Ra)).unwrap();
        assert_eq!(r, Response::Position(-1));
    }

    #[test]
    fn decode_cpr_inquiry_returns_unsigned_u24() {
        // From the GTi probe table: =005F37\r → 0x375F00 = 3,628,800
        let r = Response::decode(b"=005F37\r", &Command::InquireCpr(Axis::Ra)).unwrap();
        assert_eq!(r, Response::U24(0x37_5F00));
    }

    #[test]
    fn decode_status_inquiry_returns_axis_status() {
        // From the GTi probe table: =100\r → tracking-mode preset, motor
        // stopped, not initialised. Per AxisStatus::decode bit layout:
        // first nibble 1 → goto=false (tracking), forward=false, fast=false
        // second nibble 0 → running=false
        // third nibble 0 → initialized=false
        let r = Response::decode(b"=100\r", &Command::InquireStatus(Axis::Ra)).unwrap();
        let status = match r {
            Response::Status(s) => s,
            other => panic!("expected Status, got {other:?}"),
        };
        assert!(!status.goto);
        assert!(!status.forward);
        assert!(!status.fast);
        assert!(!status.running);
        assert!(!status.initialized);
    }

    #[test]
    fn decode_error_reply_yields_mount_error() {
        // !04 is "NotInitialized".
        let err = Response::decode(b"!04\r", &Command::StartMotion(Axis::Ra)).unwrap_err();
        assert_eq!(
            err,
            ProtocolError::MountError(MountErrorCode::NotInitialized)
        );

        // !02 is "MotorNotStopped".
        let err = Response::decode(
            b"!02\r",
            &Command::SetGotoTarget {
                axis: Axis::Ra,
                ticks: 0,
            },
        )
        .unwrap_err();
        assert_eq!(
            err,
            ProtocolError::MountError(MountErrorCode::MotorNotStopped)
        );
    }

    #[test]
    fn decode_rejects_malformed_frames() {
        // Setter command but reply has an unexpected payload.
        let r = Response::decode(b"=ABC\r", &Command::Initialize(Axis::Ra));
        assert!(matches!(r, Err(ProtocolError::PayloadError(_))));

        // Inquiry that needs 6 hex bytes but reply has 3.
        let r = Response::decode(b"=123\r", &Command::InquireCpr(Axis::Ra));
        assert!(matches!(r, Err(ProtocolError::PayloadError(_))));

        // No leading prefix.
        let r = Response::decode(b"000080\r", &Command::InquirePosition(Axis::Ra));
        assert!(matches!(r, Err(ProtocolError::FrameError(_))));

        // No trailing `\r`.
        let r = Response::decode(b"=000080", &Command::InquirePosition(Axis::Ra));
        assert!(matches!(r, Err(ProtocolError::FrameError(_))));
    }

    #[test]
    fn axis_status_decode_recovers_goto_running_flags() {
        // First nibble bits: 0x1=tracking-flag (set→tracking, clear→goto),
        // 0x2=forward, 0x4=fast.
        // Second nibble: 0x1=running.
        // Third nibble: 0x1=initialised.
        // 612 → 6=fast+forward+goto, 1=running, 2=??? — third nibble
        // 2 has bit 0x1 clear so initialised=false. (Real mounts report
        // 1 here once initialised; the value 2 is hypothetical.)
        // Use a more realistic payload instead:
        // 631: 6=fast+forward+goto, 3=running+(unused), 1=initialised.
        let s = AxisStatus::decode(b"631").unwrap();
        assert!(s.goto, "goto flag");
        assert!(s.forward, "forward flag");
        assert!(s.fast, "fast flag");
        assert!(s.running, "running flag");
        assert!(s.initialized, "initialized flag");

        // 111: first nibble bit 0x1 set → tracking, second nibble 1 →
        // running, third nibble 1 → initialised. (Tracking sidereal in
        // progress, the steady-state for sidereal observation.)
        let s = AxisStatus::decode(b"111").unwrap();
        assert!(!s.goto, "tracking, not goto");
        assert!(!s.forward, "no forward bit set in 1");
        assert!(!s.fast, "no fast bit set in 1");
        assert!(s.running);
        assert!(s.initialized);
    }
}
