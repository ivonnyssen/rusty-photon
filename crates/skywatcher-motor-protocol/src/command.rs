//! Outbound commands.
//!
//! The wire form is `: <cmd> <axis> <payload?> \r` where `<cmd>` is a single
//! ASCII letter (uppercase = setter / motion; lowercase = inquiry), `<axis>`
//! is `'1'`, `'2'`, or `'3'`, and `<payload>` is 0..=6 ASCII hex bytes.

use crate::codec::{encode_position, encode_u24, encode_u8};
use crate::error::Result;

/// Which physical axis a command targets.
///
/// The wire byte is `'1'` (RA / Az motor), `'2'` (Dec / Alt motor), or `'3'`
/// (both axes). `Both` is only valid for a subset of commands; the codec does
/// not police this — the caller is responsible for not asking for impossible
/// combinations.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Axis {
    Ra,
    Dec,
    Both,
}

impl Axis {
    fn wire_byte(self) -> u8 {
        match self {
            Self::Ra => b'1',
            Self::Dec => b'2',
            Self::Both => b'3',
        }
    }
}

/// Motion-mode flags for the `:G` command.
///
/// The wire payload is one byte (two hex digits). The bit layout used here:
///
/// | Bit | Meaning |
/// |-----|---------|
/// | `0x10` | `goto` (1) vs tracking (0) |
/// | `0x20` | `fast` (1) vs slow (0) |
/// | `0x01` | `!forward` — reverse direction (1) vs forward (0) |
///
/// This matches the layout used by the EQMOD Windows driver and the INDI
/// `indi-eqmod` driver source for the Sky-Watcher protocol family. Common
/// values:
/// * `0x00` — tracking, slow, forward (sidereal default)
/// * `0x01` — tracking, slow, reverse
/// * `0x10` — goto, slow, forward
/// * `0x30` — goto, fast, forward (typical slew)
/// * `0x31` — goto, fast, reverse
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct MotionMode {
    pub goto: bool,
    pub fast: bool,
    pub forward: bool,
}

impl MotionMode {
    /// Tracking (sidereal) preset: tracking, slow, forward.
    pub const TRACKING: Self = Self {
        goto: false,
        fast: false,
        forward: true,
    };
    /// Goto/slew preset: goto, fast, forward (caller flips `forward` from
    /// the sign of the tick delta).
    pub const GOTO_FAST_FORWARD: Self = Self {
        goto: true,
        fast: true,
        forward: true,
    };

    /// Pack this mode into the single byte the `:G` command expects on the
    /// wire.
    pub fn to_byte(self) -> u8 {
        let mut byte = 0u8;
        if self.goto {
            byte |= 0x10;
        }
        if self.fast {
            byte |= 0x20;
        }
        if !self.forward {
            byte |= 0x01;
        }
        byte
    }
}

/// Every command the MVP issues. See
/// [`docs/services/star-adventurer-gti.md`](../../../docs/services/star-adventurer-gti.md)
/// §"Commands used by the MVP" for the table that drives this enum.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Command {
    /// `:F<axis>` — initialise axis.
    Initialize(Axis),
    /// `:a<axis>` — inquire counts-per-revolution.
    InquireCpr(Axis),
    /// `:b1` — inquire timer-interrupt frequency. Always axis 1.
    InquireTmrFreq,
    /// `:g<axis>` — inquire high-speed ratio.
    InquireHighSpeedRatio(Axis),
    /// `:e<axis>` — inquire motor-board version.
    InquireMotorBoardVersion(Axis),
    /// `:j<axis>` — inquire current encoder position.
    InquirePosition(Axis),
    /// `:f<axis>` — inquire axis status (running / mode / direction bits).
    InquireStatus(Axis),
    /// `:G<axis><mode>` — set motion mode.
    SetMotionMode { axis: Axis, mode: MotionMode },
    /// `:S<axis><pos>` — set absolute goto target (signed encoder ticks,
    /// bias-encoded by the codec).
    SetGotoTarget { axis: Axis, ticks: i32 },
    /// `:I<axis><period>` — set step period (T1 preset). 24-bit unsigned.
    SetStepPeriod { axis: Axis, period: u32 },
    /// `:E<axis><pos>` — set current axis position (sync). Signed encoder
    /// ticks; bias-encoded by the codec.
    SetPosition { axis: Axis, ticks: i32 },
    /// `:J<axis>` — start motion.
    StartMotion(Axis),
    /// `:K<axis>` — stop motion (decelerate).
    StopMotion(Axis),
    /// `:L<axis>` — instant stop (used for AbortSlew).
    InstantStop(Axis),
}

impl Command {
    /// Encode this command into `out`, including the leading `:` and the
    /// trailing `\r`. Appends; does not clear `out` first.
    pub fn encode_into(&self, out: &mut Vec<u8>) -> Result<()> {
        out.push(b':');
        match *self {
            Self::Initialize(axis) => {
                out.push(b'F');
                out.push(axis.wire_byte());
            }
            Self::InquireCpr(axis) => {
                out.push(b'a');
                out.push(axis.wire_byte());
            }
            Self::InquireTmrFreq => {
                out.extend_from_slice(b"b1");
            }
            Self::InquireHighSpeedRatio(axis) => {
                out.push(b'g');
                out.push(axis.wire_byte());
            }
            Self::InquireMotorBoardVersion(axis) => {
                out.push(b'e');
                out.push(axis.wire_byte());
            }
            Self::InquirePosition(axis) => {
                out.push(b'j');
                out.push(axis.wire_byte());
            }
            Self::InquireStatus(axis) => {
                out.push(b'f');
                out.push(axis.wire_byte());
            }
            Self::SetMotionMode { axis, mode } => {
                out.push(b'G');
                out.push(axis.wire_byte());
                out.extend_from_slice(&encode_u8(mode.to_byte()));
            }
            Self::SetGotoTarget { axis, ticks } => {
                out.push(b'S');
                out.push(axis.wire_byte());
                out.extend_from_slice(&encode_position(ticks)?);
            }
            Self::SetStepPeriod { axis, period } => {
                out.push(b'I');
                out.push(axis.wire_byte());
                out.extend_from_slice(&encode_u24(period));
            }
            Self::SetPosition { axis, ticks } => {
                out.push(b'E');
                out.push(axis.wire_byte());
                out.extend_from_slice(&encode_position(ticks)?);
            }
            Self::StartMotion(axis) => {
                out.push(b'J');
                out.push(axis.wire_byte());
            }
            Self::StopMotion(axis) => {
                out.push(b'K');
                out.push(axis.wire_byte());
            }
            Self::InstantStop(axis) => {
                out.push(b'L');
                out.push(axis.wire_byte());
            }
        }
        out.push(b'\r');
        Ok(())
    }

