//! Inbound responses.
//!
//! Two shapes:
//!
//! * `=<payload?>\r` — success. Payload is empty for setters and 0..=6 ASCII
//!   hex bytes for inquiries.
//! * `!<XX>\r` — error. `XX` is one ASCII hex byte (the mount-side error
//!   code); decoded into [`crate::error::MountErrorCode`].

use crate::command::{Axis, Command};
use crate::error::Result;

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
    pub fn decode(_frame: &[u8], _in_reply_to: &Command) -> Result<Self> {
        unimplemented!(
            "Phase 3: validate framing, branch on '=' vs '!', dispatch on the original Command"
        )
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
}
