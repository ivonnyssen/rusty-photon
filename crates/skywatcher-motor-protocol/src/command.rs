//! Outbound commands.
//!
//! The wire form is `: <cmd> <axis> <payload?> \r` where `<cmd>` is a single
//! ASCII letter (uppercase = setter / motion; lowercase = inquiry), `<axis>`
//! is `'1'`, `'2'`, or `'3'`, and `<payload>` is 0..=6 ASCII hex bytes.

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

/// Motion-mode flags for the `:G` command.
///
/// The wire payload is a two-nibble hex byte; the high nibble selects
/// goto/tracking and fast/slow, and the low nibble selects direction and a
/// few mode-specific bits. Spelled out as a struct so callers don't pass
/// magic hex through the API.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct MotionMode {
    pub goto: bool,
    pub fast: bool,
    pub forward: bool,
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
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub fn encode_into(&self, _out: &mut Vec<u8>) -> Result<()> {
        unimplemented!("Phase 3: format `:<cmd><axis><payload?>\\r` per design doc table")
    }

    /// Convenience: allocate a fresh `Vec<u8>` and encode into it.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(10);
        self.encode_into(&mut out)?;
        Ok(out)
    }
}