    /// Convenience: allocate a fresh `Vec<u8>` and encode into it.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(10);
        self.encode_into(&mut out)?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_mode_bit_layout() {
        assert_eq!(MotionMode::TRACKING.to_byte(), 0x00);
        assert_eq!(MotionMode::GOTO_FAST_FORWARD.to_byte(), 0x30);
        // tracking + reverse
        assert_eq!(
            MotionMode {
                goto: false,
                fast: false,
                forward: false,
            }
            .to_byte(),
            0x01
        );
        // goto + slow + reverse
        assert_eq!(
            MotionMode {
                goto: true,
                fast: false,
                forward: false,
            }
            .to_byte(),
            0x11
        );
        // goto + fast + reverse
        assert_eq!(
            MotionMode {
                goto: true,
                fast: true,
                forward: false,
            }
            .to_byte(),
            0x31
        );
    }

    #[test]
    fn axis_wire_bytes() {
        assert_eq!(Axis::Ra.wire_byte(), b'1');
        assert_eq!(Axis::Dec.wire_byte(), b'2');
        assert_eq!(Axis::Both.wire_byte(), b'3');
    }

    #[test]
    fn handshake_inquiries_encode_to_design_doc_form() {
        // From docs/services/star-adventurer-gti.md §"Initialisation sequence".
        assert_eq!(Command::Initialize(Axis::Ra).encode().unwrap(), b":F1\r");
        assert_eq!(Command::Initialize(Axis::Dec).encode().unwrap(), b":F2\r");
        assert_eq!(Command::InquireCpr(Axis::Ra).encode().unwrap(), b":a1\r");
        assert_eq!(Command::InquireCpr(Axis::Dec).encode().unwrap(), b":a2\r");
        assert_eq!(Command::InquireTmrFreq.encode().unwrap(), b":b1\r");
        assert_eq!(
            Command::InquireHighSpeedRatio(Axis::Ra).encode().unwrap(),
            b":g1\r"
        );
        assert_eq!(
            Command::InquireMotorBoardVersion(Axis::Ra)
                .encode()
                .unwrap(),
            b":e1\r"
        );
        assert_eq!(
            Command::InquirePosition(Axis::Ra).encode().unwrap(),
            b":j1\r"
        );
        assert_eq!(Command::InquireStatus(Axis::Ra).encode().unwrap(), b":f1\r");
    }

    #[test]
    fn motion_setters_encode_with_payloads() {
        // :G1<mode> with the GOTO_FAST_FORWARD preset → 0x30 → "30"
        assert_eq!(
            Command::SetMotionMode {
                axis: Axis::Ra,
                mode: MotionMode::GOTO_FAST_FORWARD,
            }
            .encode()
            .unwrap(),
            b":G130\r"
        );
        // :S1 with target encoder 0 → bias `0x800000` → "000080"
        assert_eq!(
            Command::SetGotoTarget {
                axis: Axis::Ra,
                ticks: 0,
            }
            .encode()
            .unwrap(),
            b":S1000080\r"
        );
        // :I1 with period 0x123456 → low-byte first "563412"
        assert_eq!(
            Command::SetStepPeriod {
                axis: Axis::Ra,
                period: 0x12_3456,
            }
            .encode()
            .unwrap(),
            b":I1563412\r"
        );
        // :E2 sync to ticks -1 → 0x7FFFFF → "FFFF7F"
        assert_eq!(
            Command::SetPosition {
                axis: Axis::Dec,
                ticks: -1,
            }
            .encode()
            .unwrap(),
            b":E2FFFF7F\r"
        );
    }

    #[test]
    fn motion_starters_and_stoppers_encode_without_payload() {
        assert_eq!(Command::StartMotion(Axis::Ra).encode().unwrap(), b":J1\r");
        assert_eq!(Command::StopMotion(Axis::Ra).encode().unwrap(), b":K1\r");
        assert_eq!(Command::InstantStop(Axis::Ra).encode().unwrap(), b":L1\r");
        assert_eq!(Command::InstantStop(Axis::Dec).encode().unwrap(), b":L2\r");
    }

    #[test]
    fn encode_into_appends_does_not_clear() {
        let mut out = vec![b'X', b'Y'];
        Command::Initialize(Axis::Ra).encode_into(&mut out).unwrap();
        assert_eq!(out, b"XY:F1\r");
    }

    #[test]
    fn out_of_range_position_propagates_error() {
        // Beyond signed-24-bit range — should bubble up as ProtocolError.
        let result = Command::SetGotoTarget {
            axis: Axis::Ra,
            ticks: i32::MAX,
        }
        .encode();
        assert!(result.is_err());
    }
}
