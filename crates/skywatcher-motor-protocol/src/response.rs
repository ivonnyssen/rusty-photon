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
/// The wire payload is **three independent nibbles** per Sky-Watcher
/// motor-controller spec §5 (Response E):
///
/// | Nibble | Bit | Meaning |
/// |--------|-----|---------|
/// | 1st (mode)   | 0 (`0x1`) | `1=Tracking`, `0=Goto` |
/// | 1st          | 1 (`0x2`) | `1=CCW`, `0=CW` |
/// | 1st          | 2 (`0x4`) | `1=Fast`, `0=Slow` |
/// | 2nd (motion) | 0 (`0x1`) | `1=Running`, `0=Stopped` |
/// | 2nd          | 1 (`0x2`) | `1=Blocked`, `0=Normal` |
/// | 3rd (init)   | 0 (`0x1`) | `1=Initialised`, `0=Not initialised` |
/// | 3rd          | 1 (`0x2`) | `1=Level-switch on`, `0=off` |
///
/// Original codec had bit 1 of nibble 1 as "Forward" — that's
/// inverted from the spec. The flag is `ccw` (counter-clockwise) and
/// matches the same bit position used by `:G`'s DB2.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct AxisStatus {
    /// Motor is currently producing step pulses.
    pub running: bool,
    /// Currently in goto mode (vs tracking mode).
    pub goto: bool,
    /// Currently stepping in the CCW direction (CW otherwise).
    pub ccw: bool,
    /// Motor is in high-speed (goto/slew) regime.
    pub fast: bool,
    /// Motor reports `Blocked` (e.g. hit an endstop or a clutch is
    /// preventing it from following the commanded steps). Not used by
    /// the MVP but reported by the spec.
    pub blocked: bool,
    /// `:F<axis>` has been issued at least once since power-on.
    pub initialized: bool,
    /// Mount-side level switch reports on. Not used by the MVP.
    pub level_switch_on: bool,
}

impl AxisStatus {
    /// Decode the three-nibble `:f<axis>` payload. See the table on
    /// [`AxisStatus`] for the bit assignments.
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
            ccw: (n0 & 0x2) != 0,
            fast: (n0 & 0x4) != 0,
            running: (n1 & 0x1) != 0,
            blocked: (n1 & 0x2) != 0,
            initialized: (n2 & 0x1) != 0,
            level_switch_on: (n2 & 0x2) != 0,
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
            // Error reply. Per spec §4: two hex digits → one byte
            // error code. Empirical: the Star Adventurer GTi
            // returns a single hex digit for single-digit codes
            // (all the documented codes 0..8 fit). Accept either.
            let code = match payload.len() {
                1 => decode_nibble(payload[0])?,
                2 => decode_u8([payload[0], payload[1]])?,
                n => {
                    return Err(ProtocolError::FrameError(format!(
                        "error response payload must be 1 or 2 hex chars, got {n}"
                    )))
                }
            };
            return Err(ProtocolError::MountError(MountErrorCode::from_byte(code)));
        }

        // Success reply.
        match in_reply_to {
            Command::Initialize(_)
            | Command::SetMotionMode { .. }
            | Command::SetGotoTarget { .. }
            | Command::SetGotoTargetIncrement { .. }
            | Command::SetBreakPointIncrement { .. }
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
            | Command::InquireMotorBoardVersion(_) => {
                Ok(Self::U24(decode_u24(expect_u24_payload(payload)?)?))
            }
            Command::InquireHighSpeedRatio(_) => {
                // Empirical: the Star Adventurer GTi returns a 2-hex-byte
                // u8 payload for `:g<axis>` (value `0x01` on both axes),
                // not the 6-hex-byte u24 the original design doc
                // assumed. The Sky-Watcher motor-controller spec is
                // ambiguous on payload width for this command; widen to
                // accept both. INDI eqmod (the canonical reference)
                // also decodes `:g` as a small unsigned and stores it
                // as a single-byte ratio. We promote whichever width
                // we receive to a `u32` so the parameter cache stays
                // uniform.
                Ok(Self::U24(match payload.len() {
                    2 => {
                        let bytes = [payload[0], payload[1]];
                        u32::from(decode_u8(bytes)?)
                    }
                    6 => decode_u24(expect_u24_payload(payload)?)?,
                    n => {
                        return Err(ProtocolError::PayloadError(format!(
                            "expected 2- or 6-hex-byte payload for `:g`, got {n} bytes"
                        )))
                    }
                }))
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
            | Command::SetGotoTargetIncrement { axis: a, .. }
            | Command::SetBreakPointIncrement { axis: a, .. }
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
            kind: crate::command::ModeKind::Goto,
            speed: crate::command::Speed::Slow,
            ccw: false,
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
    fn decode_high_speed_ratio_accepts_u8_payload() {
        // Empirical wire trace on a real Star Adventurer GTi:
        //   `:g1\r` → `=01\r` (and same on axis 2). The mount returns
        // a 2-hex-byte u8, not the 6-hex-byte u24 originally assumed.
        // Promoted to a `u32` so the parameter cache stays uniform.
        let r = Response::decode(b"=01\r", &Command::InquireHighSpeedRatio(Axis::Ra)).unwrap();
        assert_eq!(r, Response::U24(0x01));
        // Other byte values round-trip identically.
        let r = Response::decode(b"=20\r", &Command::InquireHighSpeedRatio(Axis::Dec)).unwrap();
        assert_eq!(r, Response::U24(0x20));
    }

    #[test]
    fn decode_high_speed_ratio_accepts_u24_payload() {
        // Some Sky-Watcher mounts (per the spec PDF) reply with the
        // full 6-hex u24 form. Both shapes must round-trip identically.
        let r = Response::decode(b"=200000\r", &Command::InquireHighSpeedRatio(Axis::Ra)).unwrap();
        assert_eq!(r, Response::U24(0x20));
    }

    #[test]
    fn decode_high_speed_ratio_rejects_other_widths() {
        // 4-hex bytes is neither u8 nor u24 — reject so a corrupt
        // reply doesn't silently succeed with a truncated value.
        let r = Response::decode(b"=0123\r", &Command::InquireHighSpeedRatio(Axis::Ra));
        assert!(matches!(r, Err(ProtocolError::PayloadError(_))));
    }

    #[test]
    fn decode_status_inquiry_returns_axis_status() {
        // From the GTi probe table: =100\r → tracking-mode preset, motor
        // stopped, not initialised. Per AxisStatus::decode bit layout:
        //   nibble 0 = 1 → bit-0 set: tracking (goto=false); bit-1=0 CW
        //              (ccw=false); bit-2=0 slow (fast=false)
        //   nibble 1 = 0 → running=false, blocked=false
        //   nibble 2 = 0 → initialized=false, level_switch_on=false
        let r = Response::decode(b"=100\r", &Command::InquireStatus(Axis::Ra)).unwrap();
        let status = match r {
            Response::Status(s) => s,
            other => panic!("expected Status, got {other:?}"),
        };
        assert!(!status.goto);
        assert!(!status.ccw);
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
    fn decode_single_digit_error_reply_matches_gti_wire_format() {
        // Empirically the Star Adventurer GTi sends `!X\r` (3 bytes,
        // single hex digit) for single-digit error codes 0..8 —
        // see the doc comment on `validate_response_frame`. Both
        // widths must decode to the same `MountErrorCode`.
        let short = Response::decode(b"!2\r", &Command::StartMotion(Axis::Ra)).unwrap_err();
        let wide = Response::decode(b"!02\r", &Command::StartMotion(Axis::Ra)).unwrap_err();
        assert_eq!(short, wide);
        assert_eq!(
            short,
            ProtocolError::MountError(MountErrorCode::MotorNotStopped)
        );

        // `!4\r` was the empirical reply that surfaced this case
        // in the first place (`NotInitialized` after a stale `:F`).
        let err = Response::decode(b"!4\r", &Command::StartMotion(Axis::Ra)).unwrap_err();
        assert_eq!(
            err,
            ProtocolError::MountError(MountErrorCode::NotInitialized)
        );
    }

    #[test]
    fn axis_status_decode_recovers_blocked_and_level_switch_bits() {
        // Spec §5 Response E:
        //   nibble 1 bit 1 = Blocked
        //   nibble 2 bit 1 = Level-switch on
        //
        // 121 → nibble 0=1 (tracking-slow-CW); nibble 1=2 (blocked,
        // not running); nibble 2=1 (initialised).
        let s = AxisStatus::decode(b"121").unwrap();
        assert!(!s.running, "blocked alone shouldn't imply running");
        assert!(s.blocked, "blocked bit must propagate");
        assert!(s.initialized);
        assert!(!s.level_switch_on);

        // 133 → nibble 1=3 (running AND blocked — the realistic
        // "motor stepping but encoder not advancing" case);
        // nibble 2=3 (initialised + level-switch on).
        let s = AxisStatus::decode(b"133").unwrap();
        assert!(s.running);
        assert!(s.blocked);
        assert!(s.initialized);
        assert!(s.level_switch_on);
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
        // Per Sky-Watcher spec §5 (Response E):
        //   nibble 0 bit 0 = 1 → tracking; bit 1 = 1 → CCW; bit 2 = 1 → fast.
        //   nibble 1 bit 0 = 1 → running; bit 1 = 1 → blocked.
        //   nibble 2 bit 0 = 1 → initialised; bit 1 = 1 → level switch on.
        //
        // 411 → nibble 0 = 4 = 0b100 → bit-0 clear (goto), bit-1 clear (CW),
        // bit-2 set (fast); nibble 1 = 1 → running; nibble 2 = 1 →
        // initialised. This is the typical mid-goto, fast-CW state.
        let s = AxisStatus::decode(b"411").unwrap();
        assert!(s.goto, "goto flag");
        assert!(!s.ccw, "CW direction (ccw=false)");
        assert!(s.fast, "fast flag");
        assert!(s.running, "running flag");
        assert!(!s.blocked, "not blocked");
        assert!(s.initialized, "initialized flag");
        assert!(!s.level_switch_on, "level switch off");

        // 111 → nibble 0 = 1 → tracking-slow-CW; nibble 1 = 1 → running;
        // nibble 2 = 1 → initialised. Steady-state sidereal tracking.
        let s = AxisStatus::decode(b"111").unwrap();
        assert!(!s.goto, "tracking, not goto");
        assert!(!s.ccw, "CW direction");
        assert!(!s.fast, "slow tracking");
        assert!(s.running);
        assert!(s.initialized);

        // 711 → nibble 0 = 7 = 0b111: tracking + CCW + fast; nibble 1 / 2 =
        // running + initialised. Exercises every bit on nibble 0.
        let s = AxisStatus::decode(b"711").unwrap();
        assert!(!s.goto);
        assert!(s.ccw);
        assert!(s.fast);
        assert!(s.running);
        assert!(s.initialized);
    }
}
